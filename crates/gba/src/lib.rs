//! A BIOS-less Game Boy Advance core. The host supplies cartridge bytes, pressed
//! keys and frame pacing, and owns persistence of the cartridge SRAM.
mod bios_affine;
mod bios_math;
mod bus;
mod compression;
mod cpu;
mod video;

use std::fmt;

use bus::{LINES, Memory, SCANLINE};
use cpu::{Bus, Cpu, Step};

pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 160;
pub const HEADER_LEN: usize = 0xc0;
pub const MAX_ROM_LEN: usize = 32 * 1024 * 1024;
pub const FRAME_RATE: f64 = 16_777_216.0 / (SCANLINE as f64 * LINES as f64);

#[derive(Debug)]
pub enum Error {
    InvalidRomSize(usize),
    InvalidSramSize(usize),
    Cpu(String),
    InvalidCompressedData(&'static str),
    UnsupportedBiosCall(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRomSize(size) => write!(
                formatter,
                "Invalid GBA ROM length ({size} bytes, expected 192 bytes through 32 MiB)"
            ),
            Self::InvalidSramSize(size) => write!(
                formatter,
                "Invalid GBA SRAM length ({size} bytes, expected 32 KiB)"
            ),
            Self::InvalidCompressedData(reason) => {
                write!(formatter, "Invalid compressed GBA data ({reason})")
            }
            Self::Cpu(message) => formatter.write_str(message),
            Self::UnsupportedBiosCall(code) => {
                write!(formatter, "Unsupported GBA BIOS call 0x{code:02x}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// One cartridge session, including CPU, memory, LCD state, keypad and save RAM.
pub struct Machine {
    cpu: Cpu,
    memory: Memory,
    bios_wait: Option<u16>,
}

impl Machine {
    /// Boots at the cartridge entry point. Homebrew cartridges can omit the boot logo.
    pub fn from_rom(rom: Vec<u8>) -> Result<Self, Error> {
        if !(HEADER_LEN..=MAX_ROM_LEN).contains(&rom.len()) {
            return Err(Error::InvalidRomSize(rom.len()));
        }
        Ok(Self {
            cpu: Cpu::default(),
            memory: Memory::new(rom),
            bios_wait: None,
        })
    }

    /// Reboots the cartridge, retaining its battery-backed save.
    pub fn reset(&mut self) {
        self.cpu = Cpu::default();
        self.memory.reset();
        self.bios_wait = None;
    }

    /// Packed RGB888 rows in display order, updated by each completed scanline.
    pub fn pixels(&self) -> &[u8] {
        &self.memory.video.pixels
    }

    /// Pressed bits from low to high: A, B, Select, Start, Right, Left, Up, Down, R, L.
    pub fn set_buttons(&mut self, pressed: u16) {
        self.memory.set_buttons(pressed);
    }

    /// Raw 32 KiB SRAM image suitable for an on-disk battery save.
    pub fn sram(&self) -> &[u8] {
        &self.memory.sram
    }

    /// Loads a complete battery save. A length mismatch preserves the previous image.
    pub fn restore_sram(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() != self.memory.sram.len() {
            return Err(Error::InvalidSramSize(bytes.len()));
        }
        self.memory.sram.copy_from_slice(bytes);
        self.memory.sram_dirty = false;
        Ok(())
    }

    /// Reports writes since the previous poll or successful restore.
    pub fn take_sram_dirty(&mut self) -> bool {
        std::mem::take(&mut self.memory.sram_dirty)
    }

    /// Advances 228 scanlines. CPU instruction timing is approximate.
    pub fn run_frame(&mut self) -> Result<(), Error> {
        let mut remaining = SCANLINE * u32::from(LINES);
        while remaining > 0 {
            if self.memory.irq_pending() {
                self.cpu.service_irq(&mut self.memory);
            }
            let step = if self.waiting() {
                self.memory.cycles_to_event()
            } else {
                if let Step::Swi(code) = self.cpu.step(&mut self.memory).map_err(Error::Cpu)? {
                    self.bios_call(code)?;
                }
                // Typical cartridge instruction fetch without sequential/wait-state accounting.
                4
            }
            .min(remaining);
            self.memory.tick(step);
            remaining -= step;
        }
        Ok(())
    }

    fn waiting(&mut self) -> bool {
        if self.memory.halted && self.memory.pending() == 0 {
            return true;
        }
        self.memory.halted = false;
        if self.cpu.in_irq() {
            return false;
        }
        if let Some(mask) = self.bios_wait {
            let flags = self.memory.read16(0x0300_7ff8);
            // IntrWait runs IRQ handlers while the BIOS is sleeping. A handler
            // acknowledges IF and ORs the handled interrupts into the BIOS flag.
            if flags & mask != 0 {
                self.memory.write16(0x0300_7ff8, flags & !mask);
                self.bios_wait = None;
            } else {
                return true;
            }
        }
        false
    }

    fn bios_call(&mut self, code: u8) -> Result<(), Error> {
        match code {
            0x00 => {
                // SoftReset clears the BIOS work area. The reset selector lies within it.
                let ram_entry = self.memory.read8(0x0300_7ffa) != 0;
                for addr in (0x0300_7e00..0x0300_8000).step_by(4) {
                    self.memory.write32(addr, 0);
                }
                self.cpu = Cpu::default();
                let entry = if ram_entry { 0x0200_0000 } else { 0x0800_0000 };
                self.cpu.regs[14] = entry;
                self.cpu.regs[15] = entry;
                self.bios_wait = None;
            }
            0x01 => self.memory.reset_ram(self.cpu.regs[0]),
            0x02 | 0x03 => self.memory.halted = true,
            0x04 | 0x05 => {
                let mask = if code == 5 {
                    1
                } else {
                    self.cpu.regs[1] as u16
                };
                if code == 5 || self.cpu.regs[0] != 0 {
                    let flags = self.memory.read16(0x0300_7ff8);
                    self.memory.write16(0x0300_7ff8, flags & !mask);
                }
                self.memory.write16(0x0400_0208, 1);
                self.bios_wait = Some(mask);
            }
            0x06 | 0x07 => {
                let (mut dividend, mut divisor) =
                    (self.cpu.regs[0] as i32, self.cpu.regs[1] as i32);
                if code == 7 {
                    std::mem::swap(&mut dividend, &mut divisor);
                }
                let (quotient, remainder) = if divisor == 0 {
                    (if dividend < 0 { -1 } else { 1 }, dividend)
                } else {
                    (
                        dividend.wrapping_div(divisor),
                        dividend.wrapping_rem(divisor),
                    )
                };
                self.cpu.regs[0] = quotient as u32;
                self.cpu.regs[1] = remainder as u32;
                self.cpu.regs[3] = quotient.unsigned_abs();
            }
            0x08 => self.cpu.regs[0] = self.cpu.regs[0].isqrt(),
            0x09 => self.arc_tan(),
            0x0a => self.arc_tan2(),
            0x0b | 0x0c => self.bios_copy(code == 0x0c),
            0x0e => self.bg_affine_set(),
            0x0f => self.obj_affine_set(),
            0x11 | 0x12 | 0x14 | 0x15 => {
                compression::decode(&mut self.memory, code, self.cpu.regs[0], self.cpu.regs[1])?;
            }
            _ => return Err(Error::UnsupportedBiosCall(code)),
        }
        Ok(())
    }

    fn bios_copy(&mut self, fast: bool) {
        let control = self.cpu.regs[2];
        let mut count = control & 0x1f_ffff;
        if fast {
            count = count.next_multiple_of(8);
        }
        let width = if fast || control & (1 << 26) != 0 {
            4
        } else {
            2
        };
        let mut src = self.cpu.regs[0] & !(width - 1);
        let mut dst = self.cpu.regs[1] & !(width - 1);
        for _ in 0..count {
            if width == 4 {
                self.memory.write32(dst, self.memory.read32(src));
            } else {
                self.memory.write16(dst, self.memory.read16(src));
            }
            dst = dst.wrapping_add(width);
            if control & (1 << 24) == 0 {
                src = src.wrapping_add(width);
            }
        }
    }
}

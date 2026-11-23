//! A Game Boy Advance core. The host supplies firmware, cartridge bytes, pressed
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
/// Size in bytes of the GBA boot ROM mapped at address zero.
pub const BIOS_LEN: usize = 16 * 1024;
pub const FRAME_RATE: f64 = 16_777_216.0 / (SCANLINE as f64 * LINES as f64);

/// Guest instruction encoding at the point of a CPU fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionSet {
    Arm,
    Thumb,
}

#[derive(Debug)]
pub enum Error {
    InvalidBiosSize(usize),
    BlankBios,
    InvalidRomSize(usize),
    InvalidSramSize(usize),
    Cpu(String),
    InvalidCompressedData(&'static str),
    UnsupportedBiosCall {
        code: u8,
        address: u32,
        instruction_set: InstructionSet,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBiosSize(size) => write!(
                formatter,
                "Invalid GBA BIOS length ({size} bytes, expected {BIOS_LEN} bytes)"
            ),
            Self::BlankBios => formatter.write_str("The GBA BIOS image is blank"),
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
            Self::UnsupportedBiosCall {
                code,
                address,
                instruction_set,
            } => {
                let mode = match instruction_set {
                    InstructionSet::Arm => "ARM",
                    InstructionSet::Thumb => "Thumb",
                };
                write!(
                    formatter,
                    "Unsupported GBA BIOS call 0x{code:02x} at 0x{address:08x} ({mode})"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// Checks the image size and rejects erased firmware without requiring a particular vendor dump.
pub fn validate_bios(bios: &[u8]) -> Result<(), Error> {
    if bios.len() != BIOS_LEN {
        return Err(Error::InvalidBiosSize(bios.len()));
    }
    if matches!(bios[0], 0 | 0xff) && bios.iter().all(|&byte| byte == bios[0]) {
        return Err(Error::BlankBios);
    }
    Ok(())
}

/// One cartridge session, including CPU, memory, LCD state, keypad and save RAM.
pub struct Machine {
    cpu: Cpu,
    memory: Memory,
    bios_wait: Option<u16>,
}

impl Machine {
    /// Starts at the ARM reset vector with the supplied 16 KiB BIOS mapped at address zero.
    pub fn with_bios(rom: Vec<u8>, bios: Vec<u8>) -> Result<Self, Error> {
        validate_bios(&bios)?;
        let mut machine = Self::from_rom(rom)?;
        machine.memory.install_bios(bios.into_boxed_slice());
        machine.cpu = Cpu::power_on();
        Ok(machine)
    }

    /// Boots at the cartridge entry point with emulated BIOS services for firmware-free tests.
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

    /// Reboots through the same firmware or direct-entry mode and retains battery-backed SRAM.
    pub fn reset(&mut self) {
        self.cpu = if self.memory.has_bios() {
            Cpu::power_on()
        } else {
            Cpu::default()
        };
        self.memory.reset();
        self.bios_wait = None;
    }

    /// Packed RGB888 rows in display order, updated by each completed scanline.
    pub fn pixels(&self) -> &[u8] {
        &self.memory.video.pixels
    }

    /// Whether the CPU has fetched a Game Pak ROM instruction since the last reset.
    pub fn cartridge_started(&self) -> bool {
        self.memory.cartridge_started()
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
                if self.memory.has_bios() {
                    self.cpu.hardware_irq();
                } else {
                    self.cpu.service_irq(&mut self.memory);
                }
            }
            let step = if self.waiting() {
                self.memory.cycles_to_event()
            } else {
                if let Step::Swi(code) = self.cpu.step(&mut self.memory).map_err(Error::Cpu)? {
                    if self.memory.has_bios() {
                        self.cpu.software_interrupt();
                    } else {
                        self.bios_call(code)?;
                    }
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
            0x10 => {
                let [src, dst, info, ..] = self.cpu.regs;
                (self.cpu.regs[0], self.cpu.regs[1]) =
                    compression::bit_unpack(&mut self.memory, src, dst, info)?;
            }
            0x11 | 0x12 | 0x14 | 0x15 => {
                let format = if code <= 0x12 {
                    compression::Format::Lz77
                } else {
                    compression::Format::RunLength
                };
                compression::decode(
                    &mut self.memory,
                    format,
                    matches!(code, 0x12 | 0x15),
                    self.cpu.regs[0],
                    self.cpu.regs[1],
                )?;
            }
            _ => {
                let (address, instruction_set) = self.cpu.swi_location();
                return Err(Error::UnsupportedBiosCall {
                    code,
                    address,
                    instruction_set,
                });
            }
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

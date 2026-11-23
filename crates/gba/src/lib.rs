//! A Game Boy Advance core with built-in BIOS services. The host supplies cartridge
//! bytes, pressed keys and frame pacing, and owns persistence of the cartridge SRAM.
mod bios_affine;
mod bios_math;
mod bus;
mod compression;
mod cpu;
pub mod debug;
mod hle;
mod video;

use std::fmt;

use bus::{LINES, Memory, SCANLINE};
use cpu::{Bus, Cpu, Step};

/// Whether this build includes a GPU compositor. Adapter availability is checked at activation.
pub const GPU_COMPILED: bool = cfg!(feature = "gpu");
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

    /// Boots at the cartridge entry point and handles BIOS calls and IRQ dispatch in Rust.
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

    /// Switches LCD color composition on this machine without rebooting the cartridge.
    pub fn set_hardware_acceleration(&mut self, enabled: bool) -> Result<(), String> {
        self.memory.video.set_hardware_acceleration(enabled)
    }

    /// The graphics API and adapter currently composing the LCD image.
    pub fn graphics_device(&self) -> Option<&str> {
        self.memory.video.graphics_device()
    }

    /// A device failure returns composition to software and preserves the affected frame.
    pub fn take_graphics_error(&mut self) -> Option<String> {
        self.memory.video.graphics_error.take()
    }

    /// Packed RGB888 rows in display order. GPU composition publishes at the end of a visible field.
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
            remaining -= self.advance(remaining)?;
        }
        Ok(())
    }

    /// Executes one CPU instruction, or advances to the next hardware event while sleeping.
    pub fn step(&mut self) -> Result<(), Error> {
        self.advance(u32::MAX).map(|_| ())
    }

    fn advance(&mut self, budget: u32) -> Result<u32, Error> {
        if self.memory.irq_pending() {
            if self.memory.has_bios() {
                self.cpu.hardware_irq();
            } else {
                self.cpu.service_irq(&mut self.memory);
            }
        }
        let cycles = if self.waiting() {
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
        .min(budget);
        self.memory.tick(cycles);
        Ok(cycles)
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
}

//! A Game Boy Advance core with built-in BIOS services. The host supplies cartridge
//! bytes, pressed keys and frame pacing, and owns persistence of the cartridge save chip.
mod audio;
mod backup;
mod bios_affine;
mod bios_math;
mod bus;
mod compression;
use eidoryn_arm as cpu;
pub mod debug;
mod hle;
mod rtc;
mod state;
mod video;

use std::fmt;

pub use audio::AUDIO_SAMPLE_RATE;
use bus::{LINES, Memory, SCANLINE};
use cpu::{Bus, Cpu, Step};
use sha2::{Digest, Sha256};
pub use state::{MAX_STATE_BYTES, StateError};

/// Whether this build includes a GPU compositor. Adapter availability is checked at activation.
pub const GPU_COMPILED: bool = cfg!(feature = "gpu");
pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 160;
pub const HEADER_LEN: usize = 0xc0;
pub const MAX_BATTERY_LEN: usize = backup::MAX_SIZE;
/// Bytes in a GBA RTC sidecar (clock offset, status, and weekday offset).
pub const RTC_BATTERY_LEN: usize = 10;
pub const MAX_ROM_LEN: usize = 32 * 1024 * 1024;
/// Size in bytes of the GBA boot ROM mapped at address zero.
pub const BIOS_LEN: usize = 16 * 1024;
pub const FRAME_RATE: f64 = 16_777_216.0 / (SCANLINE as f64 * LINES as f64);

pub use eidoryn_arm::InstructionSet;

#[derive(Debug)]
pub enum Error {
    InvalidBiosSize(usize),
    BlankBios,
    InvalidRomSize(usize),
    InvalidSramSize(usize),
    InvalidRtcBattery,
    InvalidBatterySize {
        size: usize,
        expected: &'static str,
    },
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
            Self::InvalidRtcBattery => formatter.write_str("Invalid GBA RTC battery state"),
            Self::InvalidSramSize(size) => write!(
                formatter,
                "Invalid GBA SRAM length ({size} bytes, expected 32 KiB)"
            ),
            Self::InvalidBatterySize { size, expected } => write!(
                formatter,
                "Invalid GBA battery save length ({size} bytes, expected {expected})"
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
    rom_digest: [u8; 32],
    bios_digest: Option<[u8; 32]>,
    cpu: Cpu,
    memory: Memory,
    bios_wait: Option<u16>,
}

fn direct_boot_cpu() -> Cpu {
    Cpu::direct_boot(0x0800_0000, 0x0300_7f00, 0x0300_7fa0, 0x0300_7fe0)
}

impl Machine {
    /// Starts at the ARM reset vector with the supplied 16 KiB BIOS mapped at address zero.
    pub fn with_bios(rom: Vec<u8>, bios: Vec<u8>) -> Result<Self, Error> {
        validate_bios(&bios)?;
        let mut machine = Self::from_rom(rom)?;
        machine.bios_digest = Some(Sha256::digest(&bios).into());
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
            rom_digest: Sha256::digest(&rom).into(),
            bios_digest: None,
            cpu: direct_boot_cpu(),
            memory: Memory::new(rom),
            bios_wait: None,
        })
    }

    /// Reboots through the same firmware or direct-entry mode and retains battery save data.
    pub fn reset(&mut self) {
        self.cpu = if self.memory.has_bios() {
            Cpu::power_on()
        } else {
            direct_boot_cpu()
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

    /// Drains interleaved left/right PCM frames at [`AUDIO_SAMPLE_RATE`] from this timeline.
    pub fn take_audio_samples(&mut self) -> Vec<[i16; 2]> {
        self.memory.audio.drain()
    }

    /// SHA-256 of the loaded cartridge, shared by snapshot checks and host storage paths.
    pub fn rom_digest(&self) -> &[u8; 32] {
        &self.rom_digest
    }

    /// Whether the CPU has fetched a Game Pak ROM instruction since the last reset.
    pub fn cartridge_started(&self) -> bool {
        self.memory.cartridge_started()
    }

    /// Pressed bits from low to high: A, B, Select, Start, Right, Left, Up, Down, R, L.
    pub fn set_buttons(&mut self, pressed: u16) {
        self.memory.set_buttons(pressed);
    }

    /// Raw, headerless battery image in the currently loaded cartridge's format.
    pub fn battery_save(&self) -> &[u8] {
        self.memory.backup.bytes()
    }

    /// Checks the file size against the cartridge chip; EEPROM supports two capacities.
    pub fn validate_battery_size(&self, size: usize) -> Result<(), Error> {
        self.memory.backup.validate_size(size)
    }

    /// Installs a complete battery image and resets the chip's command interface.
    pub fn restore_battery_save(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.memory.backup.restore(bytes)
    }

    /// Reports writes since the previous poll or successful restore.
    pub fn take_battery_dirty(&mut self) -> bool {
        self.memory.backup.take_dirty()
    }

    /// Battery-backed clock settings, separate from the interoperable raw game save.
    pub fn rtc_battery(&self) -> Option<[u8; RTC_BATTERY_LEN]> {
        self.memory.rtc.as_ref().map(rtc::Rtc::battery)
    }

    /// Restore an RTC sidecar from the same cartridge before executing it.
    pub fn restore_rtc_battery(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.memory
            .rtc
            .as_mut()
            .ok_or(Error::InvalidRtcBattery)?
            .restore_battery(bytes)
    }

    /// Compatibility accessor for hosts using the original SRAM-only interface.
    pub fn sram(&self) -> &[u8] {
        self.battery_save()
    }

    /// Compatibility loader for hosts using the original SRAM-only interface.
    pub fn restore_sram(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.restore_battery_save(bytes)
    }

    /// Compatibility notification for hosts using the original SRAM-only interface.
    pub fn take_sram_dirty(&mut self) -> bool {
        self.take_battery_dirty()
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
                self.cpu.service_irq(&mut self.memory, 0x0300_7ffc);
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

//! A BIOS-less Nintendo DS direct-boot core. Both CPUs execute the cartridge's own programs.
mod audio;
mod bus;
mod devices;
mod display;
mod header;
mod hle;

use bincode::Options;
use bus::{Memory, Port};
use eidoryn_arm::{Bus, Cpu, Step};
pub use header::Header;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Native stereo DAC rate, rounded from the ARM7 clock divided by 1,024.
pub const AUDIO_SAMPLE_RATE: u32 = 32_728;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;
pub const WIDTH: usize = SCREEN_WIDTH;
pub const HEIGHT: usize = 2 * SCREEN_HEIGHT;
pub const FRAME_RATE: f64 = 33_513_982.0 / (2130.0 * 263.0);
pub const HEADER_LEN: usize = 0x200;
pub const MAX_ROM_LEN: usize = 512 * 1024 * 1024;
pub const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BACKUP_BYTES: usize = devices::backup::LARGE;
const LEGACY_STATE_MAGIC: &[u8; 8] = b"EIDNDS\x01\0";
type ExtendedState = (
    System,
    devices::backup::Backup,
    display::geometry::Geometry,
    devices::wifi::Wifi,
);
const EXTENDED_STATE_MAGIC: &[u8; 8] = b"EIDNDS\x02\0";
const STATE_MAGIC: &[u8; 8] = b"EIDNDS\x03\0";

#[derive(Debug)]
pub enum Error {
    InvalidRom(&'static str),
    Execution {
        processor: &'static str,
        pc: u32,
        reason: String,
    },
    InvalidState(String),
    InvalidBackupSize(usize),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRom(reason) => f.write_str(reason),
            Self::InvalidBackupSize(size) => write!(f, "Unsupported NDS EEPROM size: {size} bytes"),
            Self::Execution {
                processor,
                pc,
                reason,
            } => write!(f, "{processor} at {pc:#010x}: {reason}"),
            Self::InvalidState(s) => write!(f, "Invalid DS snapshot: {s}"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Serialize, Deserialize)]
struct System {
    cpus: [Cpu; 2],
    memory: Memory,
    waiting: [Option<u32>; 2],
    frames: u64,
}
impl System {
    fn boot(rom: &[u8], h: &Header) -> Self {
        let mut memory = Memory::default();
        let mut nine = Port::<true> {
            m: &mut memory,
            rom,
        };
        for (n, b) in rom[h.arm9.source.clone()].iter().enumerate() {
            nine.write8(h.arm9.load + n as u32, *b);
        }
        let mut seven = Port::<false> {
            m: &mut memory,
            rom,
        };
        for (n, b) in rom[h.arm7.source.clone()].iter().enumerate() {
            seven.write8(h.arm7.load + n as u32, *b);
        }
        for (i, b) in rom[..HEADER_LEN].iter().enumerate() {
            seven.write8(0x027f_fe00 + i as u32, *b);
        }
        let chip_id = devices::chip_id(rom);
        for base in [0x027f_f800, 0x027f_fc00] {
            seven.write32(base, chip_id);
            seven.write32(base + 4, chip_id);
            seven.write16(base + 8, u16::from_le_bytes([rom[0x15e], rom[0x15f]]));
            seven.write16(base + 10, u16::from_le_bytes([rom[0x6c], rom[0x6d]]));
        }
        seven.write16(0x027f_fc40, 1);
        for (offset, value) in devices::USER_SETTINGS.iter().enumerate() {
            seven.write8(0x027f_fc80 + offset as u32, *value);
        }
        let arm9 = Cpu::direct_boot(h.arm9.entry, 0x0300_2f7c, 0x0300_3f80, 0x0300_3fc0);
        let arm7 = Cpu::direct_boot(h.arm7.entry, 0x0380_fd80, 0x0380_ff80, 0x0380_ffc0);
        Self {
            cpus: [arm9, arm7],
            memory,
            waiting: [None; 2],
            frames: 0,
        }
    }
    fn clock<const NINE: bool>(&mut self, rom: &[u8]) -> Result<(), Error> {
        let i = usize::from(!NINE);
        let cpu = &mut self.cpus[i];
        let mut port = Port::<NINE> {
            m: &mut self.memory,
            rom,
        };
        port.dma();
        let irq = port.m.irq(i);
        if port.m.halted[i] {
            if irq == 0 {
                return Ok(());
            }
            port.m.halted[i] = false;
        }
        if irq != 0 && port.m.master_irq(i) {
            let vector = port.m.irq_vector(NINE);
            cpu.service_irq(&mut port, vector);
        }
        // IntrWait resumes only after the guest's IRQ handler records a matching
        // event in the BIOS flags word. The hardware IF register is independent.
        if let Some(mask) = self.waiting[i]
            && !cpu.in_irq()
        {
            let addr = port.m.bios_flags(NINE);
            let flags = port.read32(addr);
            if flags & mask == 0 {
                return Ok(());
            }
            port.write32(addr, flags & !mask);
            self.waiting[i] = None;
        }
        let pc = cpu.regs[15];
        let error = |reason| Error::Execution {
            processor: if NINE { "ARM9" } else { "ARM7" },
            pc,
            reason,
        };
        if let Step::Swi(op) = cpu.step(&mut port).map_err(error)? {
            hle::invoke(op, cpu, &mut port, &mut self.waiting[i]).map_err(error)?;
        }
        Ok(())
    }
}

/// Running DS cartridge and its two LCDs. Snapshots contain mutable machine state only.
pub struct Machine {
    rom: Vec<u8>,
    digest: [u8; 32],
    header: Header,
    system: System,
}
impl Machine {
    pub fn from_rom(rom: Vec<u8>) -> Result<Self, Error> {
        let header = Header::parse(&rom, rom.len() as u64)?;
        let system = System::boot(&rom, &header);
        let digest = Sha256::digest(&rom).into();
        Ok(Self {
            rom,
            digest,
            header,
            system,
        })
    }
    pub fn reset(&mut self) {
        let mut backup = std::mem::take(&mut self.system.memory.backup);
        backup.reset_bus();
        self.system = System::boot(&self.rom, &self.header);
        self.system.memory.backup = backup;
    }
    /// One scheduler quantum (two ARM9 instruction slots and one ARM7 slot).
    pub fn step(&mut self) -> Result<(), Error> {
        self.system.clock::<true>(&self.rom)?;
        self.system.clock::<true>(&self.rom)?;
        self.system.clock::<false>(&self.rom)?;
        if self.system.memory.advance(4) {
            self.system.frames = self.system.frames.wrapping_add(1);
        }
        Ok(())
    }
    pub fn run_frame(&mut self) -> Result<(), Error> {
        let frame = self.system.frames;
        while self.system.frames == frame {
            self.step()?;
        }
        Ok(())
    }
    /// Drain the pending stereo PCM in chronological order.
    pub fn take_audio_samples(&mut self) -> Vec<[i16; 2]> {
        self.system.memory.audio.take_samples()
    }
    pub fn set_buttons(&mut self, pressed: u16) {
        self.system.memory.set_keys(pressed);
    }
    pub fn set_touch(&mut self, pen: Option<(u8, u8)>) {
        self.system.memory.pen = pen.filter(|&(_, y)| usize::from(y) < SCREEN_HEIGHT);
    }
    pub fn pcs(&self) -> (u32, u32) {
        (self.system.cpus[0].regs[15], self.system.cpus[1].regs[15])
    }
    pub fn frame_count(&self) -> u64 {
        self.system.frames
    }
    pub fn pixels(&self) -> &[u8] {
        self.system.memory.display.pixels()
    }
    /// Contents of the in-game EEPROM once a battery image has been created or loaded.
    pub fn backup(&self) -> Option<&[u8]> {
        self.system.memory.backup.data()
    }

    /// Change counter for the host's battery-file writer.
    pub fn backup_revision(&self) -> u64 {
        self.system.memory.backup.revision()
    }

    /// Attach a raw 8 KiB or 64 KiB EEPROM image before running the cartridge.
    pub fn load_backup(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.system.memory.backup.import(bytes) {
            Ok(())
        } else {
            Err(Error::InvalidBackupSize(bytes.len()))
        }
    }

    pub fn rom_digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub fn save_state(&self) -> Result<Vec<u8>, Error> {
        let mut data = Vec::with_capacity(5 * 1024 * 1024);
        data.extend_from_slice(STATE_MAGIC);
        data.extend_from_slice(&self.digest);
        data.extend_from_slice(&[0; 4]);
        codec()
            .serialize_into(
                &mut data,
                &(
                    (
                        &self.system,
                        &self.system.memory.backup,
                        &self.system.memory.display.geometry,
                        &self.system.memory.wifi,
                    ),
                    &self.system.memory.audio,
                ),
            )
            .map_err(|e| Error::InvalidState(e.to_string()))?;
        let checksum = crc32fast::hash(&data[44..]);
        data[40..44].copy_from_slice(&checksum.to_le_bytes());
        Ok(data)
    }
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), Error> {
        if data.len() < 44
            || data.len() > MAX_STATE_BYTES
            || ![STATE_MAGIC, EXTENDED_STATE_MAGIC, LEGACY_STATE_MAGIC]
                .iter()
                .any(|magic| data.starts_with(*magic))
        {
            return Err(Error::InvalidState("unknown format or length".into()));
        }
        if data[8..40] != self.digest {
            return Err(Error::InvalidState("belongs to another cartridge".into()));
        }
        if crc32fast::hash(&data[44..]).to_le_bytes() != data[40..44] {
            return Err(Error::InvalidState("checksum mismatch".into()));
        }
        let (mut state, backup) = if &data[..8] == LEGACY_STATE_MAGIC {
            codec().deserialize(&data[44..]).map(|s| (s, None))
        } else {
            let decoded = if data.starts_with(EXTENDED_STATE_MAGIC) {
                codec()
                    .deserialize::<ExtendedState>(&data[44..])
                    .map(|s| (s, audio::Audio::default()))
            } else {
                codec().deserialize::<(ExtendedState, audio::Audio)>(&data[44..])
            };
            decoded.map(|((mut state, backup, geometry, wifi), audio)| {
                state.memory.wifi = wifi;
                state.memory.display.geometry = geometry;
                state.memory.audio = audio;
                (state, Some(backup))
            })
        }
        .map_err(|e| Error::InvalidState(e.to_string()))?;
        if !state.memory.valid() {
            return Err(Error::InvalidState("invalid device state".into()));
        }
        if let Some(mut backup) = backup {
            if !backup.valid() {
                return Err(Error::InvalidState("invalid backup device".into()));
            }
            backup.restored(&self.system.memory.backup);
            state.memory.backup = backup;
        } else {
            state.memory.backup = std::mem::take(&mut self.system.memory.backup);
        }
        self.system = state;
        Ok(())
    }
}
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit((MAX_STATE_BYTES - 44) as u64)
        .reject_trailing_bytes()
}
#[cfg(test)]
mod tests;

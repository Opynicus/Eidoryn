//! Versioned machine images shared by on-disk snapshots and in-memory rewind.
use std::fmt;

use bincode::Options;
use serde::{Deserialize, Serialize};

use crate::bus::Memory;
use crate::cpu::Cpu;
use crate::{HEIGHT, Machine, WIDTH};

const MAGIC: [u8; 8] = *b"EIDOGBA\0";
const VERSION: u32 = 1;
const PIXEL_BYTES: usize = WIDTH * HEIGHT * 3;
/// Maximum supported encoded size, including RAM, the LCD image and cartridge backup.
pub const MAX_STATE_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Header {
    magic: [u8; 8],
    version: u32,
    rom: [u8; 32],
    bios: Option<[u8; 32]>,
}

/// Reason a machine image could not be saved or restored.
#[derive(Debug)]
pub enum StateError {
    Invalid,
    UnsupportedVersion(u32),
    CartridgeMismatch,
    FirmwareMismatch,
    Codec(bincode::Error),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => f.write_str("Invalid or corrupt GBA snapshot"),
            Self::UnsupportedVersion(version) => {
                write!(f, "Unsupported GBA snapshot version {version}")
            }
            Self::CartridgeMismatch => f.write_str("GBA snapshot belongs to a different cartridge"),
            Self::FirmwareMismatch => {
                f.write_str("GBA snapshot requires the firmware it was saved with")
            }
            Self::Codec(error) => write!(f, "GBA snapshot encoding error: {error}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            _ => None,
        }
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_STATE_BYTES as u64)
        .reject_trailing_bytes()
}

impl Machine {
    /// Captures guest state at the current instruction and scanline, including cartridge backup.
    pub fn save_state(&self) -> Result<Vec<u8>, StateError> {
        let header = Header {
            magic: MAGIC,
            version: VERSION,
            rom: self.rom_digest,
            bios: self.bios_digest,
        };
        let state = (header, &self.cpu, self.bios_wait, &self.memory);
        let encoded_size = codec().serialized_size(&state).map_err(StateError::Codec)?;
        let size = encoded_size as usize + PIXEL_BYTES + 4;
        if size > MAX_STATE_BYTES {
            return Err(StateError::Invalid);
        }
        let mut bytes = Vec::with_capacity(size);
        codec()
            .serialize_into(&mut bytes, &state)
            .map_err(StateError::Codec)?;
        self.memory.video.append_snapshot_pixels(&mut bytes);
        bytes.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
        Ok(bytes)
    }

    /// Restores a matching image atomically. The live renderer and host cartridge remain attached.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(StateError::Invalid);
        }
        let (body, checksum) = bytes.split_last_chunk::<4>().ok_or(StateError::Invalid)?;
        if crc32fast::hash(body) != u32::from_le_bytes(*checksum) {
            return Err(StateError::Invalid);
        }
        let (mut encoded, pixels) = body
            .split_last_chunk::<PIXEL_BYTES>()
            .ok_or(StateError::Invalid)?;
        let header: Header = codec()
            .deserialize_from(&mut encoded)
            .map_err(StateError::Codec)?;
        if header.magic != MAGIC {
            return Err(StateError::Invalid);
        }
        if header.version != VERSION {
            return Err(StateError::UnsupportedVersion(header.version));
        }
        if header.rom != self.rom_digest {
            return Err(StateError::CartridgeMismatch);
        }
        if header.bios != self.bios_digest {
            return Err(StateError::FirmwareMismatch);
        }
        let (cpu, bios_wait, mut memory): (Cpu, Option<u16>, Memory) =
            codec().deserialize(encoded).map_err(StateError::Codec)?;
        if !memory.valid_snapshot_for(&self.memory) {
            return Err(StateError::Invalid);
        }
        memory.video.pixels = std::mem::take(&mut self.memory.video.pixels);
        memory.video.pixels.copy_from_slice(pixels);
        memory.attach_session(&mut self.memory);
        self.memory = memory;
        self.cpu = cpu;
        self.bios_wait = bios_wait;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Bus;
    use crate::{BIOS_LEN, HEADER_LEN};

    fn cartridge() -> Vec<u8> {
        let mut rom = vec![0; HEADER_LEN];
        rom[..4].copy_from_slice(&0xe280_0001_u32.to_le_bytes()); // add r0, r0, #1
        rom[4..8].copy_from_slice(&0xeaff_fffd_u32.to_le_bytes()); // b 0x08000000
        rom
    }

    #[test]
    fn replay_resumes_timer_dma_keypad_cpu_and_lcd_at_the_same_scanline() {
        let mut machine = Machine::from_rom(cartridge()).unwrap();
        machine.memory.write16(0x0400_0000, 3 | (1 << 10)); // bitmap BG2
        machine.memory.write16(0x0400_0100, 0xfff0); // prescaled timer, overflow IRQ
        machine.memory.write16(0x0400_0102, 0xc1);
        machine.memory.write16(0x0200_0000, 0x001f);
        machine.memory.write32(0x0400_00d4, 0x0200_0000); // DMA3 on the next HBlank
        machine
            .memory
            .write32(0x0400_00d8, 0x0600_0000 + (WIDTH * 2) as u32);
        machine.memory.write32(0x0400_00dc, 0xa000_0001);
        machine.set_buttons(1 << 5);
        for _ in 0..235 {
            machine.step().unwrap();
        }
        let saved = machine.save_state().unwrap();
        let at_save = machine.pixels().to_owned();
        let registers = machine.debug_snapshot().registers;
        machine.run_frame().unwrap();
        let expected = machine.save_state().unwrap();
        assert_eq!(&machine.pixels()[WIDTH * 3..][..3], &[255, 0, 0]);
        assert_eq!(machine.memory.read16(0x0400_0202) & 8, 8);
        assert_ne!(expected, saved);

        machine.reset();
        machine.load_state(&saved).unwrap();
        assert_eq!(machine.pixels(), at_save);
        assert_eq!(machine.debug_snapshot().registers, registers);
        assert_eq!(machine.memory.read16(0x0400_0130), 0x3df);
        machine.run_frame().unwrap();
        assert_eq!(machine.save_state().unwrap(), expected);
    }

    #[test]
    fn corrupt_or_incompatible_snapshots_leave_the_current_session_intact() {
        let mut live = Machine::from_rom(cartridge()).unwrap();
        live.step().unwrap();
        let original = live.save_state().unwrap();
        let mut corrupt = original.clone();
        corrupt[PIXEL_BYTES] ^= 1;
        assert!(matches!(
            live.load_state(&corrupt),
            Err(StateError::Invalid)
        ));
        assert!(live.load_state(&original[..original.len() - 1]).is_err());
        let mut future = original[..original.len() - 4].to_vec();
        future[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        future.extend_from_slice(&crc32fast::hash(&future).to_le_bytes());
        assert!(matches!(
            live.load_state(&future),
            Err(StateError::UnsupportedVersion(_))
        ));

        let mut other = cartridge();
        other[16] = 1; // same header identity, different cartridge bytes
        let other = Machine::from_rom(other).unwrap().save_state().unwrap();
        assert!(matches!(
            live.load_state(&other),
            Err(StateError::CartridgeMismatch)
        ));
        let firmware = Machine::with_bios(cartridge(), vec![7; BIOS_LEN])
            .unwrap()
            .save_state()
            .unwrap();
        assert!(matches!(
            live.load_state(&firmware),
            Err(StateError::FirmwareMismatch)
        ));

        // Even correctly checksummed but invalid machine data must be transactional.
        let mut malformed = Machine::from_rom(cartridge()).unwrap();
        malformed.memory.video.oam = Box::new([]);
        assert!(matches!(
            live.load_state(&malformed.save_state().unwrap()),
            Err(StateError::Invalid)
        ));
        assert_eq!(live.save_state().unwrap(), original);
    }

    #[test]
    fn restoring_an_older_backup_marks_the_reverted_bytes_for_disk_sync() {
        let mut machine = Machine::from_rom(cartridge()).unwrap();
        machine.bios_wait = Some(1);
        let before = machine.save_state().unwrap();
        machine.bios_wait = None;
        machine.memory.write8(0x0e00_0009, 0xaf);
        assert!(machine.take_battery_dirty());
        machine.load_state(&before).unwrap();
        assert_eq!(machine.bios_wait, Some(1));
        assert_eq!(machine.battery_save()[9], 0xff);
        assert!(machine.take_battery_dirty());
        assert!(!machine.take_battery_dirty());
        machine.load_state(&before).unwrap();
        assert!(!machine.take_battery_dirty());
    }

    #[test]
    fn halted_debugger_steps_replay_across_timer_stop_restart_and_cascade_changes() {
        let mut live = Machine::from_rom(cartridge()).unwrap();
        let mut restored = Machine::from_rom(cartridge()).unwrap();
        live.memory.write32(0x0400_0100, 0x00c3_ffff);
        live.memory.write32(0x0400_0104, 0x00c4_ffff);
        live.bios_call(0x02).unwrap(); // Halt with both timers armed
        live.step().unwrap(); // HBlank, with 1006/1024 cycles in timer zero
        assert!(live.memory.halted);
        for (address, control) in [
            (0x0400_0102, 3),    // stop without changing prescaler
            (0x0400_0102, 1),    // change prescaler while stopped
            (0x0400_0102, 0xc1), // restart
            (0x0400_0102, 0xc0), // shorten an active timer period to one cycle
            (0x0400_0106, 0xc3), // leave cascade mode
            (0x0400_0106, 0xc4), // reenter cascade mode
        ] {
            live.memory.write16(address, control);
            restored.load_state(&live.save_state().unwrap()).unwrap();
            live.step().unwrap();
            restored.step().unwrap();
            assert_eq!(live.save_state().unwrap(), restored.save_state().unwrap());
        }
    }
}

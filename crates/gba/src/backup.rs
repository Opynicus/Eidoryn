//! Cartridge save chips. Parallel flash commands and serial EEPROM transfers use
//! the guest bus, while the host reads/writes standard, headerless battery images.
use crate::Error;
use serde::{Deserialize, Serialize};

pub(crate) const MAX_SIZE: usize = 128 * 1024;
const FLASH_BANK: usize = 0x1_0000;
const EEPROM_WRITE_CYCLES: u32 = 108_368;

#[derive(Serialize, Deserialize)]
pub(super) struct Backup {
    chip: Chip,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    #[serde(skip)]
    dirty: bool,
}

#[derive(Serialize, Deserialize)]
enum Chip {
    Sram,
    Flash(Flash),
    Eeprom(Eeprom),
}

#[derive(Default, Serialize, Deserialize)]
struct Flash {
    command: FlashCommand,
    identification: bool,
    bank: u8,
}

#[derive(Default, Serialize, Deserialize)]
enum FlashCommand {
    #[default]
    Idle,
    Unlock,
    Command,
    Program,
    Bank,
    EraseUnlock,
    EraseConfirm,
    Erase,
}

#[derive(Default, Serialize, Deserialize)]
struct Eeprom {
    output: u64,
    bits_left: u8,
    busy_cycles: u32,
}

impl Backup {
    pub(super) fn detect(rom: &[u8]) -> Self {
        let has = |tag: &[u8]| {
            memchr::memmem::find_iter(rom, tag).any(|offset| {
                rom.get(offset + tag.len()..)
                    .and_then(|suffix| suffix.first_chunk::<3>())
                    .is_some_and(|version| version.iter().all(u8::is_ascii_digit))
            })
        };
        let (chip, size) = if has(b"FLASH1M_V") {
            (Chip::Flash(Flash::default()), MAX_SIZE)
        } else if has(b"FLASH512_V") || has(b"FLASH_V") {
            (Chip::Flash(Flash::default()), FLASH_BANK)
        } else if has(b"EEPROM_V") {
            (Chip::Eeprom(Eeprom::default()), 512)
        } else {
            // SRAM and FRAM are byte-addressed, including unmarked homebrew.
            (Chip::Sram, 32 * 1024)
        };
        Self {
            chip,
            bytes: vec![0xff; size],
            dirty: false,
        }
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(super) fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub(super) fn valid_for(&self, live: &Self) -> bool {
        match (&self.chip, &live.chip) {
            (Chip::Sram, Chip::Sram) => self.bytes.len() == live.bytes.len(),
            (Chip::Flash(flash), Chip::Flash(_)) => {
                self.bytes.len() == live.bytes.len()
                    && usize::from(flash.bank) < self.bytes.len() / FLASH_BANK
            }
            (Chip::Eeprom(eeprom), Chip::Eeprom(_)) => {
                matches!(self.bytes.len(), 512 | 8192)
                    && eeprom.bits_left <= 68
                    && eeprom.busy_cycles <= EEPROM_WRITE_CYCLES
            }
            _ => false,
        }
    }

    pub(super) fn reconcile_dirty(&mut self, live: &Self) {
        self.dirty = live.dirty || self.bytes != live.bytes;
    }

    pub(super) fn validate_size(&self, size: usize) -> Result<(), Error> {
        match self.chip {
            Chip::Eeprom(_) if !matches!(size, 512 | 8192) => Err(Error::InvalidBatterySize {
                size,
                expected: "512 or 8192 EEPROM bytes",
            }),
            Chip::Sram if self.bytes.len() != size => Err(Error::InvalidSramSize(size)),
            Chip::Flash(_) if self.bytes.len() != size => Err(Error::InvalidBatterySize {
                size,
                expected: if self.bytes.len() == FLASH_BANK {
                    "65536 flash bytes"
                } else {
                    "131072 flash bytes"
                },
            }),
            _ => Ok(()),
        }
    }

    pub(super) fn restore(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.validate_size(bytes.len())?;
        self.bytes.clear();
        self.bytes.extend_from_slice(bytes);
        self.reset_protocol();
        self.dirty = false;
        Ok(())
    }

    pub(super) fn reset_protocol(&mut self) {
        match &mut self.chip {
            Chip::Sram => {}
            Chip::Flash(flash) => *flash = Flash::default(),
            Chip::Eeprom(eeprom) => *eeprom = Eeprom::default(),
        }
    }

    pub(super) fn read_parallel(&self, address: u16) -> u8 {
        let offset = usize::from(address);
        match &self.chip {
            Chip::Sram => self.bytes[offset & 0x7fff],
            Chip::Flash(flash) if flash.identification && offset < 2 => {
                let id = if self.bytes.len() == FLASH_BANK {
                    [0x32, 0x1b]
                } else {
                    [0x62, 0x13]
                };
                id[offset]
            }
            Chip::Flash(flash) => self.bytes[usize::from(flash.bank) * FLASH_BANK + offset],
            Chip::Eeprom(_) => 0xff,
        }
    }

    pub(super) fn write_parallel(&mut self, address: u16, value: u8) {
        self.dirty |= match &mut self.chip {
            Chip::Sram => {
                std::mem::replace(&mut self.bytes[usize::from(address) & 0x7fff], value) != value
            }
            Chip::Flash(flash) => flash.write(&mut self.bytes, address, value),
            Chip::Eeprom(_) => false,
        };
    }

    pub(super) fn is_eeprom(&self) -> bool {
        matches!(self.chip, Chip::Eeprom(_))
    }

    pub(super) fn tick(&mut self, cycles: u32) {
        if let Chip::Eeprom(eeprom) = &mut self.chip {
            eeprom.busy_cycles = eeprom.busy_cycles.saturating_sub(cycles);
        }
    }

    /// DMA3 transfers a whole command, one low data bit per halfword. The same
    /// Nintendo library signature covers both chips; command length identifies the address width.
    pub(super) fn eeprom_command(&mut self, bits: &[u8]) {
        let Chip::Eeprom(eeprom) = &mut self.chip else {
            return;
        };
        let (address_bits, read) = match bits.len() {
            9 => (6, true),
            17 => (14, true),
            73 => (6, false),
            81 => (14, false),
            _ => return,
        };
        if bits[0] != 1
            || bits[1] != u8::from(read)
            || bits.last() != Some(&0)
            || eeprom.busy_cycles != 0
        {
            return;
        }
        let address = bits[2..address_bits + 2]
            .iter()
            .fold(0usize, |acc, &bit| acc << 1 | usize::from(bit));
        if address_bits == 14 && self.bytes.len() == 512 {
            self.bytes.resize(8192, 0xff);
            self.dirty = true;
        }
        let start = (address & 0x3ff) * 8;
        let block = &mut self.bytes[start..start + 8];
        if read {
            eeprom.output = u64::from_be_bytes(block.try_into().expect("8-byte EEPROM block"));
            eeprom.bits_left = 68;
        } else {
            let data = bits[address_bits + 2..bits.len() - 1]
                .iter()
                .fold(0u64, |acc, &bit| acc << 1 | u64::from(bit))
                .to_be_bytes();
            self.dirty |= block != data;
            block.copy_from_slice(&data);
            eeprom.bits_left = 0;
            eeprom.busy_cycles = EEPROM_WRITE_CYCLES;
        }
    }

    pub(super) fn peek_eeprom_bit(&self) -> u16 {
        let Chip::Eeprom(eeprom) = &self.chip else {
            return 1;
        };
        match eeprom.bits_left {
            0 => u16::from(eeprom.busy_cycles == 0),
            1..=64 => ((eeprom.output >> (eeprom.bits_left - 1)) & 1) as u16,
            _ => 0,
        }
    }

    pub(super) fn read_eeprom_bit(&mut self) -> u16 {
        let bit = self.peek_eeprom_bit();
        if let Chip::Eeprom(eeprom) = &mut self.chip {
            eeprom.bits_left = eeprom.bits_left.saturating_sub(1);
        }
        bit
    }
}

impl Flash {
    fn write(&mut self, bytes: &mut [u8], address: u16, value: u8) -> bool {
        use FlashCommand::{
            Bank, Command, Erase, EraseConfirm, EraseUnlock, Idle, Program, Unlock,
        };
        let offset = usize::from(self.bank) * FLASH_BANK + usize::from(address);
        self.command = match (&self.command, address, value) {
            (Program, _, data) => {
                self.command = Idle;
                let old = bytes[offset];
                bytes[offset] &= data; // Programming can only clear bits until an erase.
                return old != bytes[offset];
            }
            (Bank, 0, bank) if bytes.len() > FLASH_BANK => {
                self.bank = bank & 1;
                Idle
            }
            (Erase, 0x5555, 0x10) => {
                self.command = Idle;
                return erase(bytes);
            }
            (Erase, _, 0x30) => {
                self.command = Idle;
                let start = offset & !0xfff;
                return erase(&mut bytes[start..start + 4096]);
            }
            (EraseUnlock, 0x5555, 0xaa) => EraseConfirm,
            (EraseConfirm, 0x2aaa, 0x55) => Erase,
            (Unlock, 0x2aaa, 0x55) => Command,
            (Command, 0x5555, 0x90) => {
                self.identification = true;
                Idle
            }
            (Command, 0x5555, 0x80) => EraseUnlock,
            (Command, 0x5555, 0xa0) => Program,
            (Command, 0x5555, 0xb0) => Bank,
            (_, _, 0xf0) => {
                self.identification = false;
                Idle
            }
            (_, 0x5555, 0xaa) => Unlock,
            _ => Idle,
        };
        false
    }
}

fn erase(bytes: &mut [u8]) -> bool {
    let changed = bytes.iter().any(|&byte| byte != 0xff);
    bytes.fill(0xff);
    changed
}

#[cfg(test)]
mod tests;

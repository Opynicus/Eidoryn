//! ARM7-local wireless controller. The MAC's register and packet RAM tests also
//! run in single-player games, before the software selects a wireless mode.
use serde::{Deserialize, Serialize};

const PACKET_WORDS: usize = 0x1000;
const REGISTER_WORDS: usize = 0x180;
const BB_REGISTERS: usize = 0x69;

#[derive(Serialize, Deserialize)]
pub(crate) struct Wifi {
    packet: Vec<u16>,
    registers: Vec<u16>,
    #[serde(with = "serde_bytes")]
    baseband: Vec<u8>,
}

impl Default for Wifi {
    fn default() -> Self {
        let mut registers = vec![0; REGISTER_WORDS];
        registers[0x036 / 2] = 1;
        registers[0x038 / 2] = 3;
        registers[0x03c / 2] = 0x0200;
        registers[0x050 / 2] = 0x4000;
        registers[0x052 / 2] = 0x4800;
        registers[0x08c / 2] = 100;
        registers[0x160 / 2] = 0x0100;
        registers[0x168 / 2] = 0x800d;
        Self {
            packet: vec![0; PACKET_WORDS],
            registers,
            baseband: vec![0; BB_REGISTERS],
        }
    }
}

impl Wifi {
    pub(crate) fn valid(&self) -> bool {
        self.packet.len() == PACKET_WORDS
            && self.registers.len() == REGISTER_WORDS
            && self.baseband.len() == BB_REGISTERS
    }

    pub(crate) fn read(&self, addr: u32) -> u16 {
        match addr & 0x7ffe {
            0x4000..=0x5ffe => self.packet[(addr as usize & 0x1ffe) / 2],
            0x0000..=0x1ffe | 0x6000..=0x7ffe => match addr & 0xffe {
                0x000 => 0x1440, // Mitsumi original DS MAC.
                0x158 => self.registers[0x15c / 2],
                off => self.registers.get(off as usize / 2).copied().unwrap_or(0),
            },
            _ => 0,
        }
    }

    // The ARM7 IRQ is latched only on a rising edge of the enabled MAC flags.
    pub(crate) fn write(&mut self, addr: u32, value: u16) -> bool {
        let was_pending = self.interrupt_pending();
        match addr & 0x7ffe {
            0x4000..=0x5ffe => self.packet[(addr as usize & 0x1ffe) / 2] = value,
            0x0000..=0x1ffe | 0x6000..=0x7ffe => self.write_register(addr as usize & 0xffe, value),
            _ => {}
        }
        !was_pending && self.interrupt_pending()
    }

    fn interrupt_pending(&self) -> bool {
        self.registers[0x010 / 2] & self.registers[0x012 / 2] != 0
    }

    fn write_register(&mut self, off: usize, value: u16) {
        let mask = match off {
            0x004 => 0x9fff,
            0x006 => 0x003f,
            0x010 => {
                // W_IF acknowledges the written interrupt sources.
                self.registers[off / 2] &= !value;
                return;
            }
            0x012
            | 0x018..=0x024
            | 0x02c
            | 0x032
            | 0x050
            | 0x052
            | 0x080
            | 0x090
            | 0x094
            | 0x0a0
            | 0x0a4
            | 0x0a8
            | 0x0f0..=0x0fe
            | 0x122
            | 0x124
            | 0x128
            | 0x134
            | 0x140
            | 0x142
            | 0x14c
            | 0x17e => u16::MAX,
            0x028 => 0x000f,
            0x02a => 0x07ff,
            0x036 | 0x048 | 0x0bc => 3,
            0x038 => 7,
            0x03c => {
                if value & 2 != 0 {
                    self.registers[0x03c / 2] = 2;
                    self.registers[0x010 / 2] |= 1 << 11;
                    self.registers[0x214 / 2] = 1;
                }
                return;
            }
            0x040 => {
                if value & 0x8000 != 0 {
                    if value & 1 != 0 {
                        self.registers[0x03c / 2] = 0x0200;
                        self.registers[0x214 / 2] = 9;
                    } else if self.registers[0x03c / 2] & 0x0200 != 0 {
                        self.registers[0x03c / 2] |= 0x0100;
                    }
                }
                0x8001
            }
            0x21c => {
                self.registers[0x010 / 2] |= value & !(1 << 10);
                return;
            }
            0x2d0 => {
                if self.registers[0x03c / 2] & 0x0100 != 0 && value == 0 {
                    self.registers[0x03c / 2] &= !0x0300;
                    self.registers[0x214 / 2] = 1;
                }
                return;
            }
            0x056 | 0x05a | 0x05c | 0x064 | 0x06c | 0x076 | 0x130 => 0x0fff,
            0x058 | 0x062 | 0x068 | 0x074 => 0x1ffe,
            0x084 | 0x088 | 0x08e | 0x144..=0x14a | 0x15a => 0x00ff,
            0x08c => 0x03ff,
            0x0e8 | 0x0ea => 1,
            0x120 => 0x81ff,
            0x132 => 0x8fff,
            0x150 => 0xff3f,
            0x154 => 0x7a7f,
            0x158 => {
                self.baseband_command(value);
                return;
            }
            0x160 => 0x4100,
            0x168 => 0x800f,
            _ => return,
        };
        self.registers[off / 2] = value & mask;
    }
    fn baseband_command(&mut self, value: u16) {
        let index = usize::from(value as u8);
        match value >> 12 {
            5 if matches!(index, 0x01..=0x0c | 0x13..=0x15 | 0x1b..=0x26 |
                        0x28..=0x4c | 0x4e..=0x5c | 0x62..=0x63 | 0x65 | 0x67..=0x68) =>
            {
                self.baseband[index] = self.registers[0x15a / 2] as u8;
            }
            6 => {
                self.registers[0x15c / 2] = match index {
                    0 => 0x6d,
                    0x5d => 1,
                    0x64 => 0xff,
                    _ => u16::from(self.baseband.get(index).copied().unwrap_or(0)),
                };
            }
            _ => {}
        }
    }
}

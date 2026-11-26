//! Small stateful devices shared by the ARM7 and ARM9 memory ports.
pub(crate) mod backup;
pub(crate) mod wifi;

use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};
// Device capacity is in units of 128 KiB. Trimmed images can be smaller than
// their underlying chip; unpadded homebrew may have left this header byte unset.
fn card_capacity(rom: &[u8]) -> u32 {
    let declared = rom
        .get(0x14)
        .copied()
        .filter(|&shift| shift <= 12)
        .map_or(0, |shift| 0x2_0000 << shift);
    declared.max((rom.len() as u32).next_power_of_two())
}

pub(crate) fn chip_id(rom: &[u8]) -> u32 {
    let mib = (card_capacity(rom) >> 20).max(1);
    let size_code = if mib <= 128 {
        mib - 1
    } else {
        0x100 - mib / 256
    };
    0xc2 | size_code << 8
}
// The two calibration targets in firmware use one-based LCD coordinates.
// Use interior ADC values even at the screen edges, as on the physical panel.
const fn adc_x(x: u8) -> u16 {
    0x100 + x as u16 * 14
}
const fn adc_y(y: u8) -> u16 {
    0x100 + y as u16 * 19
}

pub(crate) const USER_SETTINGS: [u8; 0x70] = {
    let mut b = [0; 0x70];
    b[0] = 5;
    b[3] = 1;
    b[4] = 1;
    [b[0x58], b[0x59]] = adc_x(31).to_le_bytes();
    [b[0x5a], b[0x5b]] = adc_y(31).to_le_bytes();
    b[0x5c] = 32;
    b[0x5d] = 32;
    [b[0x5e], b[0x5f]] = adc_x(223).to_le_bytes();
    [b[0x60], b[0x61]] = adc_y(159).to_le_bytes();
    b[0x62] = 224;
    b[0x63] = 160;
    b[0x64] = 0x71;
    b[0x65] = 0xec;
    b
};

pub(crate) const fn crc16_byte(mut crc: u16, byte: u8) -> u16 {
    crc ^= byte as u16;
    let mut bit = 0;
    while bit < 8 {
        crc = if crc & 1 == 0 {
            crc >> 1
        } else {
            (crc >> 1) ^ 0xa001
        };
        bit += 1;
    }
    crc
}
const fn firmware_crc(mut crc: u16, bytes: &[u8]) -> u16 {
    let mut i = 0;
    while i < bytes.len() {
        crc = crc16_byte(crc, bytes[i]);
        i += 1;
    }
    crc
}
const SETTINGS_CRC: u16 = firmware_crc(0xffff, &USER_SETTINGS);

// The offline type-2 radio exposes a factory record just like the DS SPI flash.
// Channel and startup tables address real RF registers; analog trim is neutral.
const WIFI_SETTINGS: [u8; 0x138] = {
    const START: usize = 0x2c;
    const REGISTERS: [u8; 12] = [0, 4, 5, 6, 7, 8, 9, 10, 11, 1, 2, 3];
    let mut b = [0; 0x138];
    [b[0], b[1]] = (b.len() as u16).to_le_bytes();
    // A locally administered unicast address, stable across resets.
    b[0x36 - START] = 0x02;
    b[0x3b - START] = 0x01;
    [b[0x3c - START], b[0x3d - START]] = 0x3ffe_u16.to_le_bytes();
    [b[0x3e - START], b[0x3f - START]] = u16::MAX.to_le_bytes();
    b[0x40 - START] = 2;
    b[0x41 - START] = 24; // Bits per RF serial command.
    b[0x42 - START] = REGISTERS.len() as u8;
    b[0x43 - START] = 1;
    let mut i = 0;
    while i < REGISTERS.len() {
        // Bits 18..22 select the RF register in a little-endian 24-bit command.
        b[0xce - START + i * 3 + 2] = REGISTERS[i] << 2;
        i += 1;
    }
    let mut channel = 0;
    while channel < 14 {
        let offset = 0xf2 - START + channel * 6;
        b[offset + 2] = 5 << 2;
        b[offset + 5] = 6 << 2;
        channel += 1;
    }
    b
};
const WIFI_CRC: u16 = firmware_crc(0, &WIFI_SETTINGS);

fn firmware_byte(addr: u32) -> u8 {
    match addr {
        0x20 => 0xc0,
        0x21 => 0x7f, // User settings at 0x3fe00 (offset / 8).
        0x2a..=0x2b => WIFI_CRC.to_le_bytes()[(addr & 1) as usize],
        0x2c..=0x163 => WIFI_SETTINGS[(addr - 0x2c) as usize],
        0x3fe00..=0x3fe6f | 0x3ff00..=0x3ff6f => USER_SETTINGS[(addr & 255) as usize],
        0x3fe70 | 0x3fe71 | 0x3ff71 => 0,
        0x3ff70 => 1,
        0x3fe72..=0x3fe73 | 0x3ff72..=0x3ff73 => SETTINGS_CRC.to_le_bytes()[(addr & 1) as usize],
        _ => 0xff,
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Timers([Timer; 4]);
#[derive(Default, Serialize, Deserialize)]
struct Timer {
    reload: u16,
    counter: u16,
    ctl: u8,
    divider: u32,
}
impl Timers {
    pub(crate) fn valid(&self) -> bool {
        self.0.iter().all(|t| t.divider < 1024)
    }
    pub(crate) fn advance(&mut self, ticks: u32) -> u32 {
        let mut overflow = 0;
        let mut irq = 0;
        for (i, t) in self.0.iter_mut().enumerate() {
            if t.ctl & 128 == 0 {
                overflow = 0;
                continue;
            }
            let amount = if i != 0 && t.ctl & 4 != 0 {
                overflow
            } else {
                let period = [1, 64, 256, 1024][usize::from(t.ctl & 3)];
                t.divider += ticks;
                let n = t.divider / period;
                t.divider %= period;
                n
            };
            let n = u32::from(t.counter) + amount;
            if n <= 0xffff {
                t.counter = n as u16;
                overflow = 0;
            } else {
                let period = 0x1_0000 - u32::from(t.reload);
                overflow = 1 + (n - 0x1_0000) / period;
                t.counter = (u32::from(t.reload) + (n - 0x1_0000) % period) as u16;
                if t.ctl & 64 != 0 {
                    irq |= 1 << (3 + i);
                }
            }
        }
        irq
    }
    pub(crate) fn read(&self, off: usize) -> u8 {
        let t = &self.0[off / 4];
        match off % 4 {
            0 => t.counter as u8,
            1 => (t.counter >> 8) as u8,
            2 => t.ctl,
            _ => 0,
        }
    }
    pub(crate) fn write(&mut self, off: usize, v: u8) {
        let t = &mut self.0[off / 4];
        match off % 4 {
            0 => t.reload = (t.reload & 0xff00) | u16::from(v),
            1 => t.reload = (t.reload & 0xff) | (u16::from(v) << 8),
            2 => {
                if t.ctl & 128 == 0 && v & 128 != 0 {
                    t.counter = t.reload;
                    t.divider = 0;
                }
                t.ctl = v & 0xc7;
            }
            _ => {}
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Ipc {
    sync: [u8; 2],
    ctl: [Cell<u16>; 2],
    // Indexed by sender.
    queue: [RefCell<VecDeque<u32>>; 2],
    last: [Cell<u32>; 2],
}
fn signal(p: &Cell<u32>, mask: u32) {
    p.set(p.get() | mask);
}
impl Ipc {
    pub(crate) fn valid(&self) -> bool {
        self.queue.iter().all(|q| q.borrow().len() <= 16)
    }
    pub(crate) fn read_control(&self, cpu: usize, off: usize) -> u8 {
        let other = 1 - cpu;
        let send = self.queue[cpu].borrow().len();
        let recv = self.queue[other].borrow().len();
        match off {
            0x180 => self.sync[other] & 15,
            0x181 => self.sync[cpu] & 0x4f,
            0x184 => {
                (self.ctl[cpu].get() as u8 & 4) | u8::from(send == 0) | (u8::from(send == 16) << 1)
            }
            0x185 => {
                ((self.ctl[cpu].get() >> 8) as u8 & 0xc4)
                    | u8::from(recv == 0)
                    | (u8::from(recv == 16) << 1)
            }
            _ => 0,
        }
    }
    pub(crate) fn write_control(
        &mut self,
        cpu: usize,
        off: usize,
        v: u8,
        pending: &[Cell<u32>; 2],
    ) {
        let other = 1 - cpu;
        let old = self.ctl[cpu].get();
        match off {
            0x181 => {
                self.sync[cpu] = v & 0x4f;
                if v & 0x20 != 0 && self.sync[other] & 0x40 != 0 {
                    signal(&pending[other], 1 << 16);
                }
            }
            0x184 => {
                if v & 8 != 0 {
                    self.queue[cpu].borrow_mut().clear();
                }
                self.ctl[cpu].set((old & 0xff00) | u16::from(v & 4));
                if old & 4 == 0 && v & 4 != 0 && self.queue[cpu].borrow().is_empty() {
                    signal(&pending[cpu], 1 << 17);
                }
            }
            0x185 => {
                let error = if v & 0x40 == 0 { old & 0x4000 } else { 0 };
                self.ctl[cpu].set((u16::from(v & 0x84) << 8) | (old & 4) | error);
                if old & 0x400 == 0 && v & 4 != 0 && !self.queue[other].borrow().is_empty() {
                    signal(&pending[cpu], 1 << 18);
                }
            }
            _ => {}
        }
    }
    pub(crate) fn send(&self, cpu: usize, v: u32, pending: &[Cell<u32>; 2]) {
        let ctl = self.ctl[cpu].get();
        if ctl & 0x8000 == 0 {
            return;
        }
        let mut q = self.queue[cpu].borrow_mut();
        if q.len() == 16 {
            self.ctl[cpu].set(ctl | 0x4000);
            return;
        }
        if q.is_empty() && self.ctl[1 - cpu].get() & 0x400 != 0 {
            signal(&pending[1 - cpu], 1 << 18);
        }
        q.push_back(v);
    }
    pub(crate) fn receive(&self, cpu: usize, pending: &[Cell<u32>; 2]) -> u32 {
        let ctl = self.ctl[cpu].get();
        if ctl & 0x8000 != 0 {
            let mut q = self.queue[1 - cpu].borrow_mut();
            if let Some(word) = q.pop_front() {
                self.last[cpu].set(word);
                if q.is_empty() && self.ctl[1 - cpu].get() & 4 != 0 {
                    signal(&pending[1 - cpu], 1 << 17);
                }
            } else {
                self.ctl[cpu].set(ctl | 0x4000);
            }
        }
        self.last[cpu].get()
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Card {
    op: u8,
    position: Cell<u32>,
    remaining: Cell<u32>,
}
impl Card {
    pub(crate) fn ready(&self) -> bool {
        self.remaining.get() != 0
    }
    pub(crate) fn start(&mut self, cmd: [u8; 8], size: u8) {
        self.op = cmd[0];
        self.position
            .set(u32::from_be_bytes([cmd[1], cmd[2], cmd[3], cmd[4]]));
        self.remaining.set(match size {
            0 => 0,
            7 => 4,
            n => 0x100 << n,
        });
    }
    pub(crate) fn read(&self, rom: &[u8]) -> u32 {
        if !self.ready() {
            return 0;
        }
        let pos = self.position.get();
        self.remaining.set(self.remaining.get().saturating_sub(4));
        // The bus wraps inside its current 4 KiB ROM page for the entire command.
        self.position
            .set((pos & !0xfff) | (pos.wrapping_add(4) & 0xfff));
        match self.op {
            0x90 | 0xb8 => chip_id(rom),
            0xb7 | 0 => u32::from_le_bytes(std::array::from_fn(|i| {
                let addr = ((pos & !0xfff) | (pos.wrapping_add(i as u32) & 0xfff))
                    & (card_capacity(rom) - 1);
                let addr = if self.op == 0 {
                    addr & 0xfff
                } else if addr < 0x8000 {
                    0x8000 | (addr & 0x1ff)
                } else {
                    addr
                };
                rom.get(addr as usize).copied().unwrap_or(0xff)
            })),
            _ => u32::MAX,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Spi {
    command: Option<u8>,
    phase: u32,
    addr: u32,
    value: u8,
    touch_sample: u16,
    power: [u8; 8],
}
impl Spi {
    pub(crate) fn result(&self) -> u8 {
        self.value
    }
    pub(crate) fn deselect(&mut self) {
        self.phase = 0;
        self.addr = 0;
        self.command = None;
    }
    pub(crate) fn transfer(&mut self, ctl: u16, v: u8, pen: Option<(u8, u8)>) {
        if ctl & 0x8000 == 0 {
            return;
        }
        let dev = (ctl >> 8) & 3;
        self.value = 0;
        if let Some(cmd) = self.command {
            self.phase = self.phase.wrapping_add(1);
            self.value = match dev {
                0 => {
                    let i = usize::from(cmd & 7);
                    if cmd & 0x80 != 0 {
                        self.power[i]
                    } else {
                        self.power[i] = v;
                        0
                    }
                }
                1 if cmd == 0x03 && self.phase <= 3 => {
                    self.addr = (self.addr << 8) | u32::from(v);
                    0
                }
                1 if cmd == 0x03 => {
                    let r = firmware_byte(self.addr);
                    self.addr = self.addr.wrapping_add(1);
                    r
                }
                1 if cmd == 0x05 => 0,
                1 if cmd == 0x9f => [0x20, 0x40, 0x12][(self.phase.wrapping_sub(1) % 3) as usize],
                2 if self.phase == 1 => (self.touch_sample >> 5) as u8,
                2 if self.phase == 2 => (self.touch_sample << 3) as u8,
                _ => 0,
            };
        }
        if self.command.is_none() || dev == 2 && v & 0x80 != 0 {
            self.command = Some(v);
            self.phase = 0;
            if dev == 2 {
                let (x, y) = pen.unwrap_or_default();
                self.touch_sample = match v >> 4 & 7 {
                    1 => {
                        if pen.is_some() {
                            adc_y(y)
                        } else {
                            0xfff
                        }
                    }
                    5 if pen.is_some() => adc_x(x),
                    3 if pen.is_some() => 0x400,
                    4 => {
                        if pen.is_some() {
                            0x800
                        } else {
                            0xfff
                        }
                    }
                    _ => 0,
                };
                if v & 8 != 0 {
                    self.touch_sample &= 0x0ff0;
                }
            }
        }
        if ctl & (1 << 11) == 0 {
            self.deselect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spi_exposes_a_checksummed_wireless_factory_record() {
        const HOLD: u16 = 0x8900;
        let mut spi = Spi::default();
        for value in [3, 0, 0, 0x2a] {
            spi.transfer(HOLD, value, None);
        }
        let record: Vec<_> = (0x2a..0x164)
            .map(|_| {
                spi.transfer(HOLD, 0, None);
                spi.result()
            })
            .collect();
        let checksum = u16::from_le_bytes([record[0], record[1]]);
        let length = u16::from_le_bytes([record[2], record[3]]);
        assert_eq!(usize::from(length), record.len() - 2);
        assert_eq!(firmware_crc(0, &record[2..]), checksum);
        assert_eq!(record[0x36 - 0x2a] & 3, 2);
        assert_eq!(&record[0x3c - 0x2a..0x3e - 0x2a], &[0xfe, 0x3f]);
        assert_eq!(&record[0x40 - 0x2a..0x43 - 0x2a], &[2, 24, 12]);
    }
}

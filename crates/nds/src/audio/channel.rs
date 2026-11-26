//! Individual timer-driven sample decoders. Positions are counted in nibbles.
use super::gain;
use crate::bus::SoundRam;
use serde::{Deserialize, Serialize};

const ENABLE: u32 = 1 << 31;
const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
struct Decoder {
    value: i16,
    index: u8,
}
impl Decoder {
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEPS[usize::from(self.index)];
        let mut delta = step >> 3;
        if nibble & 1 != 0 {
            delta += step >> 2;
        }
        if nibble & 2 != 0 {
            delta += step >> 1;
        }
        if nibble & 4 != 0 {
            delta += step;
        }
        let value = i32::from(self.value);
        self.value = if nibble & 8 == 0 {
            (value + delta).min(32767)
        } else {
            (value - delta).max(-32767)
        } as i16;
        let change: i16 = [-1, -1, -1, -1, 2, 4, 6, 8][usize::from(nibble & 7)];
        self.index = (i16::from(self.index) + change).clamp(0, 88) as u8;
        self.value
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Channel {
    pub(super) control: u32,
    source: u32,
    timer: u16,
    loop_words: u16,
    length_words: u32,
    address: u32,
    position: u32,
    wait: u8,
    noise: u16,
    decoder: Decoder,
    loop_decoder: Decoder,
    sample: i16,
    pub(super) remaining: u32,
}
impl Channel {
    pub(super) fn valid(&self) -> bool {
        self.decoder.index < 89
            && self.loop_decoder.index < 89
            && self.remaining <= 131_072
            && (!self.active() || self.remaining > 0)
            && self.length_words <= 0x3f_ffff
            && self.address <= 0x07ff_fffc
            && self.source <= 0x07ff_fffc
            && self.wait <= 11
    }
    pub(super) fn active(&self) -> bool {
        self.control & ENABLE != 0
    }
    pub(super) fn period(&self) -> u32 {
        2 * (65536 - u32::from(self.timer))
    }
    pub(super) fn pan(&self) -> i64 {
        gain((self.control >> 16) as u8)
    }
    pub(super) fn volume_sample(&self) -> i64 {
        let shift = [0, 1, 2, 4][(self.control as usize >> 8) & 3];
        ((i64::from(self.sample) << 4) >> shift) * gain(self.control as u8)
    }
    fn format(&self) -> u32 {
        (self.control >> 29) & 3
    }
    pub(super) fn write(&mut self, off: usize, v: u8) {
        let update_word = |word: &mut u32, mask: u32| {
            let shift = (off & 3) * 8;
            *word = ((*word & !(255 << shift)) | (u32::from(v) << shift)) & mask;
        };
        let update_half = |half: &mut u16| {
            let shift = (off & 1) * 8;
            *half = (*half & !(255 << shift)) | (u16::from(v) << shift);
        };
        match off {
            0..=3 => {
                let old = self.active();
                update_word(&mut self.control, 0xff7f_837f);
                if !old && self.active() {
                    self.address = self.source;
                    self.position = 0;
                    self.remaining = self.period();
                    self.noise = 0x7fff;
                    self.wait = match self.format() {
                        3 => 1,
                        2 => 11,
                        _ => 3,
                    };
                    self.sample = 0;
                } else if off == 3 && !self.active() {
                    self.sample = 0;
                }
            }
            4..=7 => update_word(&mut self.source, 0x07ff_fffc),
            8..=9 => update_half(&mut self.timer),
            10..=11 => update_half(&mut self.loop_words),
            _ => update_word(&mut self.length_words, 0x3f_ffff),
        }
    }
    pub(super) fn tick(&mut self, id: usize, ram: &SoundRam<'_>) {
        self.remaining = self.period();
        if self.wait > 0 {
            self.wait -= 1;
            if self.wait == 0 && self.format() == 2 {
                self.decoder = Decoder {
                    value: ram.read16(self.address),
                    index: (ram.read8(self.address + 2) & 127).min(88),
                };
                self.loop_decoder = self.decoder;
                self.position = 8;
            }
            return;
        }
        if self.format() == 3 {
            let duty = (self.control >> 24) & 7;
            self.sample = match id {
                8..=13 if duty != 7 && self.position & 7 >= 7 - duty => 32767,
                8..=13 => -32767,
                14..=15 => {
                    let low = self.noise & 1 != 0;
                    self.noise = (self.noise >> 1) ^ if low { 0x6000 } else { 0 };
                    if low { -32767 } else { 32767 }
                }
                _ => 0,
            };
            self.position = self.position.wrapping_add(1) & 7;
            return;
        }
        let start = u32::from(self.loop_words) * 8;
        let end = start + self.length_words * 8;
        let repeat = self.control >> 27 & 3;
        if end < 32 && repeat != 0 {
            self.sample = 0;
            return;
        }
        if self.position >= end && repeat != 0 {
            if repeat == 1 && self.length_words != 0 {
                self.position = if self.format() == 2 {
                    start.max(8)
                } else {
                    start
                };
                self.decoder = self.loop_decoder;
            } else {
                self.control &= !ENABLE;
                if self.control & 0x8000 == 0 {
                    self.sample = 0;
                }
                return;
            }
        }
        let addr = self.address.wrapping_add(self.position / 2);
        self.sample = match self.format() {
            0 => {
                self.position = self.position.wrapping_add(2);
                i16::from(ram.read8(addr) as i8) << 8
            }
            1 => {
                self.position = self.position.wrapping_add(4);
                ram.read16(addr)
            }
            _ => {
                if self.position == start {
                    self.loop_decoder = self.decoder;
                }
                let shift = (self.position & 1) * 4;
                self.position = self.position.wrapping_add(1);
                self.decoder.decode((ram.read8(addr) >> shift) & 15)
            }
        };
    }
}

//! ARM7 sound unit. Playback and capture clocks run independently of the host audio device.
mod channel;
use crate::bus::SoundRam;
use channel::Channel;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const DAC_PERIOD: u32 = 1024;
// Keep the latest quarter second when a debugger or a headless caller does not drain PCM.
const QUEUE_LIMIT: usize = 8192;
fn gain(v: u8) -> i64 {
    let g = v & 127;
    i64::from(g) + i64::from(g == 127)
}

#[derive(Default, Serialize, Deserialize)]
struct Capture {
    control: u8,
    destination: u32,
    words: u16,
    offset: u32,
    remaining: u32,
}
impl Capture {
    fn record(&mut self, ram: &mut SoundRam<'_>, sample: i16) {
        let addr = self.destination + self.offset;
        if self.control & 8 != 0 {
            ram.write8(addr, sample.to_le_bytes()[1]);
            self.offset += 1;
        } else {
            let [lo, hi] = sample.to_le_bytes();
            ram.write8(addr, lo);
            ram.write8(addr + 1, hi);
            self.offset += 2;
        }
        if self.offset >= u32::from(self.words.max(1)) * 4 {
            self.offset = 0;
            if self.control & 4 != 0 {
                self.control &= 0x7f;
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Audio {
    channels: [Channel; 16],
    capture: [Capture; 2],
    control: u16,
    bias: u16,
    dac_remaining: u32,
    elapsed: u32,
    next: u32,
    #[serde(skip)]
    pending: VecDeque<[i16; 2]>,
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            channels: Default::default(),
            capture: Default::default(),
            control: 0,
            bias: 512,
            dac_remaining: DAC_PERIOD,
            elapsed: 0,
            next: DAC_PERIOD,
            pending: VecDeque::new(),
        }
    }
}
impl Audio {
    // Scheduling the earliest source, capture or DAC event avoids scanning all 16
    // channels on every four-cycle instruction quantum.
    fn next_event(&self) -> u32 {
        let mut next = self.dac_remaining;
        if self.control & 0x8000 != 0 {
            for ch in &self.channels {
                if ch.active() {
                    next = next.min(ch.remaining);
                }
            }
            for cap in &self.capture {
                if cap.control & 128 != 0 {
                    next = next.min(cap.remaining);
                }
            }
        }
        next
    }
    pub(crate) fn valid(&self) -> bool {
        (1..=DAC_PERIOD).contains(&self.dac_remaining)
            && self.bias < 1024
            && self.channels.iter().all(Channel::valid)
            && self.capture.iter().all(|c| {
                c.destination <= 0x07ff_fffc
                    && c.offset <= 0x40000
                    && c.remaining <= 131_072
                    && (c.control & 128 == 0 || c.remaining > 0)
            })
            && self.next == self.next_event()
            && self.elapsed < self.next
    }
    pub(crate) fn take_samples(&mut self) -> Vec<[i16; 2]> {
        self.pending.drain(..).collect()
    }
    pub(crate) fn accumulate(&mut self, ticks: u16) -> bool {
        self.elapsed += u32::from(ticks);
        self.elapsed >= self.next
    }
    pub(crate) fn read(&self, off: usize) -> u8 {
        match off {
            0x000..=0x0ff if off & 15 < 4 => self.channels[off >> 4].control.to_le_bytes()[off & 3],
            0x100..=0x101 => self.control.to_le_bytes()[off & 1],
            0x104..=0x105 => self.bias.to_le_bytes()[off & 1],
            0x108..=0x109 => self.capture[off & 1].control,
            0x110..=0x113 | 0x118..=0x11b => {
                self.capture[(off >> 3) & 1].destination.to_le_bytes()[off & 3]
            }
            _ => 0,
        }
    }
    pub(crate) fn write(&mut self, off: usize, v: u8) {
        match off {
            0..=0xff => self.channels[off >> 4].write(off & 15, v),
            0x100 => self.control = self.control & 0xff00 | u16::from(v & 127),
            0x101 => self.control = self.control & 0xff | u16::from(v & 0xbf) << 8,
            0x104 => self.bias = (self.bias & 0x300) | u16::from(v),
            0x105 => self.bias = (self.bias & 0xff) | u16::from(v & 3) << 8,
            0x108..=0x109 => {
                let i = off & 1;
                let c = &mut self.capture[i];
                if c.control & 128 == 0 && v & 128 != 0 {
                    c.offset = 0;
                    c.remaining = self.channels[i * 2 + 1].period();
                }
                c.control = v & 0x8f;
            }
            0x110..=0x115 | 0x118..=0x11d => {
                let c = &mut self.capture[(off >> 3) & 1];
                if off & 7 < 4 {
                    let shift = (off & 3) * 8;
                    c.destination =
                        ((c.destination & !(255 << shift)) | u32::from(v) << shift) & 0x07ff_fffc;
                } else {
                    let shift = (off & 1) * 8;
                    c.words = (c.words & !(255 << shift)) | u16::from(v) << shift;
                }
            }
            _ => return,
        }
        self.next = self.next_event();
    }
    /// Flush partial timers before register writes so a low-frequency channel stays in phase.
    pub(crate) fn flush(&mut self, ram: &mut SoundRam<'_>, powered: bool) {
        while self.elapsed != 0 {
            let cycles = self.elapsed.min(self.next);
            self.elapsed -= cycles;
            self.dac_remaining -= cycles;
            let mut capture_due = [false; 2];
            if self.control & 0x8000 != 0 && powered {
                for (id, ch) in self.channels.iter_mut().enumerate() {
                    if ch.active() {
                        ch.remaining -= cycles;
                        if ch.remaining == 0 {
                            ch.tick(id, ram);
                        }
                    }
                }
                for (i, cap) in self.capture.iter_mut().enumerate() {
                    if cap.control & 128 != 0 {
                        cap.remaining -= cycles;
                        if cap.remaining == 0 {
                            cap.remaining = self.channels[2 * i + 1].period();
                            capture_due[i] = true;
                        }
                    }
                }
            }
            if self.dac_remaining == 0 || capture_due != [false; 2] {
                let (output, capture) = self.mix(powered);
                for (i, due) in capture_due.into_iter().enumerate() {
                    if due {
                        self.capture[i].record(ram, capture[i]);
                    }
                }
                if self.dac_remaining == 0 {
                    if self.pending.len() == QUEUE_LIMIT {
                        self.pending.pop_front();
                    }
                    self.pending.push_back(output);
                    self.dac_remaining = DAC_PERIOD;
                }
            }
            self.next = self.next_event();
        }
    }
    fn mix(&self, powered: bool) -> ([i16; 2], [i16; 2]) {
        if !powered {
            return ([0; 2], [0; 2]);
        }
        if self.control & 0x8000 == 0 {
            return ([((self.bias as i16) - 512) << 6; 2], [0; 2]);
        }
        let mut mix = [0i64; 2];
        let mut direct = [[0; 2]; 2];
        let mut capture = [0; 2];
        for (i, ch) in self.channels.iter().enumerate() {
            let mut value = ch.volume_sample();
            let pan = ch.pan();
            if i == 0 || i == 2 {
                let c = &self.capture[i / 2];
                let other = self.channels[i + 1].volume_sample();
                let add = c.control & 0x81 == 0x81;
                // Direct-channel capture wraps sums and produces the most negative
                // value if both input channels are negative with addition disabled.
                let capture_value = if add {
                    value + other
                } else if value < 0 && other < 0 {
                    -32768 << 11
                } else {
                    value
                };
                capture[i / 2] = (capture_value >> 11) as i16;
                if add && ch.active() && self.control & (1 << (12 + i / 2)) == 0 {
                    value += other;
                }
            }
            // The volume output has eleven fractional bits. The panner hands
            // the mixer eight, retaining precision across the channel sum.
            let sides = [(value * (128 - pan)) >> 10, (value * pan) >> 10];
            if i == 1 || i == 3 {
                direct[i / 2] = sides;
                if self.control & (1 << (12 + i / 2)) != 0
                    || self.capture[i / 2].control & 0x81 == 0x81
                {
                    continue;
                }
            }
            for (side, add) in mix.iter_mut().zip(sides) {
                *side += add;
            }
        }
        let output = std::array::from_fn(|side| {
            if self.capture[side].control & 2 == 0 {
                capture[side] = (mix[side] >> 8).clamp(-32768, 32767) as i16;
            }
            let selected = match (self.control >> (8 + 2 * side)) & 3 {
                0 => mix[side],
                1 => direct[0][side],
                2 => direct[1][side],
                _ => direct[0][side] + direct[1][side],
            };
            let dac = (((selected * gain(self.control as u8)) >> 21) + i64::from(self.bias))
                .clamp(0, 1023);
            ((dac - 512) << 6) as i16
        });
        (output, capture)
    }
}

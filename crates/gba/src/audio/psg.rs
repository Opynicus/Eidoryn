//! The four Game Boy-compatible oscillators and their 512 Hz frame sequencer.
use serde::{Deserialize, Serialize};

const LENGTH: [usize; 4] = [0x62, 0x68, 0x72, 0x78];
const CONTROL: [usize; 4] = [0x65, 0x6d, 0x75, 0x7d];
const SEQUENCER_PERIOD: u32 = 32_768;

#[derive(Default, Serialize, Deserialize)]
struct Voice {
    active: bool,
    length: u16,
    remaining: u32,
    phase: u8,
    volume: u8,
    envelope_ticks: u8,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Psg {
    voices: [Voice; 4],
    wave: [[u8; 16]; 2],
    noise: u16,
    sweep_shadow: u16,
    sweep_ticks: u8,
    sweep_negated: bool,
    sequencer_cycles: u32,
    sequencer_step: u8,
}

fn frequency(io: &[u8], index: usize) -> u16 {
    let hi = CONTROL[index];
    u16::from_le_bytes([io[hi - 1], io[hi] & 7])
}

fn period(io: &[u8], index: usize) -> u32 {
    if index < 3 {
        let divider = 2048 - u32::from(frequency(io, index));
        divider * if index == 2 { 8 } else { 16 }
    } else {
        let ratio = u32::from(io[0x7c] & 7);
        (if ratio == 0 { 32 } else { 64 * ratio }) << (io[0x7c] >> 4)
    }
}

impl Psg {
    pub(super) fn valid(&self) -> bool {
        self.voices.iter().enumerate().all(|(index, voice)| {
            voice.length <= if index == 2 { 256 } else { 64 }
                && voice.phase < if index == 2 { 64 } else { 8 }
                && voice.volume < 16
                && voice.envelope_ticks <= 8
                && voice.remaining <= (448 << 15)
                && (!voice.active || voice.remaining != 0)
        }) && self.sweep_shadow < 2048
            && self.sweep_ticks <= 8
            && self.sequencer_cycles < SEQUENCER_PERIOD
            && self.sequencer_step < 8
    }

    pub(super) fn power_off(&mut self) {
        let wave = self.wave;
        *self = Self {
            wave,
            ..Self::default()
        };
    }

    fn wave_bank(&self, io: &[u8]) -> usize {
        let bank = io[0x70] >> 6 & 1;
        usize::from(if self.voices[2].active && io[0x70] & 0x20 != 0 {
            bank ^ (self.voices[2].phase >> 5)
        } else {
            bank
        })
    }

    pub(super) fn read_wave(&self, io: &[u8], offset: usize) -> u8 {
        self.wave[self.wave_bank(io) ^ 1][offset - 0x90]
    }

    pub(super) fn write_wave(&mut self, io: &[u8], offset: usize, value: u8) {
        self.wave[self.wave_bank(io) ^ 1][offset - 0x90] = value;
    }

    pub(super) fn status(&self) -> u8 {
        self.voices
            .iter()
            .enumerate()
            .fold(0, |bits, (index, v)| bits | (u8::from(v.active) << index))
    }

    pub(super) fn write(&mut self, io: &mut [u8], offset: usize, value: u8) {
        if offset == 0x60 && self.sweep_negated && value & 8 == 0 {
            self.voices[0].active = false;
        }
        io[offset] = value;
        match offset {
            0x62 | 0x68 | 0x72 | 0x78 => {
                let index = (offset - 0x60) / 8;
                self.voices[index].length = if index == 2 {
                    256 - u16::from(value)
                } else {
                    64 - u16::from(value & 63)
                };
            }
            0x63 | 0x69 | 0x79 if value & 0xf8 == 0 => {
                self.voices[(offset - 0x60) / 8].active = false;
            }
            0x70 if value & 0x80 == 0 => self.voices[2].active = false,
            0x65 | 0x6d | 0x75 | 0x7d if value & 0x80 != 0 => {
                self.trigger(io, (offset - 0x60) / 8);
                io[offset] &= !0x80;
            }
            _ => {}
        }
    }

    fn trigger(&mut self, io: &[u8], index: usize) {
        let voice = &mut self.voices[index];
        let envelope = io[LENGTH[index] + 1];
        voice.active = if index == 2 {
            io[0x70] & 0x80 != 0
        } else {
            envelope & 0xf8 != 0
        };
        if voice.length == 0 {
            voice.length = if index == 2 { 256 } else { 64 };
        }
        voice.volume = envelope >> 4;
        voice.envelope_ticks = envelope & 7;
        voice.phase = 0;
        voice.remaining = period(io, index);
        if index == 0 {
            self.sweep_shadow = frequency(io, 0);
            self.sweep_ticks = io[0x60] >> 4 & 7;
            self.sweep_negated = false;
            if io[0x60] & 7 != 0 && self.sweep_frequency(io[0x60]) >= 2048 {
                self.voices[0].active = false;
            }
        } else if index == 3 {
            self.noise = if io[0x7c] & 8 == 0 { 0x4000 } else { 0x40 };
        }
    }

    fn sweep_frequency(&mut self, sweep: u8) -> u16 {
        let delta = self.sweep_shadow >> (sweep & 7);
        if sweep & 8 == 0 {
            self.sweep_shadow + delta
        } else {
            self.sweep_negated = true;
            self.sweep_shadow - delta
        }
    }

    fn sequence(&mut self, io: &mut [u8]) {
        let step = self.sequencer_step;
        self.sequencer_step = (step + 1) & 7;
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if step & 1 == 0 && io[CONTROL[index]] & 0x40 != 0 && voice.length != 0 {
                voice.length -= 1;
                if voice.length == 0 {
                    voice.active = false;
                }
            }
            if step == 7 && index != 2 && voice.active && voice.envelope_ticks != 0 {
                voice.envelope_ticks -= 1;
                if voice.envelope_ticks == 0 {
                    let envelope = io[LENGTH[index] + 1];
                    voice.envelope_ticks = envelope & 7;
                    if voice.envelope_ticks != 0 {
                        voice.volume = if envelope & 8 == 0 {
                            voice.volume.saturating_sub(1)
                        } else {
                            (voice.volume + 1).min(15)
                        };
                    }
                }
            }
        }
        if matches!(step, 2 | 6) && self.voices[0].active && self.sweep_ticks != 0 {
            self.sweep_ticks -= 1;
            if self.sweep_ticks == 0 {
                let sweep = io[0x60];
                self.sweep_ticks = sweep >> 4 & 7;
                if self.sweep_ticks != 0 && sweep & 7 != 0 {
                    let next = self.sweep_frequency(sweep);
                    if next < 2048 {
                        self.sweep_shadow = next;
                        io[0x64] = next as u8;
                        io[0x65] = (io[0x65] & !7) | (next >> 8) as u8;
                    }
                    if next >= 2048 || self.sweep_frequency(sweep) >= 2048 {
                        self.voices[0].active = false;
                    }
                }
            }
        }
    }

    pub(super) fn advance(&mut self, io: &mut [u8], mut cycles: u32) {
        while cycles != 0 {
            let span = cycles.min(SEQUENCER_PERIOD - self.sequencer_cycles);
            for (index, v) in self.voices.iter_mut().enumerate() {
                if !v.active {
                    continue;
                }
                if span < v.remaining {
                    v.remaining -= span;
                    continue;
                }
                let period = period(io, index);
                let elapsed = span - v.remaining;
                let steps = 1 + elapsed / period;
                v.remaining = period - elapsed % period;
                if index == 3 {
                    let taps = if io[0x7c] & 8 == 0 { 0x6000 } else { 0x60 };
                    for _ in 0..steps {
                        let high = self.noise & 1;
                        self.noise >>= 1;
                        if high != 0 {
                            self.noise ^= taps;
                        }
                        v.phase = high as u8;
                    }
                } else {
                    let mask = if index == 2 { 63 } else { 7 };
                    v.phase = v.phase.wrapping_add(steps as u8) & mask;
                }
            }
            self.sequencer_cycles += span;
            cycles -= span;
            if self.sequencer_cycles == SEQUENCER_PERIOD {
                self.sequencer_cycles = 0;
                self.sequence(io);
            }
        }
    }

    pub(super) fn amplitudes(&self, io: &[u8]) -> [i32; 4] {
        std::array::from_fn(|index| {
            let v = &self.voices[index];
            if !v.active {
                return 0;
            }
            if index == 2 {
                let byte = self.wave[self.wave_bank(io)][usize::from((v.phase & 31) / 2)];
                let sample = if v.phase & 1 == 0 {
                    byte >> 4
                } else {
                    byte & 15
                };
                let sample = 2 * i32::from(sample) - 15;
                let level = io[0x73];
                if level & 0x80 != 0 {
                    sample * 3 / 4
                } else {
                    match level >> 5 & 3 {
                        0 => 0,
                        1 => sample,
                        2 => sample / 2,
                        _ => sample / 4,
                    }
                }
            } else {
                let high = if index == 3 {
                    v.phase != 0
                } else {
                    const DUTIES: [u8; 4] = [0x01, 0x81, 0x87, 0x7e];
                    DUTIES[usize::from(io[LENGTH[index]] >> 6)] & (1 << v.phase) != 0
                };
                if high {
                    i32::from(v.volume)
                } else {
                    -i32::from(v.volume)
                }
            }
        })
    }
}

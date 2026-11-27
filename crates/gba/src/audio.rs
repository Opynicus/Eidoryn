//! PSG, timer-driven Direct Sound FIFOs and the stereo PWM mixer on the Game Pak bus.
mod psg;
mod pwm;
#[cfg(test)]
mod tests;

use psg::Psg;
use pwm::PwmOutput;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Number of stereo frames per second delivered to the host, independent of PWM resolution.
pub const AUDIO_SAMPLE_RATE: u32 = 65_536;
const CLOCKS_PER_SAMPLE: u32 = 16_777_216 / AUDIO_SAMPLE_RATE;
const BASE_PWM_PERIOD: u32 = 512;
const MAX_QUEUED: usize = AUDIO_SAMPLE_RATE as usize / 4;

#[derive(Default, Serialize, Deserialize)]
struct Fifo {
    bytes: [i8; 32],
    first: u8,
    count: u8,
    sample: i8,
}

impl Fifo {
    fn push(&mut self, value: u8) {
        if self.count < 32 {
            self.bytes[usize::from((self.first + self.count) & 31)] = value as i8;
            self.count += 1;
        }
    }

    fn pop(&mut self) {
        if self.count != 0 {
            self.sample = self.bytes[usize::from(self.first)];
            self.first = (self.first + 1) & 31;
            self.count -= 1;
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Audio {
    psg: Psg,
    fifo: [Fifo; 2],
    // Oscillators advance lazily to the next PSG access or PWM sampling edge.
    pending_cycles: u32,
    phase: u32,
    pwm: PwmOutput,
    #[serde(skip)]
    output: VecDeque<[i16; 2]>,
}

/// Version 3 saved partial 32,768 Hz box-filter accumulators.
#[derive(Deserialize)]
pub(super) struct LegacyAudio {
    psg: Psg,
    fifo: [Fifo; 2],
    pending_cycles: u32,
    phase: u32,
    integrated_cycles: u32,
    sum: [i32; 2],
}

impl LegacyAudio {
    pub(super) fn restore(self) -> Option<Audio> {
        if self.phase >= BASE_PWM_PERIOD
            || self.integrated_cycles > self.phase
            || self
                .sum
                .iter()
                .any(|s| s.unsigned_abs() > self.integrated_cycles * 32768)
        {
            return None;
        }
        let mut pwm = PwmOutput::default();
        pwm.advance((self.phase / 64) as usize, None);
        Some(Audio {
            psg: self.psg,
            fifo: self.fifo,
            pending_cycles: self.pending_cycles,
            phase: self.phase,
            pwm,
            output: VecDeque::new(),
        })
    }
}

impl Audio {
    pub(super) fn valid(&self) -> bool {
        self.psg.valid()
            && self
                .fifo
                .iter()
                .all(|fifo| fifo.first < 32 && fifo.count <= 32)
            && self.phase < BASE_PWM_PERIOD
            && self.pending_cycles <= self.phase
            && self.pwm.valid(self.phase)
    }

    /// BIOS `RegisterRamReset` clears audio registers inside a running timeline.
    pub(super) fn reset_registers(&mut self) {
        self.psg = Psg::default();
        self.fifo = [Fifo::default(), Fifo::default()];
        self.pending_cycles = 0;
    }

    pub(super) fn drain(&mut self) -> Vec<[i16; 2]> {
        self.output.drain(..).collect()
    }

    pub(super) fn read(&self, io: &[u8], offset: usize) -> u8 {
        match offset {
            0x60 => io[offset] & 0x7f,
            0x62 | 0x68 => io[offset] & 0xc0,
            0x63 | 0x69 | 0x79 | 0x7c | 0x81 => io[offset],
            0x65 | 0x6d | 0x75 | 0x7d => io[offset] & 0x40,
            0x70 | 0x73 => io[offset] & 0xe0,
            0x80 | 0x83 => io[offset] & 0x77,
            0x82 => io[offset] & 0xf,
            0x84 => (io[offset] & 0x80) | self.psg.status(),
            0x88 => io[offset] & 0xfe,
            0x89 => io[offset] & 0xc3,
            0x90..=0x9f => self.psg.read_wave(io, offset),
            _ => 0,
        }
    }

    fn sync_psg(&mut self, io: &mut [u8]) {
        if self.pending_cycles != 0 && io[0x84] & 0x80 != 0 {
            self.psg.advance(io, self.pending_cycles);
        }
        self.pending_cycles = 0;
    }

    pub(super) fn write(&mut self, io: &mut [u8], offset: usize, value: u8) {
        match offset {
            0x60..=0x81 if io[0x84] & 0x80 != 0 => {
                self.sync_psg(io);
                self.psg.write(io, offset, value);
            }
            0x82 => io[offset] = value & 15,
            0x83 => {
                io[offset] = value & 0x77;
                for (index, fifo) in self.fifo.iter_mut().enumerate() {
                    if value & (8 << (4 * index)) != 0 {
                        *fifo = Fifo::default();
                    }
                }
            }
            0x84 => {
                self.sync_psg(io);
                if value & 0x80 == 0 {
                    io[0x60..=0x81].fill(0);
                    self.psg.power_off();
                }
                io[offset] = value & 0x80;
            }
            0x88 => io[offset] = value & 0xfe,
            0x89 => io[offset] = value & 0xc3,
            0x90..=0x9f => {
                self.sync_psg(io);
                self.psg.write_wave(io, offset, value);
            }
            0xa0..=0xa7 => self.fifo[(offset - 0xa0) / 4].push(value),
            _ => {}
        }
    }

    pub(super) fn timer_overflow(&mut self, io: &[u8], timer: usize) -> u8 {
        if io[0x84] & 0x80 == 0 {
            return 0;
        }
        let mut requests = 0;
        for (index, fifo) in self.fifo.iter_mut().enumerate() {
            let route = io[0x83] >> (index * 4);
            if usize::from(route >> 2 & 1) == timer {
                fifo.pop();
                if fifo.count <= 16 {
                    requests |= 1 << index;
                }
            }
        }
        requests
    }

    pub(super) fn until_pwm(&self, io: &[u8]) -> u32 {
        let period = BASE_PWM_PERIOD >> (io[0x89] >> 6);
        period - (self.phase & (period - 1))
    }

    fn mix(&self, io: &[u8]) -> [i32; 2] {
        if io[0x84] & 0x80 == 0 {
            return [0; 2];
        }
        let psg = self.psg.amplitudes(io);
        let bias = i32::from(u16::from_le_bytes([io[0x88], io[0x89] & 3]));
        let truncation = (1 << (1 + (io[0x89] >> 6))) - 1;
        [1, 0].map(|side| {
            let mask = io[0x81] >> (4 * side);
            let volume = i32::from(1 + (io[0x80] >> (4 * side) & 7));
            let amplitude: i32 = psg
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, a)| *a)
                .sum();
            let mut total = amplitude * volume / (4 >> (io[0x82] & 3).min(2));
            for (i, fifo) in self.fifo.iter().enumerate() {
                if (io[0x83] >> (4 * i + side)) & 1 != 0 {
                    total += i32::from(fifo.sample) << (1 + (io[0x82] >> (2 + i) & 1));
                }
            }
            // SOUNDBIAS sets the DAC operating point and its clipping headroom.
            // Remove the quantized idle level before sending signed PCM to the host.
            let dac = (total + bias).clamp(0, 1023) & !truncation;
            let pcm = (dac - (bias & !truncation)) * 64;
            pcm.clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        })
    }

    /// Latch the DAC on its hardware clock and publish on the independent PCM clock.
    pub(super) fn advance(&mut self, io: &mut [u8], mut cycles: u32) {
        while cycles != 0 {
            let until = self.until_pwm(io);
            let to_output = CLOCKS_PER_SAMPLE - self.phase % CLOCKS_PER_SAMPLE;
            let span = cycles.min(until).min(to_output);
            let previous_tick = self.phase / 64;
            self.phase += span;
            self.pending_cycles += span;
            cycles -= span;
            let ticks = self.phase / 64 - previous_tick;
            if ticks != 0 {
                let edge = (span == until).then(|| {
                    self.sync_psg(io);
                    (self.mix(io).map(|s| s as i16), io[0x89] >> 6)
                });
                self.pwm.advance(ticks as usize, edge);
            }
            if span == to_output {
                if self.output.len() == MAX_QUEUED {
                    self.output.pop_front();
                }
                self.output.push_back(self.pwm.sample());
            }
            self.phase %= BASE_PWM_PERIOD;
        }
    }
}

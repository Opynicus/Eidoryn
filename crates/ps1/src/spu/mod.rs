//! Original 24-voice SPU, 44.1 kHz mixer and sound-RAM transfer implementation.
//! Register reference: https://psx-spx.consoledev.net/soundprocessingunitspu/
use crate::EmuError;
use std::collections::VecDeque;
mod envelope;
mod gaussian;
mod reverb;
mod voice;
use envelope::{Envelope, Phase};
use reverb::Reverb;
use voice::Voice;

pub const SAMPLE_RATE: u32 = 44_100;
pub const SAMPLE_CLOCKS: u32 = 768;
pub type StereoFrame = [i16; 2];

const RAM_SIZE: usize = 512 * 1024;
const CONTROL: usize = 0x1aa / 2;
const RAM_CONTROL: usize = 0x1ac / 2;
/// Approximate control latch delay, expressed in CPU-clock cycles.
pub const CONTROL_CLOCKS: u32 = 768;

#[derive(Clone)]
pub struct Spu {
    registers: [u16; 256],
    ram: Vec<u8>,
    fifo: VecDeque<u16>,
    cursor: usize,
    applied: u16,
    control_delay: u32,
    transfer_clock: u32,
    irq: bool,
    voices: [Voice; 24],
    main_volume: [Envelope; 2],
    reverb: Reverb,
    sample_clock: u32,
    capture_cursor: usize,
    noise_timer: i32,
    noise_level: u16,
    end_flags: u32,
    output: VecDeque<StereoFrame>,
    pub generated_samples: u64,
    pub nonzero_samples: u64,
    pub transferred_halfwords: u64,
}
impl Default for Spu {
    fn default() -> Self {
        Self {
            registers: [0; 256],
            ram: vec![0; RAM_SIZE],
            fifo: VecDeque::new(),
            cursor: 0,
            applied: 0,
            control_delay: 0,
            transfer_clock: 0,
            irq: false,
            voices: std::array::from_fn(|_| Voice::default()),
            main_volume: [Envelope::default(); 2],
            reverb: Reverb::default(),
            sample_clock: 0,
            capture_cursor: 0,
            noise_timer: 0,
            noise_level: 1,
            end_flags: 0,
            output: VecDeque::with_capacity(8192),
            generated_samples: 0,
            nonzero_samples: 0,
            transferred_halfwords: 0,
        }
    }
}
impl Spu {
    pub fn irq_pending(&self) -> bool {
        self.irq
    }
    pub fn status(&self) -> u16 {
        let mode = (self.applied >> 4) & 3;
        (self.applied & 0x3f)
            | (u16::from(self.irq) << 6)
            | if mode == 2 {
                0x180
            } else if mode == 3 {
                0x280
            } else {
                0
            }
            | (u16::from(mode == 1 && !self.fifo.is_empty()) << 10)
            | (u16::from(self.capture_cursor >= 512 && self.registers[RAM_CONTROL] & 0x0c != 0)
                << 11)
    }
    pub fn read(&self, address: u32) -> u16 {
        match address & 0x3fe {
            0x1ae => self.status(),
            0x19c => self.end_flags as u16,
            0x19e => (self.end_flags >> 16) as u16,
            0x1b8 => self.main_volume[0].level as u16,
            0x1ba => self.main_volume[1].level as u16,
            offset @ 0x200..=0x25e => {
                self.voices[((offset - 0x200) / 4) as usize].volumes[(offset as usize >> 1) & 1]
                    .level as u16
            }
            offset @ 0..=0x1fe => self.registers[offset as usize / 2],
            _ => 0,
        }
    }
    pub fn write(&mut self, address: u32, value: u16) -> Result<(), EmuError> {
        let offset = address as usize & 0x3fe;
        match offset {
            0x1ae | 0x19c | 0x19e | 0x200..=0x3fe => return Ok(()),
            0x188 | 0x18a | 0x18c | 0x18e => {
                let first = if offset & 2 == 0 { 0 } else { 16 };
                for index in first..(first + 16).min(24) {
                    if value & (1 << (index - first)) != 0 {
                        if offset & 4 == 0 {
                            self.voices[index].key_on(self.registers[index * 8 + 3]);
                            self.end_flags &= !(1 << index);
                        } else {
                            self.voices[index].key_off();
                        }
                    }
                }
            }
            0x1a2 => self.reverb.cursor = usize::from(value) * 8,
            0x1a6 => self.cursor = usize::from(value) * 8,
            0x1a8 => {
                if self.fifo.len() == 32 {
                    return Err(EmuError::Spu("manual-write FIFO is full"));
                }
                self.fifo.push_back(value);
            }
            0x1aa => {
                self.control_delay = CONTROL_CLOCKS;
                if value & 0x40 == 0 {
                    self.irq = false;
                }
            }
            _ => {}
        }
        if offset < 0x180 && offset & 15 == 12 {
            self.voices[offset / 16].envelope.level = i32::from(value as i16);
        }
        self.registers[offset / 2] = value;
        Ok(())
    }
    #[inline]
    pub fn tick(&mut self, clocks: u32) -> Result<(), EmuError> {
        self.tick_with_cd(clocks, || [0; 2])
    }

    #[inline]
    pub fn tick_with_cd(
        &mut self,
        clocks: u32,
        mut cd_sample: impl FnMut() -> StereoFrame,
    ) -> Result<(), EmuError> {
        // Do not apply the whole elapsed interval to a newly latched transfer mode.
        let transfer_clocks = clocks.saturating_sub(self.control_delay);
        if self.control_delay != 0 {
            self.control_delay = self.control_delay.saturating_sub(clocks);
            if self.control_delay == 0 {
                self.applied = self.registers[CONTROL] & 0x3f;
            }
        }
        if self.applied & 0x30 == 0x10 && !self.fifo.is_empty() {
            self.check_ram_config()?;
            self.transfer_clock = self.transfer_clock.saturating_add(transfer_clocks);
            while self.transfer_clock >= 16 {
                let Some(value) = self.fifo.pop_front() else {
                    self.transfer_clock = 0;
                    break;
                };
                self.store_halfword(value);
                self.transfer_clock -= 16;
            }
        } else {
            self.transfer_clock = 0;
        }
        let samples = (u64::from(self.sample_clock) + u64::from(clocks)) / u64::from(SAMPLE_CLOCKS);
        self.sample_clock =
            ((u64::from(self.sample_clock) + u64::from(clocks)) % u64::from(SAMPLE_CLOCKS)) as u32;
        for _ in 0..samples {
            self.mix_sample(cd_sample());
        }
        Ok(())
    }

    pub fn drain_audio(&mut self) -> impl Iterator<Item = StereoFrame> + '_ {
        self.output.drain(..)
    }

    fn mix_sample(&mut self, cd: StereoFrame) {
        let control = self.registers[CONTROL];
        let irq_address =
            (control & 0x8040 == 0x8040).then(|| usize::from(self.registers[0x1a4 / 2]) * 8);
        let flags = |offset: usize| {
            u32::from(self.registers[offset / 2])
                | (u32::from(self.registers[offset / 2 + 1]) << 16)
        };
        let modulation = flags(0x190);
        let noise = flags(0x194);
        let echo = flags(0x198);
        let mut mixed = [0_i32; 2];
        let mut echo_input = [0_i32; 2];
        let mut captures = [0_i16; 2];
        let mut previous = 0;
        self.clock_noise(control);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            let registers = &mut self.registers[index * 8..index * 8 + 8];
            if control & 0x8000 == 0 {
                voice.phase = Phase::Off;
                voice.envelope.level = 0;
            }
            let (sample, ended, irq) = voice.sample(
                registers,
                &self.ram,
                (index > 0 && modulation & (1 << index) != 0).then_some(previous),
                (noise & (1 << index) != 0).then_some(i32::from(self.noise_level as i16)),
                irq_address,
            );
            previous = sample;
            if index == 1 || index == 3 {
                captures[index / 2] = sample as i16;
            }
            if ended {
                self.end_flags |= 1 << index;
            }
            self.irq |= irq;
            for side in 0..2 {
                let output = (sample * voice.volumes[side].volume(registers[side])) >> 15;
                if control & 0x4000 != 0 {
                    mixed[side] += output;
                    if echo & (1 << index) != 0 {
                        echo_input[side] += output;
                    }
                }
            }
        }
        if self.applied & 1 != 0 {
            for side in 0..2 {
                let sample = (i32::from(cd[side])
                    * i32::from(self.registers[0x1b0 / 2 + side] as i16))
                    >> 15;
                mixed[side] += sample;
                if self.applied & 4 != 0 {
                    echo_input[side] += sample;
                }
            }
        }
        for (channel, sample) in [cd[0], cd[1], captures[0], captures[1]]
            .into_iter()
            .enumerate()
        {
            let address = channel * 1024 + self.capture_cursor;
            self.irq |= irq_address.is_some_and(|irq| irq == address & !7);
            self.ram[address..address + 2].copy_from_slice(&sample.to_le_bytes());
        }
        self.capture_cursor = (self.capture_cursor + 2) & 1023;
        let (echo_output, irq) = self.reverb.sample(
            echo_input.map(|s| s.clamp(-32768, 32767)),
            &self.registers[0x1c0 / 2..],
            &mut self.ram,
            control & 0x80 != 0,
            usize::from(self.registers[0x1a2 / 2]) * 8,
            irq_address,
        );
        self.irq |= irq;
        let output = std::array::from_fn(|side| {
            mixed[side] +=
                (echo_output[side] * i32::from(self.registers[0x184 / 2 + side] as i16)) >> 15;
            let volume = self.main_volume[side].volume(self.registers[0x180 / 2 + side]);
            ((mixed[side].clamp(-32768, 32767) * volume) >> 15) as i16
        });
        self.generated_samples += 1;
        self.nonzero_samples += u64::from(output != [0; 2]);
        if self.output.len() == 8192 {
            self.output.pop_front();
        }
        self.output.push_back(output);
    }
    fn clock_noise(&mut self, control: u16) {
        self.noise_timer -= 4 + i32::from((control >> 8) & 3);
        if self.noise_timer < 0 {
            let parity = ((self.noise_level >> 15)
                ^ (self.noise_level >> 12)
                ^ (self.noise_level >> 11)
                ^ (self.noise_level >> 10)
                ^ 1)
                & 1;
            self.noise_level = (self.noise_level << 1) | parity;
            let period = 0x20000 >> ((control >> 10) & 15);
            self.noise_timer += period;
            if self.noise_timer < 0 {
                self.noise_timer += period;
            }
        }
    }

    pub fn dma_ready(&self, from_ram: bool) -> bool {
        self.control_delay == 0 && (self.applied >> 4) & 3 == if from_ram { 2 } else { 3 }
    }
    fn check_ram_config(&self) -> Result<(), EmuError> {
        if self.registers[RAM_CONTROL] != 4 {
            return Err(EmuError::Spu(
                "only standard 512 KiB sound RAM configuration is implemented",
            ));
        }
        Ok(())
    }
    fn advance(&mut self) {
        if self.registers[CONTROL] & 0x8040 == 0x8040
            && self.cursor / 8 == usize::from(self.registers[0x1a4 / 2])
        {
            self.irq = true;
        }
        self.cursor = (self.cursor + 2) & (RAM_SIZE - 1);
        self.transferred_halfwords += 1;
    }
    fn store_halfword(&mut self, value: u16) {
        self.ram[self.cursor..self.cursor + 2].copy_from_slice(&value.to_le_bytes());
        self.advance();
    }
    pub fn dma_write(&mut self, value: u32) -> Result<(), EmuError> {
        self.check_ram_config()?;
        self.store_halfword(value as u16);
        self.store_halfword((value >> 16) as u16);
        Ok(())
    }
    pub fn dma_read(&mut self) -> Result<u32, EmuError> {
        self.check_ram_config()?;
        let mut words = [0; 2];
        for word in &mut words {
            *word = u16::from_le_bytes([self.ram[self.cursor], self.ram[self.cursor + 1]]);
            self.advance();
        }
        Ok(u32::from(words[0]) | (u32::from(words[1]) << 16))
    }
}

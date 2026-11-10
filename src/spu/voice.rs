use super::{
    envelope::{Envelope, Phase, adsr},
    gaussian::GAUSS,
};
use crate::adpcm::Predictor;

#[derive(Clone, Default)]
pub(super) struct Voice {
    pub envelope: Envelope,
    pub phase: Phase,
    pub volumes: [Envelope; 2],
    predictor: Predictor,
    decoded: [i16; 28],
    history: [i32; 4],
    position: usize,
    address: usize,
    fraction: u32,
    flags: u8,
    loaded: bool,
}

impl Voice {
    pub fn key_on(&mut self, address: u16) {
        let volumes = self.volumes;
        *self = Self {
            volumes,
            address: usize::from(address) * 8,
            phase: Phase::Attack,
            ..Self::default()
        };
    }

    pub fn key_off(&mut self) {
        if self.phase != Phase::Off {
            self.phase = Phase::Release;
            self.envelope.reset_counter();
        }
    }

    fn load(&mut self, ram: &[u8], repeat: &mut u16, irq_address: Option<usize>) -> bool {
        let header = ram[self.address];
        self.flags = ram[(self.address + 1) & 0x7ffff];
        if self.flags & 4 != 0 {
            *repeat = (self.address / 8) as u16;
        }
        for (index, sample) in self.decoded.iter_mut().enumerate() {
            let byte = ram[(self.address + 2 + index / 2) & 0x7ffff];
            let nibble = (byte >> ((index & 1) * 4)) & 15;
            *sample = self.predictor.decode(
                i32::from((u16::from(nibble) << 12) as i16),
                header & 15,
                usize::from(header >> 4),
            );
        }
        self.position = 0;
        self.loaded = true;
        irq_address.is_some_and(|irq| (irq.wrapping_sub(self.address) & 0x7ffff) < 16)
    }

    /// Returns post-envelope mono output, loop-end event, and RAM IRQ match.
    pub fn sample(
        &mut self,
        registers: &mut [u16],
        ram: &[u8],
        modulation: Option<i32>,
        noise: Option<i32>,
        irq_address: Option<usize>,
    ) -> (i32, bool, bool) {
        if self.phase == Phase::Off {
            self.envelope.level = 0;
            registers[6] = 0;
        }
        let mut irq = false;
        let mut ended = false;
        if !self.loaded {
            irq |= self.load(ram, &mut registers[7], irq_address);
            self.history[3] = i32::from(self.decoded[0]);
        }
        let index = ((self.fraction >> 4) & 255) as usize;
        let value = if self.envelope.level == 0 {
            0
        } else {
            noise.unwrap_or_else(|| {
                let coefficients = [
                    GAUSS[255 - index],
                    GAUSS[511 - index],
                    GAUSS[256 + index],
                    GAUSS[index],
                ];
                self.history
                    .iter()
                    .zip(coefficients)
                    .map(|(sample, weight)| (sample * weight) >> 15)
                    .sum::<i32>()
            })
        };
        let output = (value * self.envelope.level) >> 15;
        adsr(
            &mut self.envelope,
            &mut self.phase,
            registers[4],
            registers[5],
        );
        registers[6] = self.envelope.level as u16;
        let pitch = modulation
            .map_or(u32::from(registers[2]), |previous| {
                ((i32::from(registers[2] as i16) * (previous + 32768)) >> 15) as u32 & 0xffff
            })
            .min(0x4000);
        self.fraction += pitch;
        while self.fraction >= 0x1000 {
            self.fraction -= 0x1000;
            self.position += 1;
            if self.position == 28 {
                if self.flags & 1 != 0 {
                    ended = true;
                    self.address = usize::from(registers[7]) * 8;
                    if self.flags & 2 == 0 {
                        self.envelope.level = 0;
                        self.phase = Phase::Off;
                        registers[6] = 0;
                    }
                } else {
                    self.address = (self.address + 16) & 0x7ffff;
                }
                irq |= self.load(ram, &mut registers[7], irq_address);
            }
            self.history.rotate_left(1);
            self.history[3] = i32::from(self.decoded[self.position]);
        }
        (output.clamp(-32768, 32767), ended, irq)
    }
}

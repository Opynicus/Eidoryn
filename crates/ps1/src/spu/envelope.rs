//! Per-sample envelope counters shared by ADSR and stereo volume sweeps.
#[derive(Clone, Copy, Default)]
pub(super) struct Envelope {
    pub level: i32,
    counter: u32,
}

impl Envelope {
    pub fn reset_counter(&mut self) {
        self.counter = 0;
    }

    pub fn advance(&mut self, rate: u8, decrease: bool, exponential: bool, negative: bool) {
        let shift = u32::from(rate >> 2);
        let mut step = 7 - i32::from(rate & 3);
        if decrease ^ negative {
            step = !step;
        }
        step <<= 11_u32.saturating_sub(shift);
        let mut increment = 0x8000 >> shift.saturating_sub(11);
        if exponential && !decrease && self.level > 0x6000 {
            match shift {
                0..=9 => step >>= 2,
                10 => {
                    step >>= 1;
                    increment >>= 1;
                }
                _ => increment >>= 2,
            }
        } else if exponential && decrease {
            step = (step * self.level) >> 15;
        }
        if rate != 0x7f {
            increment = increment.max(1);
        }
        self.counter += increment;
        if self.counter < 0x8000 {
            return;
        }
        self.counter &= 0x7fff;
        self.level = if decrease {
            if negative {
                (self.level + step).clamp(-32768, 0)
            } else {
                (self.level + step).max(0)
            }
        } else {
            (self.level + step).clamp(-32768, 32767)
        };
    }

    pub fn volume(&mut self, register: u16) -> i32 {
        if register & 0x8000 == 0 {
            self.level = i32::from((register << 1) as i16);
            self.counter = 0;
        } else {
            self.advance(
                (register & 0x7f) as u8,
                register & 0x2000 != 0,
                register & 0x4000 != 0,
                register & 0x1000 != 0,
            );
        }
        self.level
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Phase {
    #[default]
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

pub(super) fn adsr(envelope: &mut Envelope, phase: &mut Phase, low: u16, high: u16) {
    let (rate, decrease, exponential) = match *phase {
        Phase::Off => return,
        Phase::Attack => (((low >> 8) & 0x7f) as u8, false, low & 0x8000 != 0),
        Phase::Decay => (((low >> 4) & 15) as u8 * 4, true, true),
        Phase::Sustain => (
            ((high >> 6) & 0x7f) as u8,
            high & 0x4000 != 0,
            high & 0x8000 != 0,
        ),
        Phase::Release => (
            if high & 31 == 31 {
                0x7f
            } else {
                (high & 31) as u8 * 4
            },
            true,
            high & 0x20 != 0,
        ),
    };
    envelope.advance(rate, decrease, exponential, false);
    let next = match *phase {
        Phase::Attack if envelope.level >= 0x7fff => Phase::Decay,
        Phase::Decay if envelope.level <= (i32::from(low & 15) + 1) * 0x800 => Phase::Sustain,
        Phase::Release if envelope.level <= 0 => Phase::Off,
        phase => phase,
    };
    if *phase != next {
        envelope.reset_counter();
        *phase = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_decay_sustain_release_reaches_silence() {
        let mut envelope = Envelope::default();
        let mut phase = Phase::Attack;
        for _ in 0..3 {
            adsr(&mut envelope, &mut phase, 0, 0x1fc0);
        }
        assert_eq!(envelope.level, 32767);
        assert!(phase == Phase::Decay);
        for _ in 0..4 {
            adsr(&mut envelope, &mut phase, 0, 0x1fc0);
        }
        assert!(phase == Phase::Sustain);
        let sustained = envelope.level;
        for _ in 0..100_000 {
            adsr(&mut envelope, &mut phase, 0, 0x1fc0);
        }
        assert_eq!(envelope.level, sustained);
        phase = Phase::Release;
        adsr(&mut envelope, &mut phase, 0, 0x1fc0);
        assert_eq!(envelope.level, 0);
        assert!(phase == Phase::Off);
    }

    #[test]
    fn signed_volume_sweeps_saturate_and_frozen_rate_retains_sign() {
        let mut envelope = Envelope::default();
        assert_eq!(envelope.volume(0x6000), -16384);
        for _ in 0..100_000 {
            envelope.volume(0x807f);
        }
        assert_eq!(envelope.level, -16384);
        for _ in 0..4 {
            envelope.volume(0x9000); // Negative phase, increasing magnitude.
        }
        assert_eq!(envelope.level, -32768);
        for _ in 0..4 {
            envelope.volume(0xb000); // Negative phase, decreasing magnitude.
        }
        assert_eq!(envelope.level, 0);
        for _ in 0..4 {
            envelope.volume(0x8000);
        }
        assert_eq!(envelope.level, 32767);
    }
}

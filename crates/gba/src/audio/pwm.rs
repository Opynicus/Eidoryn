//! Convert the four hardware PWM rates onto the core's 65,536 Hz PCM clock.
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

// One tick is 64 master clocks, the shortest PWM period. Sixteen output frames
// of history on either side let all modes use the same presentation timestamp.
const DELAY: usize = 64;
const LENGTH: usize = 2 * DELAY;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Latch {
    pcm: [i16; 2],
    mode: u8,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct PwmOutput {
    history: [[Latch; 8]; LENGTH / 8],
    cursor: usize,
}

// Symmetric Blackman-windowed sinc. Normalize each phase to unity DC gain.
fn kernel<const N: usize>(distance: impl Fn(usize) -> f64) -> [f32; N] {
    let mut taps = std::array::from_fn(|i| {
        let x = distance(i);
        let angle = std::f64::consts::PI * x;
        let sinc = if x == 0.0 { 1.0 } else { angle.sin() / angle };
        let window = 0.42 + 0.5 * (angle / 8.0).cos() + 0.08 * (2.0 * angle / 8.0).cos();
        (sinc * window) as f32
    });
    let center = if distance(0) == 0.0 { taps[0] } else { 0.0 };
    let total = 2.0 * taps.iter().sum::<f32>() - center;
    for tap in &mut taps {
        *tap /= total;
    }
    taps
}

static MIDPOINT: LazyLock<[f32; 8]> = LazyLock::new(|| kernel(|i| i as f64 + 0.5));
static HALF_RATE: LazyLock<[f32; 17]> = LazyLock::new(|| kernel(|i| i as f64 / 2.0));
static QUARTER_RATE: LazyLock<[f32; 33]> = LazyLock::new(|| kernel(|i| i as f64 / 4.0));

impl PwmOutput {
    pub(super) fn valid(&self, phase: u32) -> bool {
        self.cursor < LENGTH
            && self.cursor % 8 == (phase / 64) as usize
            && self.history.iter().flatten().all(|latch| latch.mode <= 3)
    }

    fn at(&self, index: usize) -> Latch {
        self.history.as_flattened()[index & (LENGTH - 1)]
    }

    #[cfg(test)]
    pub(super) fn latched_pcm(&self) -> [i16; 2] {
        self.at(self.cursor).pcm
    }

    /// Hold the previous DAC value between edges, including across a mode write.
    pub(super) fn advance(&mut self, ticks: usize, edge: Option<([i16; 2], u8)>) {
        let held = self.at(self.cursor);
        for _ in 0..ticks {
            self.cursor = (self.cursor + 1) & (LENGTH - 1);
            self.history.as_flattened_mut()[self.cursor] = held;
        }
        if let Some((pcm, mode)) = edge {
            self.history.as_flattened_mut()[self.cursor] = Latch { pcm, mode };
        }
    }

    pub(super) fn sample(&self) -> [i16; 2] {
        let center = self.cursor + DELAY;
        let latch = self.at(center);
        match latch.mode {
            0 if !center.is_multiple_of(8) => self.filtered(center, 4, 8, &*MIDPOINT),
            0 | 1 => latch.pcm,
            2 => self.filtered(center, 0, 2, &*HALF_RATE),
            _ => self.filtered(center, 0, 1, &*QUARTER_RATE),
        }
    }

    // Paired taps are equidistant in master clocks. For decimation, count
    // the center once and low-pass before discarding higher-rate samples.
    fn filtered(&self, center: usize, offset: usize, stride: usize, taps: &[f32]) -> [i16; 2] {
        let mut sum = [0.0; 2];
        for (i, &tap) in taps.iter().enumerate() {
            let d = offset + i * stride;
            let left = self.at(center - d).pcm;
            let right = if d == 0 {
                [0; 2]
            } else {
                self.at(center + d).pcm
            };
            for side in 0..2 {
                sum[side] += (f32::from(left[side]) + f32::from(right[side])) * tap;
            }
        }
        sum.map(|s| s.round() as i16)
    }
}

#[cfg(test)]
mod tests;

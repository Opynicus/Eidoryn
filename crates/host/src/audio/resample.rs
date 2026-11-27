//! Polyphase windowed-sinc conversion between source and device sample rates.
use super::{MAX_CLOCK_CORRECTION, StereoFrame};
use std::{collections::VecDeque, f64::consts::PI};

const TAPS: usize = 64;
const PAST: usize = TAPS / 2 - 1;
const PHASES: usize = 512;

pub(super) struct SincResampler {
    pub(super) bandwidth_rate: u32,
    coefficients: Box<[[f32; TAPS]]>,
    history: [StereoFrame; PAST],
    primed: bool,
}

impl SincResampler {
    pub(super) fn new(source_rate: u32, bandwidth_rate: u32) -> Self {
        // Leave room for the fastest queue clock when filtering to a lower device rate.
        let cutoff = if bandwidth_rate < source_rate {
            f64::from(bandwidth_rate) / (f64::from(source_rate) * (1.0 + MAX_CLOCK_CORRECTION))
        } else {
            1.0
        };
        let coefficients = (0..=PHASES)
            .map(|phase| {
                let offset = phase as f64 / PHASES as f64;
                let row: [f64; TAPS] = std::array::from_fn(|i| {
                    let x = i as f64 - PAST as f64 - offset;
                    let a = PI * x / (TAPS / 2) as f64;
                    let window = 0.42 + 0.5 * a.cos() + 0.08 * (2.0 * a).cos();
                    let sinc = if x == 0.0 {
                        cutoff
                    } else {
                        (PI * x * cutoff).sin() / (PI * x)
                    };
                    window * sinc
                });
                let sum: f64 = row.iter().sum();
                row.map(|weight| (weight / sum) as f32)
            })
            .collect();
        Self {
            bandwidth_rate,
            coefficients,
            history: [[0; 2]; PAST],
            primed: false,
        }
    }

    pub(super) fn reset(&mut self) {
        self.primed = false;
    }

    pub(super) fn consume(&mut self, frame: StereoFrame) {
        self.history.rotate_left(1);
        self.history[PAST - 1] = frame;
    }

    pub(super) fn sample(&mut self, frames: &VecDeque<StereoFrame>, fraction: f64) -> [f32; 2] {
        let first = frames[0];
        if !self.primed {
            self.history.fill(first);
            self.primed = true;
        }
        let weights = &self.coefficients[(fraction * PHASES as f64).round() as usize];
        // Extend the nearest sample at either stream boundary. Accumulating
        // differences keeps a constant signal exactly constant at every phase.
        let center = first.map(f32::from);
        let mut sum = center;
        let available = self.history.iter().chain(frames);
        let last = frames.back().expect("resampling requires queued audio");
        for (weight, frame) in weights.iter().zip(available.chain(std::iter::repeat(last))) {
            sum[0] += weight * (f32::from(frame[0]) - center[0]);
            sum[1] += weight * (f32::from(frame[1]) - center[1]);
        }
        sum
    }
}

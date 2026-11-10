//! Bounded host audio transport. Device callbacks never wait for the interpreter.
use crate::spu::{SAMPLE_RATE, StereoFrame};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[cfg(feature = "desktop")]
pub mod output;
pub mod wav;

const CAPACITY: usize = 4096;
const PREBUFFER: usize = 1024;
const TARGET: f64 = 1536.0;

#[derive(Clone, Copy, Default)]
pub struct AudioStats {
    pub queued_frames: usize,
    pub underruns: u64,
    pub dropped_frames: u64,
    pub callback_misses: u64,
}

struct Queue {
    frames: VecDeque<StereoFrame>,
    playing: bool,
    primed: bool,
    fraction: f64,
    volume: u8,
    stats: AudioStats,
}

#[derive(Clone)]
pub struct AudioBuffer(Arc<Transport>);

struct Transport {
    queue: Mutex<Queue>,
    callback_misses: AtomicU64,
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self(Arc::new(Transport {
            queue: Mutex::new(Queue {
                frames: VecDeque::with_capacity(CAPACITY),
                playing: false,
                primed: false,
                fraction: 0.0,
                volume: 75,
                stats: AudioStats::default(),
            }),
            callback_misses: AtomicU64::new(0),
        }))
    }
}

impl AudioBuffer {
    pub fn set_playing(&self, playing: bool) {
        if let Ok(mut queue) = self.0.queue.lock()
            && queue.playing != playing
        {
            queue.frames.clear();
            queue.primed = false;
            queue.fraction = 0.0;
            queue.playing = playing;
        }
    }
    pub fn clear(&self) {
        if let Ok(mut queue) = self.0.queue.lock() {
            queue.frames.clear();
            queue.primed = false;
            queue.fraction = 0.0;
        }
    }
    pub fn set_volume(&self, percent: u8) {
        if let Ok(mut queue) = self.0.queue.lock() {
            queue.volume = percent.min(100);
        }
    }
    pub fn stats(&self) -> AudioStats {
        self.0.queue.lock().map_or_else(
            |_| AudioStats::default(),
            |queue| AudioStats {
                queued_frames: queue.frames.len(),
                callback_misses: self.0.callback_misses.load(Ordering::Relaxed),
                ..queue.stats
            },
        )
    }
    pub fn push(&self, samples: impl IntoIterator<Item = StereoFrame>) {
        if let Ok(mut queue) = self.0.queue.lock()
            && queue.playing
        {
            for frame in samples {
                if queue.frames.len() == CAPACITY {
                    queue.frames.pop_front();
                    queue.stats.dropped_frames += 1;
                    queue.fraction = 0.0;
                }
                queue.frames.push_back(frame);
            }
        }
    }

    /// Converts to the device's sample format without allocating in its callback.
    pub fn render<T: Copy>(
        &self,
        output: &mut [T],
        channels: usize,
        rate: u32,
        convert: impl Fn(f32) -> T,
    ) {
        output.fill(convert(0.0));
        if channels == 0 || rate == 0 {
            return;
        }
        let Ok(mut queue) = self.0.queue.try_lock() else {
            self.0.callback_misses.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if !queue.playing {
            return;
        }
        if !queue.primed {
            if queue.frames.len() < PREBUFFER {
                return;
            }
            queue.primed = true;
        }
        // Small clock drift correction keeps a bounded queue across independent DAC/video clocks.
        let correction =
            ((queue.frames.len() as f64 - TARGET) / TARGET * 0.005).clamp(-0.005, 0.005);
        let step = f64::from(SAMPLE_RATE) / f64::from(rate) * (1.0 + correction);
        let gain = f32::from(queue.volume) / (100.0 * 32768.0);
        for frame in output.chunks_exact_mut(channels) {
            if queue.frames.len() < 2 {
                queue.stats.underruns += 1;
                queue.primed = false;
                queue.fraction = 0.0;
                break;
            }
            let fraction = queue.fraction as f32;
            let sample: [f32; 2] = std::array::from_fn(|side| {
                (f32::from(queue.frames[0][side]) * (1.0 - fraction)
                    + f32::from(queue.frames[1][side]) * fraction)
                    * gain
            });
            if channels == 1 {
                frame[0] = convert(f32::midpoint(sample[0], sample[1]));
            } else {
                frame[0] = convert(sample[0]);
                frame[1] = convert(sample[1]);
            }
            queue.fraction += step;
            while queue.fraction >= 1.0 && !queue.frames.is_empty() {
                queue.fraction -= 1.0;
                queue.frames.pop_front();
            }
        }
    }
}

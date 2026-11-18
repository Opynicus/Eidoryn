//! Bounded host audio transport. Device callbacks never wait for the interpreter.
use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[cfg(feature = "audio-output")]
pub mod output;
pub mod wav;

pub type StereoFrame = [i16; 2];
/// Compatibility default; new systems should supply their own PCM rate.
pub const DEFAULT_SAMPLE_RATE: NonZeroU32 = NonZeroU32::new(44_100).unwrap();

const CAPACITY: usize = 4096;
const PREBUFFER: usize = 1024;
const TARGET: f64 = 1536.0;
// Bounds callback stack storage to 8 KiB, independently of device buffer size.
const RENDER_FRAMES: usize = 1024;

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
    generation: u64,
    volume: u8,
    stats: AudioStats,
}

#[derive(Clone)]
pub struct AudioBuffer(Arc<Transport>);

struct Transport {
    sample_rate: NonZeroU32,
    queue: Mutex<Queue>,
    callback_misses: AtomicU64,
}

#[derive(Clone, Copy)]
struct RenderParameters {
    step: f64,
    gain: f32,
    generation: u64,
}

impl Queue {
    fn clear(&mut self) {
        self.frames.clear();
        self.primed = false;
        self.fraction = 0.0;
        self.generation = self.generation.wrapping_add(1);
    }

    fn fill_pcm(&mut self, pcm: &mut [[f32; 2]], parameters: RenderParameters) -> usize {
        for (index, sample) in pcm.iter_mut().enumerate() {
            if self.frames.len() < 2 {
                self.stats.underruns += 1;
                self.primed = false;
                self.fraction = 0.0;
                return index;
            }
            let fraction = self.fraction as f32;
            *sample = std::array::from_fn(|side| {
                (f32::from(self.frames[0][side]) * (1.0 - fraction)
                    + f32::from(self.frames[1][side]) * fraction)
                    * parameters.gain
            });
            self.fraction += parameters.step;
            while self.fraction >= 1.0 && !self.frames.is_empty() {
                self.fraction -= 1.0;
                self.frames.pop_front();
            }
        }
        pcm.len()
    }
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self::with_sample_rate(DEFAULT_SAMPLE_RATE)
    }
}

impl AudioBuffer {
    /// Creates an independent queue for a system's interleaved stereo i16 PCM.
    /// The source rate stays fixed for the queue's lifetime, including its clones.
    pub fn with_sample_rate(sample_rate: NonZeroU32) -> Self {
        Self(Arc::new(Transport {
            sample_rate,
            queue: Mutex::new(Queue {
                frames: VecDeque::with_capacity(CAPACITY),
                playing: false,
                primed: false,
                fraction: 0.0,
                generation: 0,
                volume: 75,
                stats: AudioStats::default(),
            }),
            callback_misses: AtomicU64::new(0),
        }))
    }

    pub fn set_playing(&self, playing: bool) {
        if let Ok(mut queue) = self.0.queue.lock()
            && queue.playing != playing
        {
            queue.clear();
            queue.playing = playing;
        }
    }
    pub fn clear(&self) {
        if let Ok(mut queue) = self.0.queue.lock() {
            queue.clear();
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
    /// Enqueues prepared PCM as one batch, dropping the oldest frames on overflow.
    /// Paused queues discard the batch. Prepare samples before calling this method;
    /// no caller-provided iterator runs while the queue is locked.
    pub fn push(&self, samples: &[StereoFrame]) {
        if let Ok(mut queue) = self.0.queue.lock()
            && queue.playing
        {
            for &frame in samples {
                if queue.frames.len() == CAPACITY {
                    queue.frames.pop_front();
                    queue.stats.dropped_frames += 1;
                    queue.fraction = 0.0;
                }
                queue.frames.push_back(frame);
            }
        }
    }

    /// Renders without allocating or waiting for the queue lock. `convert` always
    /// runs outside the lock; it must also avoid allocation/blocking for device use.
    ///
    /// PCM is prepared in blocks of at most 1024 frames (8 KiB of stack storage).
    /// The initial silence conversion precedes queue access. Gain and drift
    /// correction are then sampled once per call, so volume changes during PCM
    /// conversion take effect on the next call. Clear or playback changes
    /// stop consumption after the prepared block; the remainder stays silent and
    /// newly queued audio waits for the next call. Enqueues between blocks are
    /// visible to the next block, with the usual overflow and fraction reset rules.
    /// A missed lock silences the rest of this call and counts one callback miss.
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
        let mut pcm = [[0.0; 2]; RENDER_FRAMES];
        let mut parameters: Option<RenderParameters> = None;
        let mut output_frames = output.chunks_exact_mut(channels);
        loop {
            let requested = output_frames.len().min(RENDER_FRAMES);
            let count = {
                let Ok(mut queue) = self.0.queue.try_lock() else {
                    self.0.callback_misses.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let settings = if let Some(settings) = parameters {
                    if queue.generation != settings.generation {
                        return;
                    }
                    settings
                } else {
                    if !queue.playing || (!queue.primed && queue.frames.len() < PREBUFFER) {
                        return;
                    }
                    queue.primed = true;
                    // Freeze clock correction across every block in this callback.
                    let correction = ((queue.frames.len() as f64 - TARGET) / TARGET * 0.005)
                        .clamp(-0.005, 0.005);
                    let settings = RenderParameters {
                        step: f64::from(self.0.sample_rate.get()) / f64::from(rate)
                            * (1.0 + correction),
                        gain: f32::from(queue.volume) / (100.0 * 32768.0),
                        generation: queue.generation,
                    };
                    parameters = Some(settings);
                    settings
                };
                queue.fill_pcm(&mut pcm[..requested], settings)
            };
            for (sample, frame) in pcm[..count].iter().zip(output_frames.by_ref()) {
                if channels == 1 {
                    frame[0] = convert(f32::midpoint(sample[0], sample[1]));
                } else {
                    frame[0] = convert(sample[0]);
                    frame[1] = convert(sample[1]);
                }
            }
            if count < requested || output_frames.len() == 0 {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;

//! Bounded host audio transport. Device callbacks never wait for the interpreter.
use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
#[cfg(feature = "audio-output")]
pub mod output;
mod resample;
pub mod wav;
use resample::SincResampler;

pub type StereoFrame = [i16; 2];
/// Compatibility default; new systems should supply their own PCM rate.
pub const DEFAULT_SAMPLE_RATE: NonZeroU32 = NonZeroU32::new(44_100).unwrap();

const CAPACITY: usize = 4096;
const PREBUFFER: usize = 1024;
const TARGET: f64 = 1536.0;
const MAX_CLOCK_CORRECTION: f64 = 0.005;
// Limit even a misreported device period to 64 KiB of source PCM.
const MAX_DEVICE_FRAMES: usize = 16_384;
// A normal frame push copies a few KiB. Cover that short critical section
// without sleeping or waiting indefinitely for a preempted producer.
const DEVICE_LOCK_RETRIES: usize = 256;
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
    envelope: f32,
    capacity: usize,
    generation: u64,
    volume: u8,
    resampler: Option<SincResampler>,
    stats: AudioStats,
}

#[derive(Clone)]
pub struct AudioBuffer(Arc<Transport>);

struct Transport {
    sample_rate: NonZeroU32,
    device_reserve: Duration,
    queue: Mutex<Queue>,
    callback_misses: AtomicU64,
}

#[derive(Clone, Copy)]
struct RenderParameters {
    step: f64,
    gain: f32,
    generation: u64,
    fade_per_source_frame: f64,
    fade_per_output_frame: f32,
}

#[derive(Clone, Copy)]
struct Buffering {
    prebuffer: usize,
    target: f64,
    capacity: usize,
    lock_retries: usize,
}

impl Buffering {
    const DEFAULT: Self = Self {
        prebuffer: PREBUFFER,
        target: TARGET,
        capacity: CAPACITY,
        lock_retries: 0,
    };

    fn for_device(
        source_rate: u32,
        output_rate: u32,
        output_frames: usize,
        reserve: Duration,
    ) -> Self {
        // The complete callback must fit, with a reserve for the producer's
        // next batch after the device has drained the queue.
        let period =
            (output_frames as f64 * f64::from(source_rate) / f64::from(output_rate)).ceil();
        let reserve = (f64::from(source_rate) * reserve.as_secs_f64()).max((PREBUFFER / 2) as f64);
        let prebuffer = (period + reserve + 1.0)
            .ceil()
            .clamp(PREBUFFER as f64, MAX_DEVICE_FRAMES as f64);
        let target = (prebuffer + (TARGET - PREBUFFER as f64)).min(MAX_DEVICE_FRAMES as f64);
        Self {
            prebuffer: prebuffer as usize,
            target,
            capacity: (target + period)
                .ceil()
                .clamp(CAPACITY as f64, MAX_DEVICE_FRAMES as f64) as usize,
            lock_retries: DEVICE_LOCK_RETRIES,
        }
    }
}

fn try_callback_lock<T>(mut acquire: impl FnMut() -> Option<T>, retries: usize) -> Option<T> {
    if let Some(lock) = acquire() {
        return Some(lock);
    }
    for _ in 0..retries {
        std::hint::spin_loop();
        if let Some(lock) = acquire() {
            return Some(lock);
        }
    }
    None
}

impl Queue {
    fn reset_interpolation(&mut self) {
        self.fraction = 0.0;
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.primed = false;
        self.reset_interpolation();
        self.envelope = 1.0;
        self.generation = self.generation.wrapping_add(1);
    }

    fn fill_pcm(&mut self, pcm: &mut [[f32; 2]], parameters: RenderParameters) -> usize {
        for (index, sample) in pcm.iter_mut().enumerate() {
            if self.frames.len() < 2 {
                self.stats.underruns += 1;
                self.frames.clear();
                self.primed = false;
                self.reset_interpolation();
                self.envelope = 0.0;
                return index;
            }
            // Fade over the last millisecond of available audio. Also ramp up
            // if a producer refills the queue partway through a fade.
            let available = (self.frames.len() - 1) as f64 - self.fraction;
            let max_gain = (available * parameters.fade_per_source_frame).min(1.0) as f32;
            self.envelope = max_gain.min(self.envelope + parameters.fade_per_output_frame);
            let gain = parameters.gain * self.envelope;
            *sample = if let Some(resampler) = &mut self.resampler {
                resampler
                    .sample(&self.frames, self.fraction)
                    .map(|level| (level * gain).clamp(-1.0, 1.0))
            } else {
                let fraction = self.fraction as f32;
                std::array::from_fn(|side| {
                    (f32::from(self.frames[0][side]) * (1.0 - fraction)
                        + f32::from(self.frames[1][side]) * fraction)
                        * gain
                })
            };
            self.fraction += parameters.step;
            while self.fraction >= 1.0 && !self.frames.is_empty() {
                self.fraction -= 1.0;
                if let Some(frame) = self.frames.pop_front()
                    && let Some(resampler) = &mut self.resampler
                {
                    resampler.consume(frame);
                }
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
        Self::with_device_reserve(sample_rate, Duration::ZERO)
    }

    /// Requests extra source audio for uneven frame delivery before starting
    /// live playback. The whole device queue is bounded to 16,384 stereo frames.
    pub fn with_device_reserve(sample_rate: NonZeroU32, reserve: Duration) -> Self {
        Self::new(sample_rate, reserve, None)
    }

    /// Preserves the source passband when converting PCM to a device rate.
    /// Call [`Self::prepare_device_rate`] before rendering to downsampled outputs.
    pub fn with_bandlimited_output(sample_rate: NonZeroU32, reserve: Duration) -> Self {
        Self::new(
            sample_rate,
            reserve,
            Some(SincResampler::new(sample_rate.get(), sample_rate.get())),
        )
    }

    fn new(sample_rate: NonZeroU32, reserve: Duration, resampler: Option<SincResampler>) -> Self {
        Self(Arc::new(Transport {
            sample_rate,
            device_reserve: reserve,
            queue: Mutex::new(Queue {
                frames: VecDeque::with_capacity(CAPACITY),
                playing: false,
                primed: false,
                fraction: 0.0,
                envelope: 1.0,
                capacity: CAPACITY,
                generation: 0,
                volume: 75,
                resampler,
                stats: AudioStats::default(),
            }),
            callback_misses: AtomicU64::new(0),
        }))
    }

    /// Sets the band limit to the lower Nyquist frequency before device playback.
    /// Call on the control thread when the output device opens or changes rate.
    pub fn prepare_device_rate(&self, rate: u32) {
        let source_rate = self.0.sample_rate.get();
        let bandwidth_rate = source_rate.min(rate);
        if bandwidth_rate == 0 {
            return;
        }
        let Ok(queue) = self.0.queue.lock() else {
            return;
        };
        if queue
            .resampler
            .as_ref()
            .is_none_or(|filter| filter.bandwidth_rate == bandwidth_rate)
        {
            return;
        }
        drop(queue);
        let filter = SincResampler::new(source_rate, bandwidth_rate);
        if let Ok(mut queue) = self.0.queue.lock() {
            queue.resampler = Some(filter);
        }
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
            let overflow = queue
                .frames
                .len()
                .saturating_add(samples.len())
                .saturating_sub(queue.capacity);
            let from_queue = overflow.min(queue.frames.len());
            if overflow > 0 {
                queue.frames.drain(..from_queue);
                queue.stats.dropped_frames += overflow as u64;
                queue.reset_interpolation();
            }
            queue.frames.extend(&samples[overflow - from_queue..]);
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
        self.render_with(output, channels, rate, Buffering::DEFAULT, convert);
    }

    /// Fills one live device period, allowing for its full size and this
    /// source's jitter reserve when priming and correcting the source clock.
    pub fn render_device<T: Copy>(
        &self,
        output: &mut [T],
        channels: usize,
        rate: u32,
        convert: impl Fn(f32) -> T,
    ) {
        let buffering = Buffering::for_device(
            self.0.sample_rate.get(),
            rate,
            output.len().checked_div(channels).unwrap_or(0),
            self.0.device_reserve,
        );
        self.render_with(output, channels, rate, buffering, convert);
    }

    fn render_with<T: Copy>(
        &self,
        output: &mut [T],
        channels: usize,
        rate: u32,
        buffering: Buffering,
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
                let Some(mut queue) =
                    try_callback_lock(|| self.0.queue.try_lock().ok(), buffering.lock_retries)
                else {
                    self.0.callback_misses.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let settings = if let Some(settings) = parameters {
                    if queue.generation != settings.generation {
                        return;
                    }
                    settings
                } else {
                    queue.capacity = buffering.capacity;
                    if !queue.playing || (!queue.primed && queue.frames.len() < buffering.prebuffer)
                    {
                        return;
                    }
                    queue.primed = true;
                    // Freeze clock correction across every block in this callback.
                    let correction = ((queue.frames.len() as f64 - buffering.target)
                        / buffering.target
                        * MAX_CLOCK_CORRECTION)
                        .clamp(-MAX_CLOCK_CORRECTION, MAX_CLOCK_CORRECTION);
                    let settings = RenderParameters {
                        step: f64::from(self.0.sample_rate.get()) / f64::from(rate)
                            * (1.0 + correction),
                        gain: f32::from(queue.volume) / (100.0 * 32768.0),
                        generation: queue.generation,
                        fade_per_source_frame: 1000.0 / f64::from(self.0.sample_rate.get()),
                        fade_per_output_frame: 1000.0 / rate as f32,
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

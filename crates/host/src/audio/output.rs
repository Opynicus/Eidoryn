//! CPAL output for the currently selected emulation core.
use super::AudioBuffer;
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::sync::{Arc, Mutex};

pub struct AudioOutput {
    _stream: cpal::Stream,
    source: Arc<Mutex<AudioBuffer>>,
    error: Arc<Mutex<Option<String>>>,
    pub sample_rate: u32,
}

impl AudioOutput {
    pub fn open(buffer: AudioBuffer) -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("No default audio output device")?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("Audio format: {e}"))?;
        let mut config = supported.config();
        if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
            config.buffer_size = cpal::BufferSize::Fixed(512_u32.clamp(*min, *max));
        }
        let error = Arc::new(Mutex::new(None));
        buffer.prepare_device_rate(config.sample_rate);
        let source = Arc::new(Mutex::new(buffer));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, &source, &error),
            cpal::SampleFormat::F64 => build::<f64>(&device, &config, &source, &error),
            cpal::SampleFormat::I8 => build::<i8>(&device, &config, &source, &error),
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, &source, &error),
            cpal::SampleFormat::I24 => build::<cpal::I24>(&device, &config, &source, &error),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, &source, &error),
            cpal::SampleFormat::I64 => build::<i64>(&device, &config, &source, &error),
            cpal::SampleFormat::U8 => build::<u8>(&device, &config, &source, &error),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, &source, &error),
            cpal::SampleFormat::U24 => build::<cpal::U24>(&device, &config, &source, &error),
            cpal::SampleFormat::U32 => build::<u32>(&device, &config, &source, &error),
            cpal::SampleFormat::U64 => build::<u64>(&device, &config, &source, &error),
            format => return Err(format!("Unsupported audio output format: {format}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("Cannot start audio: {e}"))?;
        Ok(Self {
            _stream: stream,
            source,
            error,
            sample_rate: config.sample_rate,
        })
    }

    /// Route the open device to a different fixed-rate PCM queue.
    pub fn select_buffer(&self, buffer: AudioBuffer) {
        buffer.prepare_device_rate(self.sample_rate);
        *self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = buffer;
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    source: &Arc<Mutex<AudioBuffer>>,
    error: &Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    let source = Arc::clone(source);
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let error = Arc::clone(error);
    device
        .build_output_stream(
            *config,
            move |samples: &mut [T], _| {
                let buffer = source.try_lock().ok().map(|current| current.clone());
                if let Some(buffer) = buffer {
                    buffer.render_device(samples, channels, rate, T::from_sample);
                } else {
                    samples.fill(T::from_sample(0.0));
                }
            },
            move |failure| {
                if let Ok(mut slot) = error.lock() {
                    *slot = Some(format!("Audio output: {failure}"));
                }
            },
            None,
        )
        .map_err(|e| format!("Cannot open audio: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    #[test]
    fn i16_and_f32_callbacks_preserve_levels_and_silence() {
        let buffer = AudioBuffer::default();
        buffer.set_volume(100);
        buffer.set_playing(true);
        buffer.push(&[[16_384, -16_384]; 2048]);
        let mut signed = [0_i16; 2];
        let mut float = [0.0_f32; 2];
        buffer.render(&mut signed, 2, 48_000, i16::from_sample);
        buffer.render(&mut float, 2, 48_000, f32::from_sample);
        assert_eq!(signed, [16_384, -16_384]);
        assert_eq!(float, [0.5, -0.5]);
        buffer.set_playing(false);
        buffer.render(&mut signed, 2, 48_000, i16::from_sample);
        buffer.render(&mut float, 2, 48_000, f32::from_sample);
        assert_eq!(signed, [0; 2]);
        assert_eq!(float, [0.0; 2]);
    }

    #[test]
    fn pcm24_callbacks_preserve_levels_and_silence() {
        let buffer = AudioBuffer::default();
        buffer.set_volume(100);
        buffer.set_playing(true);
        buffer.push(&[[16_384, -16_384]; 2048]);
        let mut signed = [cpal::I24::from_sample(0.0); 2];
        let mut unsigned = [cpal::U24::from_sample(0.0); 2];
        buffer.render(&mut signed, 2, 48_000, cpal::I24::from_sample);
        buffer.render(&mut unsigned, 2, 48_000, cpal::U24::from_sample);
        assert_eq!(signed.map(cpal::I24::inner), [4_194_304, -4_194_304]);
        assert_eq!(unsigned.map(cpal::U24::inner), [12_582_912, 4_194_304]);

        buffer.set_playing(false);
        buffer.render(&mut signed, 2, 48_000, cpal::I24::from_sample);
        buffer.render(&mut unsigned, 2, 48_000, cpal::U24::from_sample);
        assert_eq!(signed.map(cpal::I24::inner), [0; 2]);
        assert_eq!(unsigned.map(cpal::U24::inner), [8_388_608; 2]);
    }
}

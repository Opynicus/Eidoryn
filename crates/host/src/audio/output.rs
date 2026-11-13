//! Safe CPAL adapter. All PS1 sound generation stays in the original device core.
use super::AudioBuffer;
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::sync::{Arc, Mutex};

pub struct AudioOutput {
    _stream: cpal::Stream,
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
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, buffer, &error),
            cpal::SampleFormat::F64 => build::<f64>(&device, &config, buffer, &error),
            cpal::SampleFormat::I8 => build::<i8>(&device, &config, buffer, &error),
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, buffer, &error),
            cpal::SampleFormat::I24 => build::<cpal::I24>(&device, &config, buffer, &error),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, buffer, &error),
            cpal::SampleFormat::I64 => build::<i64>(&device, &config, buffer, &error),
            cpal::SampleFormat::U8 => build::<u8>(&device, &config, buffer, &error),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, buffer, &error),
            cpal::SampleFormat::U24 => build::<cpal::U24>(&device, &config, buffer, &error),
            cpal::SampleFormat::U32 => build::<u32>(&device, &config, buffer, &error),
            cpal::SampleFormat::U64 => build::<u64>(&device, &config, buffer, &error),
            format => return Err(format!("Unsupported audio output format: {format}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("Cannot start audio: {e}"))?;
        Ok(Self {
            _stream: stream,
            error,
            sample_rate: config.sample_rate,
        })
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: AudioBuffer,
    error: &Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let error = Arc::clone(error);
    device
        .build_output_stream(
            *config,
            move |samples: &mut [T], _| buffer.render(samples, channels, rate, T::from_sample),
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
    fn pcm24_callbacks_preserve_levels_and_silence() {
        let buffer = AudioBuffer::default();
        buffer.set_volume(100);
        buffer.set_playing(true);
        buffer.push(std::iter::repeat_n([16_384, -16_384], 2048));
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

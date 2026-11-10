use eidoryn_host::audio::{AudioBuffer, wav::WavWriter};
use std::{num::NonZeroU32, path::PathBuf};

#[test]
fn different_core_rates_resample_without_sharing_queue_state() {
    let queues = [32_000, 48_000].map(|rate| {
        let queue = AudioBuffer::with_sample_rate(NonZeroU32::new(rate).unwrap());
        queue.set_playing(true);
        queue.push(std::iter::repeat_n([16_384, -16_384], 2048));
        queue
    });
    // A callback holds a clone; it must retain this source's rate and transport.
    for queue in &queues {
        let mut output = [0.0_f32; 960];
        queue.clone().render(&mut output, 2, 48_000, |s| s);
        assert!(
            output
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| { (pair[0] - 0.375).abs() < 1e-6 && (pair[1] + 0.375).abs() < 1e-6 })
        );
        assert_eq!(queue.stats().underruns, 0);
    }
    // Ten milliseconds at the host rate consumes ~320 or ~480 source frames.
    // Leave room for the existing bounded clock-drift correction.
    assert!((1725..=1730).contains(&queues[0].stats().queued_frames));
    assert!((1565..=1570).contains(&queues[1].stats().queued_frames));
    queues[0].clear();
    assert!(queues[1].stats().queued_frames > 0);
}

struct TempDirectory(PathBuf);
impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "eidoryn-host-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn wav_headers_follow_source_rate_and_invalid_rates_create_no_file() {
    let directory = TempDirectory::new();
    for rate in [32_000, 44_100, 48_000] {
        let path = directory.0.join(format!("{rate}.wav"));
        let rate = NonZeroU32::new(rate).unwrap();
        let mut wav = WavWriter::with_sample_rate(&path, rate).unwrap();
        wav.write([[1, -1], [i16::MIN, i16::MAX]]).unwrap();
        wav.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[24..28], &rate.get().to_le_bytes());
        assert_eq!(&bytes[28..32], &(rate.get() * 4).to_le_bytes());
        assert_eq!(&bytes[40..44], &8_u32.to_le_bytes());
        assert_eq!(&bytes[44..], &[1, 0, 255, 255, 0, 128, 255, 127]);
        assert!(WavWriter::with_sample_rate(&path, rate).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let path = directory.0.join("invalid.wav");
    let error = WavWriter::with_sample_rate(&path, NonZeroU32::new(u32::MAX).unwrap())
        .err()
        .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!path.exists());
}

use eidoryn_host::audio::{AudioBuffer, wav::WavWriter};
use std::{
    cell::Cell,
    num::NonZeroU32,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[test]
fn render_converter_can_reenter_audio_controls() {
    const CHILD: &str = "EIDORYN_AUDIO_REENTRY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let buffer = AudioBuffer::default();
        buffer.set_playing(true);
        buffer.set_volume(100);
        buffer.push(&[[16_384; 2]; 4096]);
        let observer = buffer.clone();
        let conversions = Cell::new(0);
        let mut output = [0.0; 4096];
        buffer.render(&mut output, 2, 48_000, |sample| {
            // The initial silence conversion is already outside the queue lock.
            if sample != 0.0 {
                let _ = observer.stats();
                observer.set_volume(0);
                conversions.set(conversions.get() + 1);
            }
            sample
        });
        assert_eq!(conversions.get(), output.len());
        assert!(output.iter().all(|sample| (*sample - 0.5).abs() < 1e-6));
        buffer.render(&mut output[..2], 2, 48_000, |sample| sample);
        assert!(output[..2].iter().all(|sample| *sample == 0.0));
        check_reentrant_clear_and_pause();
        check_reentrant_enqueue_between_blocks();
        check_initial_silence_conversion_precedes_volume_sampling();
        return;
    }

    // A regression must terminate the child instead of wedging the test runner.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "render_converter_can_reenter_audio_controls",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "audio reentry child failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("audio converter deadlocked while reentering the buffer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check_reentrant_enqueue_between_blocks() {
    let buffer = AudioBuffer::default();
    buffer.set_playing(true);
    buffer.set_volume(100);
    buffer.push(&[[16_384; 2]; 1024]);
    let mut replacement = vec![[8192; 2]; 5000];
    // Overflow retains the last 4096 input frames. A distinct first retained
    // frame makes the fractional-position reset observable at the block edge.
    replacement[5000 - 4096] = [4096; 2];
    let observer = buffer.clone();
    let appended = Cell::new(false);
    let queued_at_push = Cell::new(0);
    let mut output = [0.0; 4096];
    buffer.render(&mut output, 2, 48_000, |sample| {
        if sample != 0.0 && !appended.replace(true) {
            queued_at_push.set(observer.stats().queued_frames);
            observer.push(&replacement);
        }
        sample
    });
    assert!(
        appended.get(),
        "enqueue must run during real PCM conversion"
    );
    assert!(
        output[..2048]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6)
    );
    assert!(
        output[2048..2050]
            .iter()
            .all(|sample| (*sample - 0.125).abs() < 1e-6)
    );
    assert!(
        output[2052..]
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 1e-6)
    );
    assert_eq!(
        buffer.stats().dropped_frames,
        (queued_at_push.get() + replacement.len() - 4096) as u64
    );
    assert_eq!(buffer.stats().underruns, 0);
}

fn check_initial_silence_conversion_precedes_volume_sampling() {
    let buffer = AudioBuffer::default();
    buffer.set_playing(true);
    buffer.set_volume(0);
    buffer.push(&[[16_384; 2]; 2048]);
    let observer = buffer.clone();
    let first_conversion = Cell::new(true);
    let mut output = [0.0; 2048];
    buffer.render(&mut output, 2, 48_000, |sample| {
        if first_conversion.replace(false) {
            assert_eq!(sample, 0.0);
            observer.set_volume(100);
        }
        sample
    });
    assert!(output.iter().all(|sample| (*sample - 0.5).abs() < 1e-6));
}

fn check_reentrant_clear_and_pause() {
    for action in ["clear", "pause", "pause then resume"] {
        let buffer = AudioBuffer::default();
        buffer.set_playing(true);
        buffer.set_volume(100);
        buffer.push(&[[16_384; 2]; 4096]);
        let observer = buffer.clone();
        let changed = Cell::new(false);
        let mut output = [0.0; 4096];
        buffer.render(&mut output, 2, 48_000, |sample| {
            if sample != 0.0 && !changed.replace(true) {
                match action {
                    "clear" => observer.clear(),
                    "pause" => observer.set_playing(false),
                    "pause then resume" => {
                        observer.set_playing(false);
                        observer.set_playing(true);
                    }
                    _ => unreachable!("known control action"),
                }
                observer.push(&[[8192; 2]; 2048]);
            }
            sample
        });
        assert!(changed.get(), "{action} did not reach real samples");
        // The already prepared 1024-frame block is delivered, then the current
        // callback ends. Replacement samples start with the next callback.
        assert!(
            output[..2048]
                .iter()
                .all(|sample| (*sample - 0.5).abs() < 1e-6),
            "{action}"
        );
        assert!(
            output[2048..].iter().all(|sample| *sample == 0.0),
            "{action}"
        );
        assert_eq!(buffer.stats().underruns, 0, "{action}");
        buffer.render(&mut output[..2], 2, 48_000, |sample| sample);
        if action == "pause" {
            assert_eq!(buffer.stats().queued_frames, 0);
            assert!(output[..2].iter().all(|sample| *sample == 0.0));
            buffer.set_playing(true);
            buffer.push(&[[8192; 2]; 2048]);
            buffer.render(&mut output[..2], 2, 48_000, |sample| sample);
        }
        assert!(
            output[..2]
                .iter()
                .all(|sample| (*sample - 0.25).abs() < 1e-6),
            "{action}"
        );
    }
}

#[test]
fn render_preserves_channel_layout_and_incomplete_frame_silence() {
    for channels in [1, 2, 5] {
        let buffer = AudioBuffer::default();
        buffer.set_playing(true);
        buffer.set_volume(100);
        buffer.push(&[[16_384, -8192]; 4096]);
        let mut output = vec![9.0; channels * 1100 + channels - 1];
        buffer.render(&mut output, channels, 48_000, |sample| sample);
        let mut frames = output.chunks_exact(channels);
        for frame in &mut frames {
            if channels == 1 {
                assert!((frame[0] - 0.125).abs() < 1e-6);
            } else {
                assert!((frame[0] - 0.5).abs() < 1e-6);
                assert!((frame[1] + 0.25).abs() < 1e-6);
                assert!(frame[2..].iter().all(|sample| *sample == 0.0));
            }
        }
        assert!(frames.remainder().iter().all(|sample| *sample == 0.0));
        assert_eq!(buffer.stats().underruns, 0, "{channels} channels");
    }
}

#[test]
fn invalid_parameters_and_empty_outputs_do_not_consume_audio() {
    let buffer = AudioBuffer::default();
    buffer.set_playing(true);
    buffer.push(&[[16_384; 2]; 2048]);
    for (channels, rate) in [(0, 48_000), (2, 0), (0, 0), (usize::MAX, 48_000)] {
        let mut output = [9.0; 17];
        buffer.render(&mut output, channels, rate, |sample| sample);
        assert!(output.iter().all(|sample| *sample == 0.0));
        assert_eq!(buffer.stats().queued_frames, 2048);
        assert_eq!(buffer.stats().underruns, 0);
    }
    buffer.render(&mut [], 2, 48_000, |sample| sample);
    assert_eq!(buffer.stats().queued_frames, 2048);
}

#[test]
fn prebuffer_underflow_pause_and_resume_keep_their_statistics() {
    let buffer = AudioBuffer::default();
    let mut output = vec![9.0; 10_000];
    buffer.push(&[[16_384; 2]; 2048]);
    assert_eq!(buffer.stats().queued_frames, 0, "paused input is discarded");
    buffer.render(&mut output, 2, 48_000, |sample| sample);
    assert!(output.iter().all(|sample| *sample == 0.0));
    buffer.set_playing(true);
    buffer.push(&[[16_384; 2]; 1023]);
    buffer.render(&mut output, 2, 48_000, |sample| sample);
    assert!(output.iter().all(|sample| *sample == 0.0));
    assert_eq!(buffer.stats().queued_frames, 1023);
    assert_eq!(buffer.stats().underruns, 0);

    buffer.push(&[[16_384; 2]]);
    buffer.render(&mut output, 2, 48_000, |sample| sample);
    assert!((output[0] - 0.375).abs() < 1e-6);
    assert!(output[3000..].iter().all(|sample| *sample == 0.0));
    assert!(buffer.stats().queued_frames <= 1);
    assert_eq!(buffer.stats().underruns, 1);
    buffer.render(&mut output, 2, 48_000, |sample| sample);
    assert!(output.iter().all(|sample| *sample == 0.0));
    assert_eq!(
        buffer.stats().underruns,
        1,
        "prebuffering is not another underrun"
    );

    buffer.push(&[[16_384; 2]; 2048]);
    buffer.set_playing(false);
    assert_eq!(buffer.stats().queued_frames, 0);
    buffer.set_playing(true);
    buffer.set_volume(u8::MAX);
    buffer.push(&[[16_384; 2]; 2048]);
    buffer.render(&mut output[..2], 2, 48_000, |sample| sample);
    assert!(
        output[..2]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < 1e-6)
    );
    assert_eq!(buffer.stats().underruns, 1);
    buffer.clear();
    assert_eq!(buffer.stats().queued_frames, 0);
}

#[test]
fn slice_enqueue_drops_oldest_frames_and_large_callbacks_finish_with_silence() {
    let buffer = AudioBuffer::default();
    buffer.set_playing(true);
    buffer.set_volume(100);
    let frames: Vec<_> = (0..10_000).map(|value| [value as i16; 2]).collect();
    buffer.push(&frames);
    assert_eq!(buffer.stats().queued_frames, 4096);
    assert_eq!(buffer.stats().dropped_frames, 5904);
    let mut output = vec![9.0; 100_001];
    buffer.render(&mut output, 2, 48_000, |sample| sample);
    assert!((output[0] - 5904.0 / 32768.0).abs() < 1e-6);
    assert!(output[9000..].iter().all(|sample| *sample == 0.0));
    assert_eq!(buffer.stats().underruns, 1);
    assert_eq!(buffer.stats().dropped_frames, 5904);
    assert_eq!(buffer.stats().callback_misses, 0);
}

#[test]
fn large_callbacks_can_fill_every_block_without_exhausting_the_source() {
    let buffer = AudioBuffer::with_sample_rate(NonZeroU32::new(1).unwrap());
    buffer.set_playing(true);
    buffer.set_volume(100);
    buffer.push(&[[16_384, -16_384]; 2048]);
    let mut output = vec![9.0; 100_003];
    buffer.render(&mut output, 3, 48_000, |sample| sample);
    let (frames, tail) = output.as_chunks::<3>();
    for frame in frames {
        assert!((frame[0] - 0.5).abs() < 1e-6);
        assert!((frame[1] + 0.5).abs() < 1e-6);
        assert_eq!(frame[2], 0.0);
    }
    assert!(tail.iter().all(|sample| *sample == 0.0));
    assert_eq!(buffer.stats().queued_frames, 2048);
    assert_eq!(buffer.stats().underruns, 0);
}

#[test]
fn different_core_rates_resample_without_sharing_queue_state() {
    let queues = [32_000, 48_000].map(|rate| {
        let queue = AudioBuffer::with_sample_rate(NonZeroU32::new(rate).unwrap());
        queue.set_playing(true);
        queue.push(&[[16_384, -16_384]; 2048]);
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
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "eidoryn-host-audio-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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

use super::*;
use std::{sync::mpsc, time::Duration};

#[test]
fn contended_callback_returns_silence_without_waiting_for_the_queue() {
    let audio = AudioBuffer::default();
    audio.set_playing(true);
    audio.push(&[[16_384; 2]; PREBUFFER]);
    let queue = audio.0.queue.lock().unwrap();
    let callback = audio.clone();
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut output = vec![1.0; 4097];
        callback.render(&mut output, 2, 48_000, |sample| sample);
        let mut device = vec![1.0; 1024];
        callback.render_device(&mut device, 2, 48_000, |sample| sample);
        send.send((output, device)).unwrap();
    });
    // Release the lock even on failure, so a blocking implementation cannot hang
    // this test process. Successful completion must precede that release.
    let result = receive.recv_timeout(Duration::from_secs(2));
    assert_eq!(queue.frames.len(), PREBUFFER);
    drop(queue);
    worker.join().unwrap();
    assert_eq!(
        result.expect("callback waited for the queue lock"),
        (vec![0.0; 4097], vec![0.0; 1024])
    );
    assert_eq!(audio.stats().callback_misses, 2);
    assert_eq!(audio.stats().underruns, 0);
}

#[test]
fn device_lock_retries_bridge_a_short_copy_and_remain_bounded() {
    let queue = Mutex::new(17);
    let mut writer = Some(queue.lock().unwrap());
    let mut attempts = 0;
    let guard = try_callback_lock(
        || {
            attempts += 1;
            if attempts == 3 {
                writer.take();
            }
            queue.try_lock().ok()
        },
        DEVICE_LOCK_RETRIES,
    )
    .unwrap();
    assert_eq!((*guard, attempts), (17, 3));
    attempts = 0;
    assert!(
        try_callback_lock(
            || {
                attempts += 1;
                queue.try_lock().ok()
            },
            DEVICE_LOCK_RETRIES
        )
        .is_none()
    );
    assert_eq!(attempts, DEVICE_LOCK_RETRIES + 1);
}

#[test]
fn default_low_latency_sources_retain_their_normal_device_thresholds() {
    for source_rate in [44_100, 32_728] {
        let device = Buffering::for_device(source_rate, 48_000, 512, Duration::ZERO);
        assert_eq!(
            (device.prebuffer, device.target, device.capacity),
            (PREBUFFER, TARGET, CAPACITY)
        );
    }
}

#[test]
fn resampling_a_ramp_stays_continuous_across_blocks_and_callbacks() {
    let audio = AudioBuffer::default();
    audio.set_playing(true);
    audio.set_volume(100);
    let mut next_source = 0;
    let mut position = 0.0_f64;
    // Refilling to capacity freezes the expected drift correction at +0.5%
    // once per callback; recalculating it per block would distort the ramp.
    let step = 44_100.0 / 48_000.0 * 1.005;
    for frames in [17, 3073, 1025, 7, 2048] {
        let missing = CAPACITY - audio.stats().queued_frames;
        let samples: Vec<_> = (next_source..next_source + missing)
            .map(|index| [index as i16, -(index as i16)])
            .collect();
        audio.push(&samples);
        next_source += missing;
        let mut output = vec![0.0; frames * 2];
        audio.render(&mut output, 2, 48_000, |sample| sample);
        for (index, sample) in output.as_chunks::<2>().0.iter().enumerate() {
            let expected = position as f32 / 32768.0;
            assert!(
                (sample[0] - expected).abs() < 1e-7 && (sample[1] + expected).abs() < 1e-7,
                "callback frames={frames}, frame={index}, position={position}, sample={sample:?}"
            );
            position += step;
        }
        let queue = audio.0.queue.lock().unwrap();
        assert!((queue.fraction - position.fract()).abs() < 1e-8);
        assert_eq!(queue.frames.len(), next_source - position.floor() as usize);
        assert_eq!(queue.stats.underruns, 0);
        assert_eq!(queue.stats.dropped_frames, 0);
    }
}

#[test]
fn gba_device_periods_keep_playing_through_delayed_video_frames() {
    const SOURCE: u32 = 65_536;
    const DEVICE: u32 = 48_000;
    // One GBA video frame advances 280,896 CPU clocks and produces a stereo
    // sample every 256 clocks. Delay seven frame deliveries by 45 ms.
    const FRAME_CLOCKS: u64 = 280_896;
    const CPU: u64 = 16_777_216;
    for period in [512, 2048, 4096] {
        let audio = AudioBuffer::with_bandlimited_output(
            NonZeroU32::new(SOURCE).unwrap(),
            Duration::from_millis(50),
        );
        audio.prepare_device_rate(DEVICE);
        audio.set_playing(true);
        audio.set_volume(100);
        let mut output = vec![0.0; period * 2];
        let mut video_frame = 1;
        let mut started = false;
        for clock in (0..u64::from(DEVICE) * 17).step_by(period) {
            loop {
                let deadline = (video_frame * FRAME_CLOCKS * u64::from(DEVICE)).div_ceil(CPU);
                let late = u64::from(video_frame % 120 == 0 && video_frame <= 840);
                if deadline + late * u64::from(DEVICE) * 45 / 1000 > clock {
                    break;
                }
                let produced =
                    video_frame * FRAME_CLOCKS / 256 - (video_frame - 1) * FRAME_CLOCKS / 256;
                audio.push(&[[16_384, -16_384]; 1098][..produced as usize]);
                video_frame += 1;
            }
            audio.render_device(&mut output, 2, DEVICE, |sample| sample);
            started |= output[0] != 0.0;
            if started {
                assert!(
                    output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|pair| (pair[0] - 0.5).abs() < f32::EPSILON
                            && (pair[1] + 0.5).abs() < f32::EPSILON),
                    "audible gap at device frame {clock}, period {period}"
                );
            }
        }
        assert!(started);
        assert_eq!(audio.stats().underruns, 0, "period {period}");
        assert_eq!(audio.stats().dropped_frames, 0, "period {period}");
    }
}

#[test]
fn depleted_pcm_fades_to_silence_and_back_across_callbacks() {
    let audio =
        AudioBuffer::with_bandlimited_output(NonZeroU32::new(65_536).unwrap(), Duration::ZERO);
    audio.prepare_device_rate(48_000);
    audio.set_playing(true);
    audio.set_volume(100);
    let mut previous = [0.5, -0.5];
    let mut samples = [0.0; 34];
    for (input, blocks, expected) in [
        ([16_384, -16_384], 200, [0.0, 0.0]),
        ([-16_384, 16_384], 4, [-0.5, 0.5]),
    ] {
        audio.push(&[input; 2048]);
        for _ in 0..blocks {
            audio.render_device(&mut samples, 2, 48_000, |sample| sample);
            for pair in samples.as_chunks::<2>().0 {
                for channel in 0..2 {
                    assert!((pair[channel] - previous[channel]).abs() <= 0.012);
                }
                previous = *pair;
            }
        }
        assert_eq!(previous, expected);
    }
    assert_eq!(audio.stats().underruns, 1);
}

#[test]
fn gba_output_preserves_treble_across_device_callbacks() {
    let amplitude = 0.5;
    for (source, frequencies) in [
        (32_768, [12_000.0, 14_000.0]),
        (65_536, [12_000.0, 16_000.0]),
        (65_536, [20_000.0, 28_000.0]),
    ] {
        for rate in [44_100, 48_000] {
            let audio = AudioBuffer::with_bandlimited_output(
                NonZeroU32::new(source).unwrap(),
                Duration::from_millis(50),
            );
            audio.prepare_device_rate(rate);
            audio.set_playing(true);
            audio.set_volume(100);
            // Freeze the clock correction at zero by filling to the target at each
            // callback. Both channels cross callback boundaries.
            let period = 257;
            let target = Buffering::for_device(source, rate, period, Duration::from_millis(50))
                .target as usize;
            let mut output = vec![0.0; 2 * period];
            audio.render_device(&mut output, 2, rate, |sample| sample);
            let mut next = 0;
            let mut error = [0.0; 2];
            let mut count = 0;
            let phase = |index: usize, frequency: f64, clock: u32| {
                (std::f64::consts::TAU * index as f64 * frequency / f64::from(clock)).sin()
                    * amplitude
            };
            for callback in 0..32 {
                let end = next + target - audio.stats().queued_frames;
                let input = (next..end)
                    .map(|index| {
                        frequencies.map(|frequency| {
                            (phase(index, frequency, source) * 32768.0).round() as i16
                        })
                    })
                    .collect::<Vec<_>>();
                audio.push(&input);
                next = end;
                audio.render_device(&mut output, 2, rate, |sample| sample);
                for (offset, frame) in output.as_chunks::<2>().0.iter().enumerate() {
                    let index = callback * period + offset;
                    if index < 64 {
                        continue;
                    }
                    for channel in 0..2 {
                        let expected = if frequencies[channel] > f64::from(rate) / 2.0 {
                            0.0
                        } else {
                            phase(index, frequencies[channel], rate)
                        };
                        error[channel] += (f64::from(frame[channel]) - expected).powi(2);
                    }
                    count += 1;
                }
            }
            for (frequency, error) in frequencies.into_iter().zip(error) {
                let relative_rms = (2.0 * error / f64::from(count)).sqrt() / amplitude;
                let tolerance = if frequency > f64::from(rate) / 2.0 {
                    0.001
                } else {
                    0.025
                };
                assert!(
                    relative_rms < tolerance,
                    "{frequency} Hz at {rate} Hz: relative RMS error {relative_rms}"
                );
            }
        }
    }
}

#[test]
fn bandlimited_output_discards_old_filter_history_on_timeline_changes() {
    let audio =
        AudioBuffer::with_bandlimited_output(NonZeroU32::new(65_536).unwrap(), Duration::ZERO);
    audio.prepare_device_rate(48_000);
    audio.set_playing(true);
    audio.set_volume(100);
    let mut output = [0.0; 256];
    for overflow in [false, true] {
        audio.clear();
        audio.push(&[[16_384; 2]; 2048]);
        audio.render_device(&mut output, 2, 48_000, |x| x);
        if !overflow {
            audio.clear();
        }
        // Either replace the timeline, or evict every old frame by overflow.
        audio.push(&vec![[-16_384; 2]; CAPACITY]);
        audio.render_device(&mut output, 2, 48_000, |x| x);
        assert!(output.iter().all(|x| (*x + 0.5).abs() < f32::EPSILON));
    }
}

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
        send.send(output).unwrap();
    });
    // Release the lock even on failure, so a blocking implementation cannot hang
    // this test process. Successful completion must precede that release.
    let result = receive.recv_timeout(Duration::from_secs(2));
    assert_eq!(queue.frames.len(), PREBUFFER);
    drop(queue);
    worker.join().unwrap();
    assert_eq!(
        result.expect("callback waited for the queue lock"),
        vec![0.0; 4097]
    );
    assert_eq!(audio.stats().callback_misses, 1);
    assert_eq!(audio.stats().underruns, 0);
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

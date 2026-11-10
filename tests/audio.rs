use eidoryn_psx::{
    audio::{AudioBuffer, wav::WavWriter},
    spu::{SAMPLE_CLOCKS, Spu},
};

const BASE: u32 = 0x1f80_1c00;

fn tone(flags: u8) -> Spu {
    let mut spu = Spu::default();
    spu.write(BASE + 0x1ac, 4).unwrap();
    spu.write(BASE + 0x1a6, 0x200).unwrap();
    let mut block = [0x44; 16];
    block[0] = 0; // Constant positive sample, filter zero, shift zero.
    block[1] = flags;
    for word in block.as_chunks::<4>().0 {
        spu.dma_write(u32::from_le_bytes(*word)).unwrap();
    }
    for (register, value) in [
        (0, 0x3fff),
        (2, 0),
        (4, 0x1000),
        (6, 0x200),
        (8, 0x7fff),
        (10, 0x1fc0),
        (0x180, 0x3fff),
        (0x182, 0x3fff),
        (0x1aa, 0xc000),
    ] {
        spu.write(BASE + register, value).unwrap();
    }
    spu.write(BASE + 0x188, 1).unwrap();
    spu.write(BASE + 12, 0x4000).unwrap(); // Frozen attack level, known gain.
    spu
}

#[test]
fn spu_sample_clock_is_exact_and_buffers_are_bounded() {
    let mut spu = Spu::default();
    spu.tick(SAMPLE_CLOCKS - 1).unwrap();
    assert_eq!(spu.generated_samples, 0);
    spu.tick(1).unwrap();
    assert_eq!(spu.drain_audio().collect::<Vec<_>>(), [[0; 2]]);
    spu.tick(SAMPLE_CLOCKS * 10_000).unwrap();
    assert_eq!(spu.generated_samples, 10_001);
    assert_eq!(spu.drain_audio().count(), 8192);
}

#[test]
fn voice_decodes_pans_loops_reports_end_and_releases() {
    let mut spu = tone(7);
    spu.tick(SAMPLE_CLOCKS * 40).unwrap();
    let output = spu.drain_audio().collect::<Vec<_>>();
    assert!(
        output[4..]
            .iter()
            .all(|sample| (8000..8200).contains(&sample[0]) && sample[1] == 0)
    );
    assert_eq!(spu.read(BASE + 14), 0x200);
    assert_eq!(spu.read(BASE + 0x19c) & 1, 1);
    spu.write(BASE + 0x188, 1).unwrap();
    assert_eq!(spu.read(BASE + 0x19c) & 1, 0);
    spu.tick(SAMPLE_CLOCKS * 4).unwrap();
    spu.write(BASE + 0x18c, 1).unwrap();
    spu.tick(SAMPLE_CLOCKS * 16).unwrap();
    assert_eq!(spu.read(BASE + 12), 0);
    assert_eq!(
        *spu.drain_audio().collect::<Vec<_>>().last().unwrap(),
        [0; 2]
    );
}

#[test]
fn one_shot_ends_while_muting_keeps_decode_and_capture_active() {
    let mut spu = tone(1);
    spu.write(BASE + 0x1aa, 0x8000).unwrap(); // Mute voices, not device clocks.
    spu.tick(SAMPLE_CLOCKS * 40).unwrap();
    assert!(spu.drain_audio().all(|sample| sample == [0; 2]));
    assert_eq!(spu.read(BASE + 0x19c) & 1, 1);
    assert_eq!(spu.read(BASE + 12), 0);
}

#[test]
fn voice_fetch_and_cd_capture_raise_irq_and_cd_ignores_spu_mute() {
    let mut spu = tone(7);
    spu.write(BASE + 0x1a4, 0x200).unwrap();
    spu.write(BASE + 0x1aa, 0xc040).unwrap();
    spu.tick(SAMPLE_CLOCKS).unwrap();
    assert!(spu.irq_pending());
    spu.write(BASE + 0x1aa, 0x8001).unwrap();
    assert!(!spu.irq_pending());
    spu.write(BASE + 0x1b0, 0x7fff).unwrap();
    spu.write(BASE + 0x1b2, 0x7fff).unwrap();
    spu.tick_with_cd(SAMPLE_CLOCKS, || [12_000, -6000]).unwrap();
    let sample = spu.drain_audio().last().unwrap();
    assert!((11_990..12_001).contains(&sample[0]));
    assert!((-6010..-5990).contains(&sample[1]));
    spu.write(BASE + 0x1a6, 0).unwrap();
    assert_eq!((spu.dma_read().unwrap() >> 16) as u16 as i16, 12_000);
    spu.write(BASE + 0x1a4, 0).unwrap();
    spu.write(BASE + 0x1aa, 0x8041).unwrap();
    spu.tick(SAMPLE_CLOCKS).unwrap();
    assert!(spu.irq_pending());
}

#[test]
fn audio_transport_resamples_mutes_clears_and_limits_latency() {
    let audio = AudioBuffer::default();
    audio.set_playing(true);
    audio.push(std::iter::repeat_n([16384, -16384], 2048));
    let mut output = [0.0_f32; 400];
    audio.render(&mut output, 2, 48_000, |s| s);
    for pair in output.as_chunks::<2>().0 {
        assert!((pair[0] - 0.375).abs() < 1e-6 && (pair[1] + 0.375).abs() < 1e-6);
    }
    assert!((1850..1870).contains(&audio.stats().queued_frames));
    audio.set_volume(0);
    audio.render(&mut output, 2, 48_000, |s| s);
    assert!(output.iter().all(|s| *s == 0.0));
    audio.set_playing(false);
    assert_eq!(audio.stats().queued_frames, 0);
    audio.set_playing(true);
    audio.set_volume(100);
    audio.render(&mut output, 2, 48_000, |s| s);
    assert!(output.iter().all(|s| *s == 0.0));
    audio.push(std::iter::repeat_n([123; 2], 10_000));
    assert_eq!(audio.stats().queued_frames, 4096);
    assert_eq!(audio.stats().dropped_frames, 5904);
    audio.clear();
    assert_eq!(audio.stats().queued_frames, 0);
}

#[test]
fn wav_export_preserves_stereo_pcm_and_refuses_overwrite() {
    let path = std::env::temp_dir().join(format!("eidoryn-wav-{}.wav", std::process::id()));
    let mut wav = WavWriter::create(&path).unwrap();
    assert!(WavWriter::create(&path).is_err());
    wav.write([[1, -1], [i16::MIN, i16::MAX]]).unwrap();
    wav.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 44);
    assert_eq!(&bytes[44..], &[1, 0, 255, 255, 0, 128, 255, 127]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn released_voices_still_fetch_sound_ram_and_trigger_irq() {
    let mut spu = tone(7);
    spu.write(BASE + 0x18c, 1).unwrap();
    spu.tick(SAMPLE_CLOCKS * 16).unwrap();
    assert_eq!(spu.read(BASE + 12), 0);
    spu.write(BASE + 0x1a4, 0x200).unwrap();
    spu.write(BASE + 0x1aa, 0xc040).unwrap();
    spu.tick(SAMPLE_CLOCKS * 32).unwrap();
    assert!(spu.irq_pending());
}

#[test]
fn noise_frequency_is_independent_of_voice_pitch() {
    let mut stopped_pitch = tone(7);
    stopped_pitch.write(BASE + 4, 0).unwrap();
    stopped_pitch.write(BASE + 0x194, 1).unwrap();
    stopped_pitch.write(BASE + 0x1aa, 0xff00).unwrap();
    let mut normal_pitch = stopped_pitch.clone();
    normal_pitch.write(BASE + 4, 0x1000).unwrap();
    stopped_pitch.tick(SAMPLE_CLOCKS * 128).unwrap();
    normal_pitch.tick(SAMPLE_CLOCKS * 128).unwrap();
    let noise = stopped_pitch.drain_audio().collect::<Vec<_>>();
    assert!(noise.windows(2).any(|pair| pair[0] != pair[1]));
    assert_eq!(noise, normal_pitch.drain_audio().collect::<Vec<_>>());
}

#[test]
fn early_wav_drop_keeps_recorded_frames_readable() {
    let path = std::env::temp_dir().join(format!("eidoryn-wav-drop-{}.wav", std::process::id()));
    {
        let mut wav = WavWriter::create(&path).unwrap();
        wav.write([[123, -456]]).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 48);
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 4);
    assert_eq!(&bytes[44..], &[123, 0, 56, 254]);
    std::fs::remove_file(path).unwrap();
}

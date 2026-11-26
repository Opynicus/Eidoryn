use eidoryn_host::audio::AudioBuffer;

/// An ARM routine that writes each halfword to a sound register, then idles.
pub fn register_program(writes: &[(u32, u16)]) -> Vec<u8> {
    let mut words = Vec::new();
    let literals = 3 * writes.len() + 1;
    for i in 0..writes.len() {
        let offset = u32::try_from((literals + 2 * i - (3 * i + 2)) * 4).unwrap();
        words.extend([0xe59f_0000 | offset, 0xe59f_1000 | offset, 0xe1c0_10b0]);
    }
    words.push(0xeaff_fffe); // B .
    for &(address, value) in writes {
        words.extend([address, u32::from(value)]);
    }
    words.into_iter().flat_map(u32::to_le_bytes).collect()
}

pub fn assert_audible(queue: &AudioBuffer) {
    let mut device = [0.0_f32; 1024];
    queue.render(&mut device, 2, 48_000, |x| x);
    for side in 0..2 {
        let samples = device.iter().skip(side).step_by(2).copied();
        let range = samples.clone().fold(f32::NEG_INFINITY, f32::max)
            - samples.fold(f32::INFINITY, f32::min);
        assert!(
            range > 0.02,
            "no varying PCM on channel {side}: range={range}"
        );
    }
}

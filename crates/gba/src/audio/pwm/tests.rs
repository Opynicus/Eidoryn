use super::PwmOutput;

const TICK_RATE: u32 = 262_144;
const TICKS_PER_PCM_FRAME: usize = 4;
const AMPLITUDE: f64 = 12_000.0;

fn tone(tick: usize, hz: f64) -> f64 {
    (std::f64::consts::TAU * tick as f64 * hz / f64::from(TICK_RATE)).sin() * AMPLITUDE
}

#[test]
fn all_pwm_modes_preserve_in_band_tones_and_reject_conversion_images() {
    // At 32 kHz an image of 12 kHz would appear at 20.768 kHz after upsampling.
    // At the faster clocks the right channel is above the PCM Nyquist frequency.
    for (mode, frequencies, expected) in [
        (0, [12_000.0, 8_000.0], [12_000.0, 8_000.0]),
        (1, [24_000.0, 12_000.0], [24_000.0, 12_000.0]),
        (2, [20_000.0, 48_000.0], [20_000.0, 0.0]),
        (3, [20_000.0, 84_000.0], [20_000.0, 0.0]),
    ] {
        let mut pwm = PwmOutput::default();
        let mut error = [0.0; 2];
        let mut measured = 0;
        for tick in 1_usize..=8192 {
            let edge = tick
                .is_multiple_of(8 >> mode)
                .then(|| (frequencies.map(|f| tone(tick, f).round() as i16), mode));
            pwm.advance(1, edge);
            if tick.is_multiple_of(TICKS_PER_PCM_FRAME) && tick >= 256 {
                // Every hardware mode has the same fixed 16-output-frame delay.
                let actual = pwm.sample();
                for side in 0..2 {
                    error[side] += (f64::from(actual[side])
                        - tone(tick - 16 * TICKS_PER_PCM_FRAME, expected[side]))
                    .powi(2);
                }
                measured += 1;
            }
        }
        for (side, total) in error.into_iter().enumerate() {
            let relative_rms = (2.0 * total / f64::from(measured)).sqrt() / AMPLITUDE;
            let tolerance = if expected[side] == 0.0 { 0.001 } else { 0.025 };
            assert!(
                relative_rms < tolerance,
                "PWM mode {mode}, {} Hz: relative waveform error {relative_rms}",
                frequencies[side]
            );
        }
    }
}

#[test]
fn changes_of_pwm_rate_preserve_timestamps_between_latches() {
    let mut pwm = PwmOutput::default();
    for tick in 1_usize..=4096 {
        // Change rate on irregular ticks, including before the old period ends.
        let mode = ((tick / 53) % 4) as u8;
        let edge = tick
            .is_multiple_of(8 >> mode)
            .then(|| ([tone(tick, 440.0).round() as i16, -23_456], mode));
        pwm.advance(1, edge);
        if tick > 136 && tick.is_multiple_of(TICKS_PER_PCM_FRAME) {
            let [left, right] = pwm.sample();
            assert_eq!(right, -23_456, "DC changed at tick {tick}");
            // At the switch the held DAC value may be up to one old PWM
            // period earlier than a sample on the new clock.
            let expected = tone(tick - 16 * TICKS_PER_PCM_FRAME, 440.0);
            let max_delta = AMPLITUDE * std::f64::consts::TAU * 440.0 / 32_768.0;
            assert!(
                (f64::from(left) - expected).abs() <= max_delta,
                "timestamp slipped at tick {tick}"
            );
        }
    }
}

//! Checks the title audio on locally provided retail cartridges without a sound device.
#[path = "support/local_roms.rs"]
mod local_roms;

use eidoryn_gba::Machine;

#[test]
#[ignore = "requires local Aria of Sorrow, Harmony of Dissonance or Pokémon Emerald ROMs"]
fn cartridges_generate_changing_audio_on_both_outputs() {
    local_roms::check(
        std::env::var_os("EIDORYN_GBA_TEST_ROMS"),
        |rom| matches!(&rom[0xac..0xb0], b"A2CE" | b"A2CJ" | b"ACHE" | b"BPEE"),
        |rom| {
            let serial = String::from_utf8_lossy(&rom[0xac..0xb0]).into_owned();
            let mut machine = Machine::from_rom(rom).map_err(|e| e.to_string())?;
            let mut frames = 0;
            let mut changes = [0; 2];
            let mut peak = [0; 2];
            let mut previous = [0; 2];
            for _ in 0..600 {
                machine.run_frame().map_err(|e| e.to_string())?;
                for sample in machine.take_audio_samples() {
                    frames += 1;
                    for side in 0..2 {
                        peak[side] = peak[side].max(sample[side].unsigned_abs());
                        changes[side] += usize::from(sample[side] != previous[side]);
                    }
                    previous = sample;
                }
            }
            assert_eq!(frames, 600 * 280_896 / 256);
            if changes.iter().any(|&n| n < 1000) {
                return Err(format!(
                    "too few transitions in title audio: {changes:?}, peaks {peak:?}"
                ));
            }
            eprintln!("{serial}: {frames} stereo frames, transitions {changes:?}, peaks {peak:?}");
            Ok(())
        },
    );
}

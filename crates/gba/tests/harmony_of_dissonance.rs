//! Local US Harmony of Dissonance cartridge, starting with an erased SRAM chip.
#[path = "support/local_roms.rs"]
mod local_roms;
#[path = "support/screen.rs"]
mod screen;

use eidoryn_gba::Machine;

fn buttons(frame: u32) -> u16 {
    match frame {
        // Open the first slot, enter a name and accept it.
        120..900 if frame % 60 < 6 => {
            if (frame / 60).is_multiple_of(2) {
                1
            } else {
                8
            }
        }
        900..7500 if frame % 16 < 4 => 1,
        // Walk, jump and whip to pass Talos on the bridge.
        7500..=11_000 => 16 | u16::from(frame % 40 < 12) | (u16::from(frame % 13 < 3) << 1),
        11_001.. => 16,
        _ => 0,
    }
}

#[test]
#[ignore = "requires a local US Harmony of Dissonance cartridge"]
fn title_new_game_and_bridge_crossing() {
    local_roms::check(
        std::env::var_os("EIDORYN_HARMONY_ROM")
            .or_else(|| std::env::var_os("EIDORYN_GBA_TEST_ROMS")),
        |rom| &rom[0xac..0xb0] == b"ACHE",
        |rom| {
            let mut machine = Machine::from_rom(rom).map_err(|e| e.to_string())?;
            // Static RGB crops from an independent mGBA run of the same inputs.
            let mut pending = vec![
                ("title prompt", 260..=340, [106, 122, 35, 10], 0xb06c_f0ed),
                (
                    "Juste's health bar",
                    7350..=7500,
                    [1, 0, 93, 13],
                    0x52e1_8fdf,
                ),
                (
                    "castle drawbridge",
                    10_900..=11_750,
                    [0, 60, 200, 95],
                    0xca98_bd11,
                ),
            ];
            for frame in 1..=11_750 {
                machine.set_buttons(buttons(frame));
                machine
                    .run_frame()
                    .map_err(|e| format!("frame {frame}: {e}"))?;
                pending.retain(|(_, frames, [x, y, w, h], expected)| {
                    !frames.contains(&frame)
                        || screen::region_signature(&machine, *x, *y, *w, *h) != *expected
                });
                if pending.is_empty() {
                    return Ok(());
                }
            }
            Err(format!("did not reach {}", pending[0].0))
        },
    );
}

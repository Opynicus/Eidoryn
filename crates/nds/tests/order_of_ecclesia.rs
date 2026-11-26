//! Region-specific regression with a user-supplied, unmodified retail cartridge.
use eidoryn_nds::{Machine, SCREEN_HEIGHT, WIDTH};
use std::path::PathBuf;

const IMAGE: &str = "Castlevania - Order of Ecclesia (USA) (En,Fr).nds";
const LCD: usize = WIDTH * SCREEN_HEIGHT * 3;

fn gold_on_row(lcd: &[u8], y: usize) -> usize {
    let row = &lcd[y * WIDTH * 3..(y + 1) * WIDTH * 3];
    row.as_chunks::<3>()
        .0
        .iter()
        .filter(|&&[r, g, b]| r > 190 && g > 110 && b < 75)
        .count()
}

// Navigate the language and save menus, accept a name, then skip the opening story.
fn walkthrough_buttons(frame: u32) -> u16 {
    match frame {
        95..=98 => 1 << 7,   // Down to French
        125..=128 => 1 << 6, // Up to English
        160..=163 => 1,
        2170..=2179 => 1 << 3, // Start selects OK on the name keyboard
        f if f >= 3322 && (f - 3322) % 271 < 10 => 1 << 3,
        f if f > 163 && !(2120..2470).contains(&f) && (f - 160) % 90 < 10 => 1,
        _ => 0,
    }
}

fn check_playable_room(machine: &mut Machine) {
    let lower = &machine.pixels()[LCD..];
    // The two horizontal HP and MP bars are at the top left of the playfield.
    for x in 35..75 {
        let at = |y| &lower[(y * WIDTH + x) * 3..(y * WIDTH + x) * 3 + 3];
        let hp = at(6);
        let mp = at(12);
        assert!(hp[0] > 200 && hp[0] > hp[1] && hp[2] > hp[1]);
        assert!(mp[1] > mp[0] && mp[1] > mp[2]);
    }
    let state = machine.save_state().unwrap();
    let mut simulate = |buttons| {
        machine.load_state(&state).unwrap();
        machine.set_buttons(buttons);
        for _ in 0..45 {
            machine.run_frame().unwrap();
        }
        machine.pixels().to_vec()
    };
    let idle = simulate(0);
    let right = simulate(1 << 4);
    assert_eq!(
        &idle[..LCD],
        &right[..LCD],
        "map should stay on the upper LCD"
    );
    let mut moved = 0;
    let (idle_pixels, _) = idle[LCD..].as_chunks::<3>();
    let (right_pixels, _) = right[LCD..].as_chunks::<3>();
    for (i, (a, b)) in idle_pixels.iter().zip(right_pixels).enumerate() {
        if a != b {
            assert!(
                (155..250).contains(&(i % WIDTH)) && (120..182).contains(&(i / WIDTH)),
                "expected only Shanoa to move in the stationary room"
            );
            moved += 1;
        }
    }
    assert!(moved > 700, "pressing Right did not move Shanoa");
}

#[test]
#[ignore = "requires the US Order of Ecclesia cartridge"]
fn cold_boot_through_language_selection_to_playable_ecclesia() {
    let path = std::env::var_os("EIDORYN_ECCLESIA_ROM").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests")
                .join(IMAGE)
        },
        PathBuf::from,
    );
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        rom.get(12..16),
        Some(b"YR9E".as_slice()),
        "expected the US edition"
    );
    let mut machine = Machine::from_rom(rom).unwrap();
    let mut changing_samples = [0; 2];
    let mut total = 0u64;
    for frame in 1..=4120 {
        machine.set_buttons(walkthrough_buttons(frame));
        machine
            .run_frame()
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        let pcm = machine.take_audio_samples();
        total += pcm.len() as u64;
        let window = usize::from(frame >= 3000);
        changing_samples[window] += pcm.windows(2).filter(|pair| pair[0] != pair[1]).count();
        let (top, bottom) = machine.pixels().split_at(LCD);
        match frame {
            80 | 120 | 150 => {
                let (selected, other) = if frame == 120 { (113, 79) } else { (79, 113) };
                assert!(
                    gold_on_row(bottom, selected) >= 65,
                    "missing selection border at frame {frame}"
                );
                assert!(
                    gold_on_row(bottom, other) < 12,
                    "both options selected at frame {frame}"
                );
                assert!(
                    top.iter().all(|&v| v == 0),
                    "language menu is displayed on the lower LCD"
                );
            }
            540 => {
                let red = top
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter(|&&[r, g, b]| r > 100 && g < 85 && b < 95)
                    .count();
                assert!(
                    (3000..5000).contains(&red),
                    "expected KONAMI logo after confirming English, found {red} red pixels"
                );
                assert!(bottom.iter().all(|&v| v == 255));
                assert_eq!(machine.backup().map(<[u8]>::len), Some(8192));
            }
            _ => {}
        }
    }
    eprintln!(
        "DS audio: {total} stereo samples; waveform changes before and after frame 3000: {changing_samples:?}"
    );
    assert_eq!(total, 4120 * 2130 * 263 / 1024);
    assert!(
        changing_samples.iter().all(|&count| count > 10_000),
        "expected guest music and sound effects in both parts of the walkthrough"
    );
    check_playable_room(&mut machine);
}

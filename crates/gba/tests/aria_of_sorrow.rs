//! Optional commercial-ROM regression. Loads a local cartridge with a fresh in-memory save.
use eidoryn_gba::{Machine, WIDTH};
use std::{env, fs, path::PathBuf};

fn buttons(frame: u32) -> u16 {
    const A: u16 = 1;
    const START: u16 = 1 << 3;
    const RIGHT: u16 = 1 << 4;
    match frame {
        120..600 if frame % 60 < 6 => {
            if (frame / 60).is_multiple_of(2) {
                A
            } else {
                START
            }
        }
        600..2950 if frame % 8 < 3 => A,
        2950.. if frame % 30 < 8 => RIGHT | A,
        2950.. => RIGHT,
        _ => 0,
    }
}

/// FNV-1a over an RGB rectangle. Compare static UI elements so guest animation
/// and small differences in the number of instructions per field can vary.
fn region_signature(machine: &Machine, x: usize, y: usize, w: usize, h: usize) -> u32 {
    machine
        .pixels()
        .as_chunks::<{ WIDTH * 3 }>()
        .0
        .iter()
        .skip(y)
        .take(h)
        .flat_map(|row| &row[x * 3..(x + w) * 3])
        .fold(0x811c_9dc5, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        })
}

fn usa_cartridge() -> Machine {
    let rom = env::var_os("EIDORYN_ARIA_ROM").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/Castlevania - Aria of Sorrow (USA).gba")
        },
        PathBuf::from,
    );
    let bytes = fs::read(&rom).unwrap_or_else(|e| panic!("{}: {e}", rom.display()));
    assert_eq!(
        bytes.get(0xac..0xb0),
        Some(b"A2CE".as_slice()),
        "expected the USA cartridge"
    );
    assert_eq!(bytes[0xbc], 0, "expected revision 0");
    Machine::from_rom(bytes).unwrap()
}

#[test]
#[ignore = "requires a locally supplied USA (A2CE, revision 0) cartridge"]
fn boots_to_the_title_screen_without_input() {
    let mut machine = usa_cartridge();
    for frame in 1..=1600 {
        machine
            .run_frame()
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        // The static lettering in the title logo also matches a BIOS-less mGBA boot.
        if frame >= 1350 && region_signature(&machine, 60, 77, 100, 12) == 0x9e7f_48f0 {
            return;
        }
    }
    panic!("did not reach the title screen");
}

#[test]
#[ignore = "requires a locally supplied USA (A2CE, revision 0) cartridge"]
fn usa_cartridge_boots_and_reaches_playable_castle_entrance() {
    let mut machine = usa_cartridge();
    let mut saw_select_data = false;
    let mut saw_health_display = false;
    let mut entered_castle_corridor = false;
    for frame in 1..=3600 {
        machine.set_buttons(buttons(frame));
        machine
            .run_frame()
            .unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        // Reference crops from a BIOS-less mGBA run with the same keypad sequence.
        if (340..=450).contains(&frame) && !saw_select_data {
            saw_select_data = region_signature(&machine, 16, 5, 76, 10) == 0x4e0e_1f06;
        }
        if (2850..=3100).contains(&frame) && !saw_health_display {
            saw_health_display = region_signature(&machine, 0, 0, 96, 16) == 0x5807_3e02;
        }
        if (3200..=3550).contains(&frame) && !entered_castle_corridor {
            // Crossing the door replaces the sky above the HUD with the corridor's stone ceiling.
            entered_castle_corridor = region_signature(&machine, 104, 0, 72, 12) == 0x6b8f_a17d;
        }
    }
    assert!(
        saw_select_data,
        "did not reach the save-slot selection screen"
    );
    assert!(
        saw_health_display,
        "did not reach gameplay with the initial health display"
    );
    assert!(entered_castle_corridor, "did not enter the Castle Corridor");
}

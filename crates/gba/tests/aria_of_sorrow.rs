//! Local regional builds of Aria of Sorrow, each booted with a fresh in-memory save.
#[path = "support/local_roms.rs"]
mod local_roms;

#[path = "support/screen.rs"]
mod screen;

use eidoryn_gba::Machine;
use screen::region_signature;
use sha2::{Digest, Sha256};
use std::{env, ops::RangeInclusive};

struct Checkpoint {
    scene: &'static str,
    frames: RangeInclusive<u32>,
    rect: [usize; 4],
    signature: u32,
}

struct Reference {
    sha256: &'static str,
    title: Checkpoint,
    input: fn(u32) -> u16,
    gameplay: &'static [Checkpoint],
}

const HEALTH_DISPLAY: Checkpoint = Checkpoint {
    scene: "health display",
    frames: 2850..=3100,
    rect: [0, 0, 96, 16],
    signature: 0x5807_3e02,
};

// RGB crops from independent mGBA runs with the same inputs, relative to cartridge entry.
const REFERENCES: &[Reference] = &[
    Reference {
        sha256: "910f88d3701e6c9f99c0d63660c14c61d8b79b0c17bfd7b35a15185be36430a5",
        title: Checkpoint {
            scene: "title logo",
            frames: 1350..=1600,
            rect: [60, 77, 100, 12],
            signature: 0x9e7f_48f0,
        },
        input: buttons,
        gameplay: &[
            Checkpoint {
                scene: "save slots",
                frames: 340..=450,
                rect: [16, 5, 76, 10],
                signature: 0x4e0e_1f06,
            },
            HEALTH_DISPLAY,
            Checkpoint {
                scene: "Castle Corridor",
                frames: 3200..=3550,
                rect: [104, 0, 72, 12],
                signature: 0x6b8f_a17d,
            },
        ],
    },
    Reference {
        sha256: "c36899e05a29b481d3fb38c2edc9569437f0811e67fa50a067bf587a6419f891",
        title: Checkpoint {
            scene: "Japanese title (暁月の円舞曲)",
            frames: 1350..=1600,
            rect: [108, 97, 80, 17],
            signature: 0xe93a_f074,
        },
        input: buttons,
        gameplay: &[
            Checkpoint {
                scene: "Japanese save-slot prompt",
                frames: 310..=390,
                rect: [12, 124, 75, 10],
                signature: 0x07ed_65d2,
            },
            HEALTH_DISPLAY,
            Checkpoint {
                scene: "Japanese Castle Corridor banner (荒城回廊)",
                frames: 3200..=3500,
                rect: [98, 77, 29, 11],
                signature: 0xf25e_f779,
            },
        ],
    },
];

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

fn check_editions(run: impl Fn(Vec<u8>, Option<&Reference>) -> Result<(), String>) {
    local_roms::check(
        env::var_os("EIDORYN_ARIA_ROM").or_else(|| env::var_os("EIDORYN_GBA_TEST_ROMS")),
        |rom| &rom[0xac..0xaf] == b"A2C",
        |rom| {
            let digest = format!("{:x}", Sha256::digest(&rom));
            run(rom, REFERENCES.iter().find(|r| r.sha256 == digest))
        },
    );
}

fn visible_scene(machine: &Machine) -> bool {
    let pixels = machine.pixels();
    pixels[..pixels.len() - 3] != pixels[3..]
}

fn advance(machine: &mut Machine, frame: u32, input: u16) -> Result<(), String> {
    machine.set_buttons(input);
    machine
        .run_frame()
        .map_err(|e| format!("frame {frame}: {e}"))
}

/// Compare a keyed run to an independent idle boot, including for images with reference data.
fn replay(
    rom: Vec<u8>,
    input: Option<fn(u32) -> u16>,
    checkpoints: &[Checkpoint],
    min_frames: u32,
) -> Result<(), String> {
    let mut idle = if input.is_some() {
        Some(Machine::from_rom(rom.clone()).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let mut machine = Machine::from_rom(rom).map_err(|e| e.to_string())?;
    let mut pending: Vec<_> = checkpoints.iter().collect();
    let end = checkpoints
        .iter()
        .map(|c| *c.frames.end())
        .max()
        .unwrap_or(0)
        .max(min_frames);
    let mut saw_scene = false;
    let mut responsive_samples = 0;
    for frame in 1..=end {
        advance(&mut machine, frame, input.map_or(0, |keys| keys(frame)))?;
        if let Some(idle) = idle.as_mut() {
            advance(idle, frame, 0)?;
        }
        if frame >= end / 2 && frame % 60 == 0 && visible_scene(&machine) {
            saw_scene = true;
            if idle
                .as_ref()
                .is_some_and(|idle| idle.pixels() != machine.pixels())
            {
                responsive_samples += 1;
            }
        }
        pending.retain(|c| {
            let [x, y, w, h] = c.rect;
            !c.frames.contains(&frame) || region_signature(&machine, x, y, w, h) != c.signature
        });
    }
    if let Some(c) = pending.first() {
        return Err(format!("did not see {} in frames {:?}", c.scene, c.frames));
    }
    if !saw_scene {
        return Err(
            "boot produced only solid-color samples during the second half of the test".into(),
        );
    }
    if idle.is_some() && responsive_samples < 3 {
        return Err(format!(
            "expected visible input response relative to idle at three or more samples, observed {responsive_samples}"
        ));
    }
    Ok(())
}

#[test]
#[ignore = "requires a locally supplied Aria of Sorrow cartridge"]
fn boots_each_edition_without_input() {
    check_editions(|rom, reference| {
        let checkpoints = reference.map_or(&[][..], |r| std::slice::from_ref(&r.title));
        replay(rom, None, checkpoints, 1600)
    });
}

#[test]
#[ignore = "requires a locally supplied Aria of Sorrow cartridge"]
fn input_advances_each_edition() {
    check_editions(|rom, reference| {
        let input = reference.map_or(buttons as fn(u32) -> u16, |r| r.input);
        let checkpoints = reference.map_or(&[][..], |r| r.gameplay);
        replay(rom, Some(input), checkpoints, 3600)
    });
}

#[test]
#[ignore = "requires a locally supplied Aria of Sorrow cartridge"]
fn restores_and_replays_each_edition() {
    check_editions(|rom, reference| {
        let input = reference.map_or(buttons as fn(u32) -> u16, |r| r.input);
        let mut machine = Machine::from_rom(rom).map_err(|e| e.to_string())?;
        for frame in 1..=360 {
            advance(&mut machine, frame, input(frame))?;
        }
        let saved = machine.save_state().map_err(|e| e.to_string())?;
        let replay = |machine: &mut Machine| {
            for frame in 361..=390 {
                advance(machine, frame, input(frame))?;
            }
            machine.save_state().map_err(|e| e.to_string())
        };
        let expected = replay(&mut machine)?;
        machine.load_state(&saved).map_err(|e| e.to_string())?;
        if replay(&mut machine)? != expected {
            return Err("replay after restore diverged from uninterrupted execution".into());
        }
        Ok(())
    });
}

#[test]
#[ignore = "requires a locally supplied Aria of Sorrow cartridge"]
fn game_initialized_battery_survives_cold_boot() {
    check_editions(|rom, reference| {
        let input = reference.map_or(buttons as fn(u32) -> u16, |r| r.input);
        let mut cart = Machine::from_rom(rom.clone()).map_err(|e| e.to_string())?;
        for frame in 1..=600 {
            advance(&mut cart, frame, input(frame))?;
        }
        let battery = cart.battery_save();
        if battery.len() != 32 * 1024 || !battery.iter().any(|&b| b != 0xff) {
            return Err("game did not initialize its 32 KiB SRAM".into());
        }
        let mut restarted = Machine::from_rom(rom.clone()).map_err(|e| e.to_string())?;
        restarted
            .restore_battery_save(battery)
            .map_err(|e| e.to_string())?;
        for frame in 1..=300 {
            advance(&mut restarted, frame, 0)?;
        }
        if restarted.battery_save() != battery {
            return Err("cold boot changed the previously initialized SRAM".into());
        }
        if reference.is_some() {
            save_in_castle_corridor_and_reload(cart, rom)?;
        }
        Ok(())
    });
}

fn save_in_castle_corridor_and_reload(mut cart: Machine, rom: Vec<u8>) -> Result<(), String> {
    const RIGHT: u16 = 1 << 4;
    const LEFT: u16 = 1 << 5;
    const UP: u16 = 1 << 6;
    const A: u16 = 1;
    // A static section of the save sanctuary wall, clear of the animated statue and Soma.
    let in_sanctuary = |machine: &Machine| region_signature(machine, 32, 92, 24, 16) == 0x6c43_98fb;

    let mut frame = 600;
    for f in frame + 1..=3300 {
        advance(&mut cart, f, buttons(f))?;
    }
    frame = 3300;
    for (count, keys) in [
        (1630, RIGHT), // Cross the zombie corridor and collect the Short Sword.
        (100, LEFT),   // Drop from the central platform to the flooded floor.
        (60, 0),
        (345, RIGHT),
        (24, RIGHT | A), // Jump out of the water and enter the lower right door.
        (145, RIGHT),
    ] {
        for _ in 0..count {
            frame += 1;
            advance(&mut cart, frame, keys)?;
        }
    }
    if !in_sanctuary(&cart) {
        return Err("did not reach the Castle Corridor save room".into());
    }
    let before = cart.battery_save().to_vec();
    for (count, keys) in [(4, UP), (36, 0), (3, UP), (4, 0), (3, A), (60, 0)] {
        for _ in 0..count {
            frame += 1;
            advance(&mut cart, frame, keys)?;
        }
    }
    if cart.battery_save() == before {
        return Err("confirming the save statue prompt did not update SRAM".into());
    }
    let mut restarted = Machine::from_rom(rom).map_err(|e| e.to_string())?;
    restarted
        .restore_battery_save(cart.battery_save())
        .map_err(|e| e.to_string())?;
    for f in 1..=720 {
        advance(&mut restarted, f, buttons(f))?;
        if f >= 300 && in_sanctuary(&restarted) {
            return Ok(());
        }
    }
    Err("loading the slot after a cold boot did not return Soma to the save room".into())
}

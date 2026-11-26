//! Cold boot, new game and Flash round-trip with a locally supplied BPEE cartridge.
#[path = "support/local_roms.rs"]
mod local_roms;
#[path = "support/screen.rs"]
mod screen;

use eidoryn_gba::Machine;
use screen::region_signature;

fn new_game_buttons(frame: u32) -> u16 {
    const A: u16 = 1;
    const START: u16 = 8;
    const RIGHT: u16 = 16;
    const DOWN: u16 = 128;
    match frame {
        // Confirm the trainer, choose a name, and reach the moving truck.
        480..=3700 if (frame - 480) % 14 < 5 => A,
        3701..=5100 if (frame - 3701) % 18 < 3 => A,
        5210..=5212 | 8380..=8383 => START,
        5271..=5520 => RIGHT,
        // Step out of the truck and follow Mom into the Littleroot house.
        5521..=6400 if (frame - 5521) % 14 < 5 => A,
        6401..=8370 if (frame - 6401) % 12 < 4 => A,
        8405..=8407 | 8420..=8422 => DOWN, // BAG -> trainer card -> SAVE
        8450..=9180 if (frame - 8450) % 34 < 4 => A,
        _ => 0,
    }
}

#[test]
#[ignore = "requires a local English Pokémon Emerald (BPEE) cartridge"]
fn new_game_in_littleroot_can_be_saved_and_continued_on_cold_boot() {
    local_roms::check(
        std::env::var_os("EIDORYN_EMERALD_ROM")
            .or_else(|| std::env::var_os("EIDORYN_GBA_TEST_ROMS")),
        |rom| &rom[0xac..0xb0] == b"BPEE",
        save_and_continue,
    );
}

fn save_and_continue(rom: Vec<u8>) -> Result<(), String> {
    let mut machine = Machine::from_rom(rom.clone()).map_err(|e| e.to_string())?;
    for f in 1..=9200 {
        machine.set_buttons(new_game_buttons(f));
        machine
            .run_frame()
            .unwrap_or_else(|e| panic!("new game at frame {f}: {e}"));
    }
    let battery = machine.battery_save().to_vec();
    assert_eq!(battery.len(), 128 * 1024);
    assert!(battery.iter().any(|&byte| byte != 0xff));
    drop(machine);
    let mut reopened = Machine::from_rom(rom).map_err(|e| e.to_string())?;
    reopened.restore_battery_save(&battery).unwrap();
    // The three early A presses skip the intro and title. A cartridge with a
    // nonworking clock stops at the dry-battery warning before CONTINUE.
    for frame in 1..=1200 {
        let press = matches!(frame, 480..=484 | 520..=524 | 560..=564 | 970..=974);
        reopened.set_buttons(u16::from(press));
        reopened
            .run_frame()
            .unwrap_or_else(|e| panic!("continued game at frame {frame}: {e}"));
        // Both static regions also matched an independent mGBA run loading the raw save.
        if frame == 780 {
            assert_eq!(
                region_signature(&reopened, 6, 6, 74, 16),
                0x8057_e598,
                "CONTINUE"
            );
        }
    }
    assert_eq!(
        region_signature(&reopened, 104, 84, 45, 20),
        0xe774_1cfc,
        "Littleroot house"
    );
    Ok(())
}

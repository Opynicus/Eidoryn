//! Cold-boot smoke test for any locally supplied GBA binaries, independent of region.
#[path = "support/local_roms.rs"]
mod local_roms;

use std::env;

#[test]
#[ignore = "requires local GBA cartridges and a GBA BIOS"]
fn boots_each_local_cartridge() {
    let frames = env::var_os("EIDORYN_GBA_BOOT_FRAMES").map_or(1800, |value| {
        value
            .to_str()
            .and_then(|s| s.parse().ok())
            .filter(|&n: &u32| n > 0)
            .expect("EIDORYN_GBA_BOOT_FRAMES must be a positive integer")
    });
    local_roms::check(
        env::var_os("EIDORYN_GBA_TEST_ROMS"),
        |_| true,
        |bytes, bios| {
            let mut machine = local_roms::boot(bytes, bios)?;
            for frame in 1..=frames {
                machine
                    .run_frame()
                    .map_err(|e| format!("frame {frame}: {e}"))?;
            }
            Ok(())
        },
    );
}

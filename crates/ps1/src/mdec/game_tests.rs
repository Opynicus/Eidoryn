//! Opt-in verification with locally supplied BIOS and game images.
use super::Operation;
use crate::{machine::Machine, timing::CPU_CLOCK_HZ};
use std::{path::PathBuf, time::Instant};

fn check_snapshot(machine: &Machine, label: &str) -> Result<(), String> {
    let bytes = machine.save_state()?;
    let mut restored = Machine::load_state(&bytes)?;
    assert!(restored.save_state()? == bytes, "round trip: {label}");
    let mut reference = machine.clone();
    reference.run_steps(100_000).map_err(|e| e.to_string())?;
    restored.run_steps(100_000).map_err(|e| e.to_string())?;
    assert!(
        reference.save_state()? == restored.save_state()?,
        "continued execution: {label}"
    );
    println!(
        "snapshot PASS {label} instructions={} frame={} command={:#010x} index={} remaining={} blocks={}",
        machine.cpu.instructions,
        machine.bus.gpu.frames,
        machine.bus.mdec.command,
        machine.bus.mdec.table_index,
        machine.bus.mdec.remaining,
        machine.bus.mdec.blocks_decoded,
    );
    Ok(())
}

// EIDORYN_TEST_DISC selects one local game; EIDORYN_TEST_STEPS bounds the run.
// EIDORYN_TEST_FRAME optionally exports the final image, without overwriting it.
#[test]
#[ignore = "requires a local PS1 BIOS and EIDORYN_TEST_DISC game image"]
fn real_game_mdec_save_state() -> Result<(), String> {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let bios_path = std::env::var_os("EIDORYN_TEST_BIOS")
        .map_or_else(|| assets.join("scph5501.bin"), PathBuf::from);
    let disc = PathBuf::from(std::env::var_os("EIDORYN_TEST_DISC").ok_or("Set EIDORYN_TEST_DISC")?);
    let limit = std::env::var("EIDORYN_TEST_STEPS")
        .unwrap_or_else(|_| "1000000000".into())
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    let bios = std::fs::read(bios_path).map_err(|e| e.to_string())?;
    let mut machine = Machine::from_disc(&disc, Some(&bios), false).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut seen = [[false; 4]; 3];
    let mut decoded = false;
    println!(
        "game={} steps={limit} BIOS boot, interpreter, serial renderer",
        disc.display()
    );
    for step in 1..=limit {
        machine.step().map_err(|e| format!("step {step}: {e}"))?;
        let mdec = &machine.bus.mdec;
        let table = match mdec.command >> 29 {
            2 if mdec.command & 1 == 0 => Some((0, 64, 4, "Quant64")),
            2 => Some((1, 128, 4, "Quant128")),
            3 => Some((2, 64, 2, "Scale")),
            _ => None,
        };
        if let Some((mode, limit, stride, name)) = table {
            let stage = match mdec.operation {
                Operation::Quant | Operation::Scale => [0, limit / 2, limit - stride]
                    .iter()
                    .position(|&index| index == mdec.table_index),
                Operation::Idle if mdec.table_index == limit => Some(3),
                _ => None,
            };
            if let Some(stage) = stage
                && !seen[mode][stage]
            {
                check_snapshot(&machine, &format!("{name} stage={stage}"))?;
                seen[mode][stage] = true;
            }
        }
        if !decoded && machine.bus.mdec.blocks_decoded > 0 {
            check_snapshot(&machine, "first decoded block")?;
            decoded = true;
        }
        if step.is_multiple_of(25_000_000) {
            check_snapshot(&machine, "periodic")?;
            println!(
                "progress steps={step} guest_seconds={:.2} frames={} sectors={} MDECcommands={} blocks={} wall_seconds={:.2}",
                machine.elapsed_cycles() as f64 / f64::from(CPU_CLOCK_HZ),
                machine.bus.gpu.frames,
                machine.bus.cdrom.sectors_read,
                machine.bus.mdec.commands,
                machine.bus.mdec.blocks_decoded,
                start.elapsed().as_secs_f64(),
            );
        }
    }
    check_snapshot(&machine, "final")?;
    if let Some(path) = std::env::var_os("EIDORYN_TEST_FRAME") {
        use std::io::Write;
        let mut file = std::fs::File::create_new(path).map_err(|e| e.to_string())?;
        let (width, height) = machine.bus.gpu.dimensions();
        write!(file, "P6\n{width} {height}\n255\n").map_err(|e| e.to_string())?;
        file.write_all(&machine.bus.gpu.display_rgb())
            .map_err(|e| e.to_string())?;
    }
    println!(
        "coverage [first, middle, last, idle]: Quant64={:?} Quant128={:?} Scale={:?}; decoded={decoded}",
        seen[0], seen[1], seen[2]
    );
    Ok(())
}

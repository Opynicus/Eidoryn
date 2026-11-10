//! Isolate interpreter/device scheduling costs in an original CPU-only guest.
use eidoryn_psx::{demo, machine::Machine};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), String> {
    let mut machine =
        Machine::from_exe(&demo::executable(), "timing probe").map_err(|e| e.to_string())?;
    machine
        .bus
        .write32(demo::ENTRY, 0x0800_4000)
        .map_err(|e| e.to_string())?;
    machine
        .bus
        .write32(demo::ENTRY + 4, 0)
        .map_err(|e| e.to_string())?;
    for part in ["full", "CPU", "bus", "GPU timing", "timers", "SIO"] {
        let mut sample = machine.clone();
        let start = Instant::now();
        for _ in 0..10_000_000 {
            match part {
                "full" => sample.step().map_err(|e| e.to_string())?,
                "CPU" => sample
                    .cpu
                    .step(&mut sample.bus)
                    .map_err(|e| e.to_string())?,
                "bus" => sample.bus.tick(1).map_err(|e| e.to_string())?,
                "GPU timing" => {
                    black_box(sample.bus.gpu.tick(1, &mut sample.bus.timers));
                }
                "timers" => {
                    black_box(sample.bus.timers.tick(1));
                }
                _ => sample.bus.sio.tick(1),
            }
        }
        println!(
            "{part}: {:.2} ns/step",
            start.elapsed().as_secs_f64() * 100.0
        );
        black_box(sample);
    }
    Ok(())
}

//! Measure the worker's throughput and pause latency without a window.
use eidoryn_psx::{
    machine::Machine,
    runner::{Command, Runner},
};
use std::time::{Duration, Instant};

fn main() -> Result<(), String> {
    let mut machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "worker probe")
        .map_err(|e| e.to_string())?;
    // CPU-only self-loop removes rasterization cost from the scheduling measurement.
    machine
        .bus
        .write32(eidoryn_psx::demo::ENTRY, 0x0800_4000)
        .map_err(|e| e.to_string())?;
    machine
        .bus
        .write32(eidoryn_psx::demo::ENTRY + 4, 0)
        .map_err(|e| e.to_string())?;
    let runner = Runner::new(machine, true).map_err(|e| e.to_string())?;
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(3));
    let pause = Instant::now();
    runner.send(Command::Toggle)?;
    loop {
        if let Some(snapshot) = runner.take_snapshot()
            && !snapshot.running
        {
            println!(
                "instructions={} instructions/s={:.0} pause_ms={:.2}",
                snapshot.cpu.instructions,
                snapshot.cpu.instructions as f64 / start.elapsed().as_secs_f64(),
                pause.elapsed().as_secs_f64() * 1000.0
            );
            break;
        }
        if pause.elapsed() > Duration::from_secs(5) {
            return Err("Worker did not pause".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

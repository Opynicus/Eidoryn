//! Compare device scheduling on equal synthetic workloads, not game FPS.
use eidoryn_psx::{bus::Bus, cpu::Cpu, demo, machine::Machine};
use std::{hint::black_box, time::Instant};

const CLOCKS: u32 = 7_770_000; // Divisible by all tested batch sizes.
const STEPS: u32 = 6_000_000;

fn main() -> Result<(), String> {
    let mut expected = None;
    for batch in [1, 7, 37] {
        for round in 1..=3 {
            let mut bus = Bus::default();
            let start = Instant::now();
            for _ in 0..CLOCKS / batch {
                bus.tick(black_box(batch)).map_err(|e| e.to_string())?;
            }
            let elapsed = start.elapsed();
            let signature = (
                bus.elapsed_cycles(),
                bus.gpu.frames,
                bus.gpu.status(),
                bus.spu.generated_samples,
                bus.timers.read(0x1100),
                bus.timers.read(0x1110),
                bus.timers.read(0x1120),
            );
            if let Some(expected) = expected {
                assert_eq!(signature, expected);
            } else {
                expected = Some(signature);
                println!("bus state={signature:?}");
            }
            println!(
                "bus batch={batch:2} round={round} elapsed_ms={:.3}",
                elapsed.as_secs_f64() * 1000.0
            );
            black_box(bus);
        }
    }

    for round in 1..=3 {
        let mut machine =
            Machine::from_exe(&demo::executable(), "clock benchmark").map_err(|e| e.to_string())?;
        machine
            .bus
            .write32(demo::ENTRY, 0x8c22_0000)
            .map_err(|e| e.to_string())?; // lw v0, 0(at)
        machine
            .bus
            .write32(demo::ENTRY + 4, 0x0800_4000)
            .map_err(|e| e.to_string())?; // j ENTRY
        machine
            .bus
            .write32(demo::ENTRY + 8, 0)
            .map_err(|e| e.to_string())?; // delay slot
        machine.cpu = Cpu::new(demo::ENTRY);
        machine.cpu.registers[1] = 0x100;
        let start = Instant::now();
        for _ in 0..STEPS {
            machine.step().map_err(|e| e.to_string())?;
        }
        let elapsed = start.elapsed();
        assert_eq!(machine.elapsed_cycles(), u64::from(STEPS) * 3);
        if round == 1 {
            println!(
                "load state=({:08x}, {}, {}, {}, {:08x})",
                machine.cpu.pc,
                machine.cpu.instructions,
                machine.elapsed_cycles(),
                machine.bus.gpu.frames,
                machine.bus.gpu.status()
            );
        }
        println!(
            "load loop round={round} elapsed_ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        );
        black_box(machine);
    }
    Ok(())
}

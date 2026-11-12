//! Synthetic snapshot microbenchmark; no game assets, renderer, or worker protocol changes.
//! Run: `cargo run --locked --release --no-default-features --example snapshot_bench`
use eidoryn_psx::{
    cpu::{Cpu, disassemble},
    machine::Machine,
    runner::{Snapshot, SnapshotMode},
};
use std::{hint::black_box, time::Instant};

fn measure<T>(name: &str, mut operation: impl FnMut() -> T) {
    for _ in 0..2_000 {
        black_box(operation());
    }
    let mut samples = [0.0_f64; 5];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..100_000 {
            black_box(operation());
        }
        *sample = start.elapsed().as_secs_f64() * 1e6 / 100_000.;
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{name}: median {:.3} us/op (min {:.3}, max {:.3}; 5 x 100000)",
        samples[2], samples[0], samples[4]
    );
}

fn main() {
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    machine.bus.gpu.latch_display();
    machine.cpu.pc = eidoryn_psx::demo::ENTRY;
    assert!(machine.bus.gpu.completed_frame().is_some());
    println!(
        "Original demo; completed framebuffer; Cpu = {} bytes",
        size_of::<Cpu>()
    );
    // Inline sizes include child structs, never their heap buffers. Do not add
    // Machine/Bus totals to their children when reporting memory by module.
    for (name, bytes) in [
        ("Machine", size_of::<Machine>()),
        ("Bus", size_of::<eidoryn_psx::bus::Bus>()),
        ("GPU", size_of::<eidoryn_psx::gpu::Gpu>()),
        ("SPU", size_of::<eidoryn_psx::spu::Spu>()),
        ("CD-ROM", size_of::<eidoryn_psx::cdrom::CdRom>()),
        ("MDEC", size_of::<eidoryn_psx::mdec::Mdec>()),
        ("SIO", size_of::<eidoryn_psx::sio::Sio>()),
        ("Snapshot", size_of::<Snapshot>()),
    ] {
        println!("{name} inline size: {bytes} bytes (excludes heap buffers)");
    }
    measure("Snapshot::capture", || {
        Snapshot::capture(black_box(&machine), true, None, 0.0)
    });
    measure("Display snapshot", || {
        Snapshot::capture_with_mode(black_box(&machine), true, None, 0.0, SnapshotMode::Display)
    });
    for mode in [SnapshotMode::Debug, SnapshotMode::Display] {
        let snapshot = Snapshot::capture_with_mode(&machine, true, None, 0.0, mode);
        let rows = &snapshot.disassembly;
        let bytes = rows.capacity() * size_of::<(u32, Option<u32>, String)>()
            + rows
                .iter()
                .map(|(_, _, text)| text.capacity())
                .sum::<usize>();
        println!(
            "{mode:?} retained disassembly: {bytes} bytes (Vec and String capacities; excludes allocator overhead)"
        );
    }
    measure("CPU clone", || black_box(&machine.cpu).clone());
    measure("9 disassembly rows", || {
        let machine = black_box(&machine);
        (0..9)
            .map(|n| {
                let pc = machine.cpu.pc.wrapping_add(n * 4);
                let word = machine.bus.peek32(pc);
                (
                    pc,
                    word,
                    word.map_or_else(|| "unmapped".into(), |word| disassemble(pc, word)),
                )
            })
            .collect::<Vec<_>>()
    });
}

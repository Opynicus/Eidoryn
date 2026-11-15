//! Linux fixed-count rewind capture/retention microbenchmark; no game assets.
//! Build: `cargo build --locked --release --no-default-features --example rewind_bench`
//! Run separately for each count: `target/release/examples/rewind_bench 0|20|40`
//! See `docs/REWIND_PERFORMANCE.md` for the CSV schema and measurement limits.
use eidoryn_ps1::{machine::Machine, memory_card::MemoryCard, timing::CPU_CLOCK_HZ};
use std::{hint::black_box, time::Instant};

fn memory_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/smaps_rollup")
        .unwrap()
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name == key).then(|| value.split_whitespace().next().unwrap().parse().unwrap())
        })
        .unwrap()
}

fn counter(machine: &Machine) -> u32 {
    let start = eidoryn_ps1::demo::FRAME_COUNTER;
    u32::from_le_bytes(machine.bus.ram[start..start + 4].try_into().unwrap())
}

fn advance(machine: &mut Machine, interval_cycles: u64) {
    let target = machine.elapsed_cycles() + interval_cycles;
    loop {
        machine.run_slice().unwrap();
        machine.bus.spu.drain_audio().for_each(drop);
        if machine.elapsed_cycles() >= target {
            break;
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let count: usize = args
        .next()
        .expect("expected count: 0, 20 or 40")
        .parse()
        .unwrap();
    assert!([0, 20, 40].contains(&count));
    let mode = args.next().unwrap_or_else(|| "frame".into());
    let interval_cycles = match mode.as_str() {
        "frame" => 0,
        "250ms" => u64::from(CPU_CLOCK_HZ) / 4,
        _ => panic!("expected frame or 250ms"),
    };
    assert!(args.next().is_none());
    let mut machine = Machine::demo();
    machine.bus.sio.cards = [Some(MemoryCard::formatted()), Some(MemoryCard::formatted())];
    for _ in 0..10 {
        advance(&mut machine, 0);
    }
    let start_cycles = machine.elapsed_cycles();
    let start_counter = counter(&machine);
    let rss_before = memory_kib("Rss");
    let pss_before = memory_kib("Pss");
    let mut states = Vec::with_capacity(count);
    let mut capture_us = Vec::with_capacity(count);
    let mut capture_cycles = Vec::with_capacity(count);
    let mut estimated_bytes = 0_usize;
    let start = Instant::now();
    for index in 0..40 {
        let previous_cycles = machine.elapsed_cycles();
        let previous_counter = counter(&machine);
        advance(&mut machine, interval_cycles);
        assert!(machine.elapsed_cycles() > previous_cycles);
        assert!(counter(&machine) > previous_counter);
        if index >= 40 - count {
            let capture_start = Instant::now();
            let state = machine.rewind_state().unwrap();
            capture_us.push(capture_start.elapsed().as_secs_f64() * 1e6);
            estimated_bytes += state.memory_size();
            capture_cycles.push(machine.elapsed_cycles());
            states.push(state);
        }
    }
    let wall_ms = start.elapsed().as_secs_f64() * 1e3;
    let rss_after = memory_kib("Rss");
    let pss_after = memory_kib("Pss");
    let capture_total: f64 = capture_us.iter().sum();
    capture_us.sort_by(f64::total_cmp);
    let mean = capture_total / count.max(1) as f64;
    let median = if count == 0 {
        0.
    } else {
        f64::midpoint(capture_us[(count - 1) / 2], capture_us[count / 2])
    };
    let p95 = capture_us
        .get((count * 95).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or(0.);
    let maximum = capture_us.last().copied().unwrap_or(0.);
    let guest_seconds = (machine.elapsed_cycles() - start_cycles) as f64 / f64::from(CPU_CLOCK_HZ);
    let history_seconds = capture_cycles.first().map_or(0., |oldest| {
        (machine.elapsed_cycles() - oldest) as f64 / f64::from(CPU_CLOCK_HZ)
    });
    println!(
        "{mode},{count},{rss_before},{rss_after},{pss_before},{pss_after},{estimated_bytes},{mean:.3},{median:.3},{p95:.3},{maximum:.3},{capture_total:.3},{wall_ms:.3},{guest_seconds:.9},{history_seconds:.9},{},{}",
        counter(&machine) - start_counter,
        machine.elapsed_cycles()
    );
    // Keep all captured guest states live until after the memory samples.
    black_box(&states);
    assert_eq!(states.len(), count);
}

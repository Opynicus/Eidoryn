//! Same-checkpoint CPU comparisons, including device clocks and GPU completion.
use eidoryn_psx::{cpu_engine::CpuMode, machine::Machine};
use std::time::Instant;

fn sample(machine: &Machine, mode: CpuMode) -> Result<Machine, String> {
    let mut sample = machine.clone();
    sample
        .bus
        .gpu
        .set_renderer(machine.bus.gpu.renderer_mode())
        .map_err(|e| e.to_string())?;
    // Both samples start without a native cache, even if the session uses JIT.
    sample
        .set_cpu_mode(CpuMode::Interpreter)
        .map_err(|e| e.to_string())?;
    sample.set_cpu_mode(mode).map_err(|e| e.to_string())?;
    Ok(sample)
}

pub fn benchmark(machine: &Machine, steps: u64) -> Result<(), String> {
    let steps = usize::try_from(steps).map_err(|e| e.to_string())?;
    if steps == 0 {
        return Err("Use a positive step count".into());
    }
    let mut elapsed = [Vec::new(), Vec::new()];
    for round in 0..3 {
        let mut samples = [
            Box::new(sample(machine, CpuMode::Interpreter)?),
            Box::new(sample(machine, CpuMode::Jit)?),
        ];
        for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let sample = &mut samples[index];
            let frames = sample.bus.gpu.frames;
            let start = Instant::now();
            sample.run_steps(steps).map_err(|e| e.to_string())?;
            sample.bus.gpu.synchronize_renderer();
            sample.check_renderer().map_err(|e| e.to_string())?;
            let seconds = start.elapsed().as_secs_f64();
            elapsed[index].push(seconds);
            let stats = sample.cpu_stats();
            println!(
                "cpu-bench round={} mode={} renderer={} steps={steps} seconds={seconds:.3} video_FPS={:.2} native={:.1}% blocks={} invalidations={}",
                round + 1,
                sample.cpu_mode().label(),
                sample.bus.gpu.renderer_mode().label(),
                (sample.bus.gpu.frames - frames) as f64 / seconds,
                100.0 * stats.native_instructions as f64 / steps as f64,
                stats.compiled_blocks,
                stats.code_invalidations
            );
        }
        let [left, right] = &mut samples;
        super::renderer::compare_state(left, right)?;
    }
    for values in &mut elapsed {
        values.sort_by(f64::total_cmp);
    }
    println!(
        "cpu-bench median interpreter={:.3}s jit={:.3}s speedup={:.3}x; cold compilation included; all 3 CPU/RAM/VRAM/GPU/queued PCM checkpoints match",
        elapsed[0][1],
        elapsed[1][1],
        elapsed[0][1] / elapsed[1][1]
    );
    Ok(())
}

pub fn compare(machine: &Machine, slices: u64) -> Result<(), String> {
    let mut interpreter = sample(machine, CpuMode::Interpreter)?;
    let mut jit = sample(machine, CpuMode::Jit)?;
    let frames = interpreter.bus.gpu.frames;
    for slice in 0..slices {
        interpreter.run_slice().map_err(|e| e.to_string())?;
        jit.run_slice().map_err(|e| e.to_string())?;
        let a = &interpreter.bus.gpu;
        let b = &jit.bus.gpu;
        let same_frame = match (a.completed_frame(), b.completed_frame()) {
            (Some(a), Some(b)) => a.dimensions == b.dimensions && a.rgb == b.rgb,
            (None, None) => true,
            _ => false,
        };
        if !same_frame
            || a.status() != b.status()
            || a.frames != b.frames
            || format!("{:?}", interpreter.cpu) != format!("{:?}", jit.cpu)
            || interpreter.bus.spu.drain_audio().collect::<Vec<_>>()
                != jit.bus.spu.drain_audio().collect::<Vec<_>>()
        {
            return Err(format!(
                "Interpreter/JIT CPU/frame/PCM differs at slice {slice}"
            ));
        }
    }
    super::renderer::compare_state(&mut interpreter, &mut jit)?;
    println!(
        "cpu-compare slices={slices} video_frames={} every slice CPU/completed pixels/status/PCM match; final RAM/VRAM match; native={} blocks={}",
        interpreter.bus.gpu.frames - frames,
        jit.cpu_stats().native_instructions,
        jit.cpu_stats().compiled_blocks
    );
    Ok(())
}

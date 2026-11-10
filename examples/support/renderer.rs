//! Same-checkpoint comparisons; never modify the interactive session or its cards.
use eidoryn_psx::{gpu::RendererMode, machine::Machine};
use std::time::Instant;

fn sample(machine: &Machine, mode: RendererMode) -> Result<Machine, String> {
    let mut sample = machine.clone();
    sample
        .bus
        .gpu
        .set_renderer(mode)
        .map_err(|e| e.to_string())?;
    Ok(sample)
}

pub fn compare_state(left: &mut Machine, right: &mut Machine) -> Result<(), String> {
    left.bus.gpu.synchronize_renderer();
    right.bus.gpu.synchronize_renderer();
    left.check_renderer().map_err(|e| e.to_string())?;
    right.check_renderer().map_err(|e| e.to_string())?;
    if format!("{:?}", left.cpu) != format!("{:?}", right.cpu)
        || left.bus.ram != right.bus.ram
        || left.bus.gpu.vram() != right.bus.gpu.vram()
        || left.bus.gpu.status() != right.bus.gpu.status()
        || left.bus.gpu.frames != right.bus.gpu.frames
        || left.bus.gpu.commands != right.bus.gpu.commands
        || left.bus.spu.drain_audio().collect::<Vec<_>>()
            != right.bus.spu.drain_audio().collect::<Vec<_>>()
    {
        return Err(
            "Checkpoint comparison: CPU, RAM, VRAM, GPU state or queued PCM differs".into(),
        );
    }
    Ok(())
}

pub fn benchmark(machine: &Machine, steps: u64, mode: RendererMode) -> Result<(), String> {
    if steps == 0 {
        return Err("Use a positive step count".into());
    }
    let mut elapsed = [Vec::new(), Vec::new()];
    for round in 0..3 {
        let mut samples = [
            Box::new(sample(machine, RendererMode::Serial)?),
            Box::new(sample(machine, mode)?),
        ];
        // Alternate order to reduce systematic warmup/thermal bias.
        for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let sample = &mut samples[index];
            let frames = sample.bus.gpu.frames;
            let start = Instant::now();
            sample
                .run_steps(usize::try_from(steps).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            // Include all submitted GPU work in the measured interval.
            sample.bus.gpu.synchronize_renderer();
            sample.check_renderer().map_err(|e| e.to_string())?;
            let seconds = start.elapsed().as_secs_f64();
            elapsed[index].push(seconds);
            println!(
                "render-bench round={} mode={} steps={steps} seconds={seconds:.3} video_FPS={:.2}",
                round + 1,
                sample.bus.gpu.renderer_mode().label(),
                (sample.bus.gpu.frames - frames) as f64 / seconds,
            );
        }
        let [left, right] = &mut samples;
        compare_state(left, right)?;
    }
    for values in &mut elapsed {
        values.sort_by(f64::total_cmp);
    }
    println!(
        "median serial={:.3}s {}={:.3}s speedup={:.3}x; all 3 checkpoints match (CPU/RAM/VRAM/GPU/queued PCM)",
        elapsed[0][1],
        mode.label(),
        elapsed[1][1],
        elapsed[0][1] / elapsed[1][1]
    );
    Ok(())
}

pub fn compare(machine: &Machine, slices: u64, mode: RendererMode) -> Result<(), String> {
    let mut serial = sample(machine, RendererMode::Serial)?;
    let mut candidate = sample(machine, mode)?;
    let frames = serial.bus.gpu.frames;
    for slice in 0..slices {
        serial.run_slice().map_err(|e| e.to_string())?;
        candidate.run_slice().map_err(|e| e.to_string())?;
        let a = &serial.bus.gpu;
        let b = &candidate.bus.gpu;
        let pixels_match = match (a.completed_frame(), b.completed_frame()) {
            (Some(a), Some(b)) => a.dimensions == b.dimensions && a.rgb == b.rgb,
            (None, None) => true,
            _ => false,
        };
        if !pixels_match || a.status() != b.status() || a.frames != b.frames {
            return Err(format!(
                "Serial/{} frame differs at slice {slice}",
                mode.label()
            ));
        }
    }
    compare_state(&mut serial, &mut candidate)?;
    println!(
        "render-compare slices={slices} video_frames={} all completed pixels/status match; final CPU/RAM/VRAM/GPU/queued PCM match",
        serial.bus.gpu.frames - frames
    );
    Ok(())
}

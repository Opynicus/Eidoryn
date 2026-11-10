//! Reproducible guest-frame latency and output trace, without host pacing or file-backed saves.
use eidoryn_psx::{machine::Machine, runner::Snapshot};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant},
};

fn hash(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |sum, byte| {
        (sum ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

pub(super) fn benchmark(
    original: &Machine,
    frames: usize,
    buttons: u16,
    prefix: &Path,
) -> Result<(), String> {
    if !(1..=1800).contains(&frames) {
        return Err("Use 1..1800 video frames".into());
    }
    let mut log =
        BufWriter::new(File::create_new(prefix.with_extension("csv")).map_err(|e| e.to_string())?);
    writeln!(log, "round,frame,buttons,ms,max_slice_ms,steps,gpu_commands,cd_sectors,spu_halfwords,pc,rgb_hash,pcm_hash")
        .map_err(|e| e.to_string())?;
    for round in 0..3 {
        let mut sample = original.clone();
        sample.bus.cdrom.enable_trace(false);
        sample
            .bus
            .gpu
            .set_renderer(original.bus.gpu.renderer_mode())
            .map_err(|e| e.to_string())?;
        sample.bus.spu.drain_audio().for_each(drop);
        let mut times = Vec::with_capacity(frames);
        for frame in 0..frames {
            // Two-frame presses, released for the remainder of each half-second.
            // Both the baseline and changed builds observe identical guest input.
            let held = if frame % 30 < 2 { buttons } else { 0 };
            sample.bus.sio.set_buttons(held);
            let old_frame = sample.bus.gpu.frames;
            let old_commands = sample.bus.gpu.commands;
            let old_sectors = sample.bus.cdrom.sectors_read;
            let old_halfwords = sample.bus.spu.transferred_halfwords;
            let mut pcm_hash = 0;
            let mut steps = 0;
            let mut elapsed = Duration::ZERO;
            let mut max_slice = Duration::ZERO;
            while sample.bus.gpu.frames == old_frame {
                let start = Instant::now();
                steps += sample.run_slice().map_err(|e| e.to_string())?;
                let duration = start.elapsed();
                elapsed += duration;
                max_slice = max_slice.max(duration);
                // Keep the normal bounded SPU queue drained; diagnostics are not timed.
                pcm_hash = u64::rotate_left(pcm_hash, 1)
                    ^ hash(
                        sample
                            .bus
                            .spu
                            .drain_audio()
                            .flatten()
                            .flat_map(i16::to_le_bytes),
                    );
                if steps > 1_000_000 {
                    return Err("No video field within the step limit".into());
                }
            }
            let start = Instant::now();
            let snapshot = Snapshot::capture(&sample, true, None, 0.0);
            elapsed += start.elapsed(); // Includes completion/readback for asynchronous renderers.
            if let Some(error) = snapshot.message {
                return Err(error);
            }
            let ms = elapsed.as_secs_f64() * 1000.0;
            times.push(ms);
            writeln!(log, "{round},{frame},{held:04x},{ms:.6},{:.6},{steps},{},{},{},{:08x},{:016x},{pcm_hash:016x}",
                max_slice.as_secs_f64() * 1000.0,
                sample.bus.gpu.commands - old_commands,
                sample.bus.cdrom.sectors_read - old_sectors,
                sample.bus.spu.transferred_halfwords - old_halfwords,
                sample.cpu.pc,
                hash(snapshot.rgb.iter().copied()),
            ).map_err(|e| e.to_string())?;
        }
        let total: f64 = times.iter().sum();
        times.sort_by(f64::total_cmp);
        println!(
            "frame-bench round={round} buttons={buttons:04x} frames={frames} total_ms={total:.3} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3} over_budget={}",
            total / frames as f64,
            times[frames / 2],
            times[(frames - 1) * 95 / 100],
            times[(frames - 1) * 99 / 100],
            times[frames - 1],
            times
                .iter()
                .filter(|&&ms| ms > 1000.0 / sample.bus.gpu.nominal_fps())
                .count()
        );
        log.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

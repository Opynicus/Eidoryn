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

#[derive(Clone, Copy)]
pub(super) enum Input {
    Pulsed(u16),
    Held(u16),
}

impl std::str::FromStr for Input {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(held) = s.strip_prefix("held:") {
            u16::from_str_radix(held, 16).map(Self::Held)
        } else {
            u16::from_str_radix(s, 16).map(Self::Pulsed)
        }
    }
}

impl Input {
    fn mask(self, field: usize) -> u16 {
        match self {
            Self::Pulsed(_) if field % 30 >= 2 => 0,
            Self::Pulsed(mask) | Self::Held(mask) => mask,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Capture {
    pub(super) warmup: usize,
    pub(super) final_fields: usize,
}

fn write_image(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let (w, h) = snapshot.dimensions;
    let mut image = File::create_new(path).map_err(|e| e.to_string())?;
    write!(image, "P6\n{w} {h}\n255\n")
        .and_then(|()| image.write_all(&snapshot.rgb))
        .map_err(|e| e.to_string())
}

pub(super) fn benchmark(
    original: &Machine,
    frames: usize,
    input: Input,
    prefix: &Path,
    capture: Capture,
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
            .set_renderer_config(
                original.bus.gpu.renderer_mode(),
                original.bus.gpu.render_scale(),
                original.bus.gpu.color_depth(),
            )
            .map_err(|e| e.to_string())?;
        sample
            .bus
            .gpu
            .set_anti_aliasing(original.bus.gpu.anti_aliasing());
        sample.bus.spu.drain_audio().for_each(drop);
        let mut times = Vec::with_capacity(frames);
        for field in 0..capture.warmup + frames {
            let held = input.mask(field);
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
            if let Some(error) = sample.bus.gpu.take_presentation_warning() {
                return Err(error);
            }
            let Some(frame) = field.checked_sub(capture.warmup) else {
                continue;
            };
            let ms = elapsed.as_secs_f64() * 1000.0;
            times.push(ms);
            if round == 2 && frames - frame <= capture.final_fields {
                let path = if frame + 1 == frames {
                    prefix.with_extension("ppm")
                } else {
                    prefix.with_extension(format!("{frame:04}.ppm"))
                };
                write_image(&path, &snapshot)?;
            }
            writeln!(log, "{round},{frame},{held:04x},{ms:.6},{:.6},{steps},{},{},{},{:08x},{:016x},{pcm_hash:016x}",
                max_slice.as_secs_f64() * 1000.0,
                sample.bus.gpu.commands - old_commands,
                sample.bus.cdrom.sectors_read - old_sectors,
                sample.bus.spu.transferred_halfwords - old_halfwords,
                sample.cpu.pc,
                hash(snapshot.rgb.iter().copied()),
            ).map_err(|e| e.to_string())?;
        }
        print_percentiles(
            round,
            input.mask(0),
            &mut times,
            sample.bus.gpu.nominal_fps(),
        );
        log.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn print_percentiles(round: usize, buttons: u16, times: &mut [f64], fps: f64) {
    let frames = times.len();
    let total: f64 = times.iter().sum();
    times.sort_by(f64::total_cmp);
    println!(
        "frame-bench round={round} buttons={buttons:04x} frames={frames} total_ms={total:.3} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3} over_budget={}",
        total / frames as f64,
        times[frames.div_ceil(2) - 1],
        times[(frames * 95).div_ceil(100) - 1],
        times[(frames * 99).div_ceil(100) - 1],
        times[frames - 1],
        times.iter().filter(|&&ms| ms > 1000.0 / fps).count()
    );
}

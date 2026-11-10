//! Compare the old live-VRAM presentation with completed-field snapshots.
use eidoryn_psx::{
    bus::physical,
    machine::{INSTRUCTIONS_PER_SLICE, Machine},
    runner::Snapshot,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

struct Metrics {
    lit: usize,
    sum: u64,
    dominant: usize,
    pixels: usize,
}

impl Metrics {
    fn measure(rgb: &[u8]) -> Self {
        let mut histogram = vec![0_usize; 32768];
        let mut lit = 0;
        let mut sum = 0;
        for p in rgb.as_chunks::<3>().0 {
            let color = usize::from(p[0] >> 3)
                | (usize::from(p[1] >> 3) << 5)
                | (usize::from(p[2] >> 3) << 10);
            histogram[color] += 1;
            lit += usize::from(p != &[0, 0, 0]);
            sum += p.iter().map(|c| u64::from(*c)).sum::<u64>();
        }
        Self {
            lit,
            sum,
            dominant: histogram.into_iter().max().unwrap_or(0),
            pixels: rgb.len() / 3,
        }
    }

    fn mostly_solid(&self) -> bool {
        self.dominant > self.pixels * 3 / 4
    }
}

fn save(path: &Path, dimensions: (usize, usize), rgb: &[u8]) -> Result<(), String> {
    let mut file = File::create_new(path).map_err(|e| e.to_string())?;
    write!(file, "P6\n{} {}\n255\n", dimensions.0, dimensions.1)
        .and_then(|()| file.write_all(rgb))
        .map_err(|e| e.to_string())
}

fn gp1_write(machine: &Machine) -> Option<u32> {
    let word = machine.bus.peek32(machine.cpu.pc)?;
    if word >> 26 != 0x2b {
        return None;
    }
    let base = machine.cpu.registers[((word >> 21) & 31) as usize];
    let address = base.wrapping_add_signed(i32::from(word as i16));
    (physical(address) == 0x1f80_1814)
        .then_some(machine.cpu.registers[((word >> 16) & 31) as usize])
}

pub(super) fn trace(
    machine: &mut Machine,
    steps: &mut u64,
    slices: u64,
    prefix: &Path,
) -> Result<(), String> {
    let mut log =
        BufWriter::new(File::create_new(prefix.with_extension("csv")).map_err(|e| e.to_string())?);
    writeln!(
        log,
        "kind,step,frame,status,value,lit_pixels,channel_sum,dominant_pixels"
    )
    .map_err(|e| e.to_string())?;
    let mut saved = 0;
    let mut frames = 0;
    let mut solid = [0; 2];
    for _ in 0..slices {
        let old_frame = machine.bus.gpu.frames;
        for _ in 0..INSTRUCTIONS_PER_SLICE {
            let write = gp1_write(machine).map(|value| {
                (
                    value,
                    machine.cpu.exceptions,
                    machine.bus.gpu.frames,
                    machine.bus.gpu.status(),
                )
            });
            machine.step().map_err(|e| e.to_string())?;
            // A pending interrupt can divert the CPU before the decoded SW.
            if let Some((value, exceptions, frame, status)) = write
                && machine.cpu.exceptions == exceptions
            {
                writeln!(log, "gp1,{steps},{frame},{status:08x},{value:08x},,,")
                    .map_err(|e| e.to_string())?;
            }
            *steps += 1;
        }
        if machine.bus.gpu.frames != old_frame {
            frames += 1;
            let snapshot = Snapshot::capture(machine, true, None, 0.0);
            let live = machine.bus.gpu.display_rgb();
            let outputs = [
                ("live", machine.bus.gpu.dimensions(), live.as_slice()),
                ("completed", snapshot.dimensions, &snapshot.rgb),
            ];
            let metrics = outputs.map(|(_, _, rgb)| Metrics::measure(rgb));
            let save_pair =
                saved < 12 && (frames <= 3 || metrics.iter().any(Metrics::mostly_solid));
            for (index, (kind, dimensions, rgb)) in outputs.into_iter().enumerate() {
                let m = &metrics[index];
                solid[index] += u64::from(m.mostly_solid());
                writeln!(
                    log,
                    "{kind},{steps},{},{:08x},,{},{},{}",
                    machine.bus.gpu.frames,
                    machine.bus.gpu.status(),
                    m.lit,
                    m.sum,
                    m.dominant
                )
                .map_err(|e| e.to_string())?;
                if save_pair {
                    save(
                        &prefix.with_extension(format!("{kind}-{frames:04}.ppm")),
                        dimensions,
                        rgb,
                    )?;
                }
            }
            saved += usize::from(save_pair);
        }
        machine.bus.spu.drain_audio().for_each(drop);
    }
    log.flush().map_err(|e| e.to_string())?;
    println!(
        "video: {frames} frame boundaries, mostly-solid live={} completed={}; steps={steps}",
        solid[0], solid[1]
    );
    Ok(())
}

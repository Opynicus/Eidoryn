//! Synthetic overdraw benchmark, not a game FPS estimate. Uses original GP0
//! commands and includes scanout/readback; device creation is outside timing.
use eidoryn_psx::gpu::{Gpu, RendererMode};
use std::time::Instant;

fn send(gpu: &mut Gpu, words: &[u32]) {
    for &word in words {
        gpu.gp0(word).unwrap();
    }
}
fn xy(x: u32, y: u32) -> u32 {
    x | (y << 16)
}

fn frame(gpu: &mut Gpu, number: u32) {
    send(gpu, &[0x0200_1828, 0, xy(512, 240)]);
    for layer in 0..32 {
        // The texture page lives below/right of the drawing area. Thirty-two
        // overlapping, modulated, semi-transparent sprites stress raster work.
        send(
            gpu,
            &[
                0xe100_011c | ((layer & 3) << 5),
                0x6680_6080,
                xy((layer * 37 + number) % 384, (layer * 19) % 112),
                xy((layer * 11) & 127, (number + layer * 7) & 127),
                xy(128, 128),
            ],
        );
    }
    std::hint::black_box(gpu.display_rgb());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frames: u32 = std::env::args().nth(1).map_or(Ok(120), |s| s.parse())?;
    if frames == 0 {
        return Err("Frame count must be positive".into());
    }
    let mut base = Gpu::default();
    for (i, value) in base.vram_mut().iter_mut().enumerate() {
        *value = (i as u16).wrapping_mul(7919) | 0x8000;
    }
    send(&mut base, &[0xe400_0000 | 511 | (239 << 10)]);
    base.gp1(0x0300_0000)?;
    base.gp1(0x0800_0002)?; // 512x240 scanout
    let modes = [
        RendererMode::Serial,
        RendererMode::Threaded,
        #[cfg(all(feature = "dx12", target_os = "windows"))]
        RendererMode::Dx12,
        #[cfg(feature = "vulkan")]
        RendererMode::Vulkan,
        #[cfg(feature = "opengl")]
        RendererMode::OpenGl,
        #[cfg(all(feature = "metal", target_os = "macos"))]
        RendererMode::Metal,
    ];
    let mut times = vec![Vec::new(); modes.len()];
    let mut expected = None;
    for round in 0..3 {
        for offset in 0..modes.len() {
            let index = if round % 2 == 0 {
                offset
            } else {
                modes.len() - 1 - offset
            };
            let mut gpu = base.clone();
            gpu.set_renderer(modes[index])?;
            if round == 0 {
                println!("{}: {}", modes[index].label(), gpu.renderer_device());
            }
            for n in 0..10 {
                frame(&mut gpu, n);
            }
            gpu.synchronize_renderer();
            let start = Instant::now();
            for n in 0..frames {
                frame(&mut gpu, n);
            }
            gpu.synchronize_renderer();
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
            if let Some(error) = gpu.renderer_error() {
                return Err(error.into());
            }
            println!(
                "round={} mode={} raster_ms/frame={ms:.3}",
                round + 1,
                modes[index].label()
            );
            times[index].push(ms);
            let vram = gpu.vram();
            if let Some(expected) = &expected {
                assert_eq!(expected, &vram, "Final VRAM differs");
            } else {
                expected = Some(vram);
            }
        }
    }
    for (mode, values) in modes.iter().zip(&mut times) {
        values.sort_by(f64::total_cmp);
        println!("{} median raster_ms/frame={:.3}", mode.label(), values[1]);
    }
    for (mode, values) in modes.iter().zip(&times).skip(1) {
        println!(
            "{}/serial raster speedup={:.2}x; all final VRAM matches",
            mode.label(),
            times[0][1] / values[1]
        );
    }
    Ok(())
}

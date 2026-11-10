//! Repeatable framebuffer conversion measurement; run with --release.
use eidoryn_psx::gpu::Gpu;
use std::{hint::black_box, time::Instant};

fn main() {
    for (name, mode) in [("RGB15 320x240", 1), ("RGB24 640x480", 0x37)] {
        let mut gpu = Gpu::default();
        for (index, pixel) in gpu.vram_mut().iter_mut().enumerate() {
            *pixel = index.wrapping_mul(7919) as u16;
        }
        gpu.gp1(0x0300_0000).unwrap();
        gpu.gp1(0x0800_0000 | mode).unwrap();
        for _ in 0..100 {
            black_box(gpu.display_rgb());
        }
        let start = Instant::now();
        for _ in 0..2_000 {
            black_box(gpu.display_rgb());
        }
        println!(
            "{name}: {:.2} us/frame (2000 conversions)",
            start.elapsed().as_secs_f64() * 500.0
        );
    }
}

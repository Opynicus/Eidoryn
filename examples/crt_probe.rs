//! CRT mask rebuild cost. Rebuilds occur on resize/mode changes, not every frame.
use eidoryn_psx::video_filter::{CrtFilter, OverlayKey};
use std::{hint::black_box, time::Instant};

fn main() {
    for (width, height) in [(640, 480), (1280, 960), (2560, 1920)] {
        let key = OverlayKey::new(width, height, 240, CrtFilter::Classic).unwrap();
        black_box(key.bgra());
        let start = Instant::now();
        for _ in 0..30 {
            black_box(key.bgra());
        }
        let (w, h) = key.dimensions();
        println!(
            "{width}x{height} -> {w}x{h} mask: {:.2} ms/rebuild, {:.2} MiB",
            start.elapsed().as_secs_f64() * 1000.0 / 30.0,
            f64::from(w) * f64::from(h) * 4.0 / 1_048_576.0
        );
    }
}

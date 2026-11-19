use super::{
    ColorDepth, DisplaySettings, DisplaySource, Rasterizer, RenderScale, VRAM_WIDTH,
    color_renderer, draw, pixel, renderer, scanout, state,
};
use crate::gpu::{AntiAliasing, compute::fxaa::Fxaa};

fn enabled(display: DisplaySettings) -> DisplaySettings {
    DisplaySettings {
        anti_aliasing: AntiAliasing::Fxaa,
        ..display
    }
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn fxaa_softens_both_staircases_and_preserves_the_guest_at_all_settings() {
    let (w, h) = (61, 37);
    let mut initial = Rasterizer::default();
    for x in 0..w {
        for y in (4 + x / 3)..(14 + x / 3) {
            initial.vram[y * VRAM_WIDTH + x] = 0x7fff;
        }
    }
    for scale in [RenderScale::Native, RenderScale::X2, RenderScale::X4] {
        for depth in [ColorDepth::Native, ColorDepth::TrueColor] {
            let mut gpu = color_renderer(scale, depth, initial.clone());
            for (x, gray) in [(0, 100), (8, 103)] {
                draw(
                    &mut gpu,
                    &[0x7000_0000 | (gray * 0x0001_0101), x | (28 << 16)],
                    state(),
                );
            }
            let display = scanout(scale, (w, h));
            let baseline = gpu.display(display).unwrap();
            let frame_bytes = (w * h * scale.factor().pow(2) * 4) as u64;
            assert_eq!(gpu.readbacks, [frame_bytes]);
            let raw = DisplaySettings {
                source: DisplaySource::Native,
                ..display
            };
            let native = gpu.display(raw).unwrap();
            let vram = gpu.snapshot().unwrap().vram;
            let movie = DisplaySettings {
                rgb24: true,
                ..display
            };
            assert_eq!(
                gpu.display(movie).unwrap(),
                gpu.display(enabled(movie)).unwrap()
            );
            assert_eq!(gpu.display(enabled(raw)).unwrap(), native);
            let disabled = DisplaySettings {
                disabled: true,
                ..enabled(display)
            };
            gpu.readbacks.clear();
            assert!(gpu.display(disabled).unwrap().iter().all(|&byte| byte == 0));
            assert_eq!(gpu.readbacks, [] as [u64; 0]);
            assert!(matches!(gpu.fxaa, Fxaa::Cold));

            let rgb = gpu.display(enabled(display)).unwrap();
            assert_eq!(gpu.readbacks, [frame_bytes]);
            assert_eq!(gpu.take_presentation_warning(), None);
            assert_eq!(rgb.len(), baseline.len());
            let factor = scale.factor();
            let width = w * factor;
            let mut softened = [0; 2];
            for y in 0..h * factor {
                for x in 18 * factor..(w - 3) * factor {
                    let edge = (4 + (x / factor) / 3) * factor;
                    let row = pixel(&rgb, width, x, y);
                    assert_eq!(row, &[row[0]; 3]);
                    let side = if y.abs_diff(edge) <= factor {
                        Some(0)
                    } else if y.abs_diff(edge + 10 * factor) <= factor {
                        Some(1)
                    } else {
                        None
                    };
                    if let Some(side) = side {
                        softened[side] += usize::from((1..255).contains(&row[0]));
                    } else {
                        assert_eq!(row, pixel(&baseline, width, x, y));
                    }
                }
            }
            assert!(softened.into_iter().all(|count| count > 0));
            for (x, gray) in [(3, 100), (11, 103)] {
                let expected = if depth == ColorDepth::TrueColor {
                    gray
                } else {
                    99
                };
                assert_eq!(pixel(&rgb, width, x * factor, 31 * factor), [expected; 3]);
            }
            assert_eq!(
                gpu.read([Some(31 * VRAM_WIDTH + 3), Some(31 * VRAM_WIDTH + 11)])
                    .unwrap(),
                [0x318c; 2]
            );
            assert_eq!(gpu.display(display).unwrap(), baseline);
            assert_eq!(gpu.display(enabled(raw)).unwrap(), native);
            assert_eq!(gpu.snapshot().unwrap().vram, vram);
        }
    }
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn fxaa_clamps_each_border_and_keeps_thin_strokes_and_flat_colors() {
    let (w, h, pad) = (29_usize, 19_usize, 24_usize);
    let (large_w, large_h) = (w + 2 * pad, h + 2 * pad);
    let mut raster = Rasterizer::default();
    let color = |x: usize, y: usize| match (x, y) {
        (0, _) => 0x001f,
        (_, 0) => 0x03e0,
        (_, _) if x == w - 1 => 0x7c00,
        (_, _) if y == h - 1 => 0x7fe0,
        (4 | 9, 3..=15) | (4..=9, 9) => 0x7fff, // one-pixel H
        (13..=24, _) if y == x / 3 + 3 => 0x7fff, // one-pixel diagonal
        _ => 0,
    };
    for y in 0..large_h {
        for x in 0..large_w {
            let value = color(
                x.saturating_sub(pad).min(w - 1),
                y.saturating_sub(pad).min(h - 1),
            );
            raster.vram[y * VRAM_WIDTH + x + 100] = value;
            if x < w && y < h {
                raster.vram[y * VRAM_WIDTH + x] = color(x, y);
            }
        }
    }
    let mut gpu = renderer(RenderScale::Native, raster);
    let small = enabled(scanout(RenderScale::Native, (w, h)));
    let rgb = gpu.display(small).unwrap();
    let first = gpu.fxaa.target().unwrap().clone();
    assert_eq!(first.size(), (w * h * 4) as u64);
    let padded = DisplaySettings {
        start: (100, 0),
        dimensions: (large_w, large_h),
        ..small
    };
    let reference = gpu.display(padded).unwrap();
    let large = gpu.fxaa.target().unwrap().clone();
    assert_ne!(first, large);
    assert_eq!(large.size(), (large_w * large_h * 4) as u64);
    for y in 0..h {
        for x in 0..w {
            let rgb = pixel(&rgb, w, x, y);
            assert_eq!(
                rgb,
                pixel(&reference, large_w, x + pad, y + pad),
                "border ({x},{y})"
            );
            if color(x, y) == 0x7fff {
                assert!(rgb[0] >= 127, "stroke ({x},{y})");
            }
        }
    }
    assert_eq!(gpu.display(small).unwrap(), rgb);
    assert_eq!(gpu.fxaa.target(), Some(&large));
    // The red strip at the left becomes a 1-column solid display; all taps clamp.
    for dims in [(1, 1), (1, h)] {
        let flat = DisplaySettings {
            dimensions: dims,
            ..small
        };
        assert_eq!(gpu.display(flat).unwrap(), [255, 0, 0].repeat(dims.1));
    }
    assert_eq!(gpu.take_presentation_warning(), None);
    let flat = DisplaySettings {
        start: (300, 0),
        dimensions: (320, 240),
        ..small
    };
    draw(&mut gpu, &[0x0267_24d3, 288, 0x0100_0160], state());
    let original = gpu
        .display(DisplaySettings {
            anti_aliasing: AntiAliasing::Off,
            ..flat
        })
        .unwrap();
    assert_eq!(original, [214, 33, 99].repeat(320 * 240));
    assert_eq!(gpu.display(flat).unwrap(), original);
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn fxaa_setup_failure_falls_back_and_reports_once() {
    let mut gpu = renderer(RenderScale::Native, Rasterizer::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // A uniform-only buffer is an invalid scanout binding. Exercise the device's actual
    // validation scope without exhausting graphics memory or poisoning the rasterizer.
    gpu.fxaa.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &gpu.buffers[5],
        [8, 8],
    );
    assert!(
        gpu.take_presentation_warning()
            .unwrap()
            .contains("FXAA unavailable")
    );
    draw(&mut gpu, &[0x70ff_ffff, 0], state());
    let display = scanout(RenderScale::Native, (16, 9));
    let expected = gpu.display(display).unwrap();
    for _ in 0..2 {
        assert_eq!(gpu.display(enabled(display)).unwrap(), expected);
        assert_eq!(gpu.take_presentation_warning(), None);
    }
}

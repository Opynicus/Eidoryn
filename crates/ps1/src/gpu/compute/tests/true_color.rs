//! Compare a single GP0 stream against the software renderer's guest-visible VRAM.
use super::{
    ColorDepth, ComputeRenderer, DisplaySettings, DisplaySource, DrawState, RasterCommand,
    Rasterizer, RenderScale, VRAM_WIDTH, color_renderer, packet, pixel, scanout, state,
};

const SCALES: [RenderScale; 3] = [RenderScale::Native, RenderScale::X2, RenderScale::X4];

#[test]
fn presentation_storage_matches_the_color_and_sample_count() {
    for (scale, native_mib, rgb888_mib) in [
        (RenderScale::Native, 2, 4),
        (RenderScale::X2, 10, 10),
        (RenderScale::X4, 34, 34),
    ] {
        for (depth, mib) in [
            (ColorDepth::Native, native_mib),
            (ColorDepth::TrueColor, rgb888_mib),
        ] {
            assert_eq!(super::super::raster_bytes(scale, depth), mib * 1024 * 1024);
        }
    }
}

fn execute(gpu: &mut ComputeRenderer, software: &mut Rasterizer, command: RasterCommand) {
    software.execute(command);
    gpu.execute(command).unwrap();
}

fn assert_native_vram(gpu: &Rasterizer, software: &Rasterizer) {
    assert_eq!(gpu.vram.len(), software.vram.len());
    for (i, (actual, expected)) in gpu.vram.iter().zip(&software.vram).enumerate() {
        assert_eq!(actual, expected, "native VRAM word {i:#x}");
    }
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn adjacent_grays_and_dithered_gradient_preserve_guest_pixels_at_every_scale() {
    for scale in SCALES {
        let factor = scale.factor();
        for depth in [ColorDepth::Native, ColorDepth::TrueColor] {
            let mut software = Rasterizer::default();
            let mut gpu = color_renderer(scale, depth, software.clone());
            for (gray, x) in [(100, 0), (103, 8)] {
                execute(
                    &mut gpu,
                    &mut software,
                    packet(&[0x7000_0000 | (gray * 0x0001_0101), x], state()),
                );
            }
            for (y, draw_mode) in [(8 << 16, 0), (12 << 16, 0x200)] {
                let bottom = y + (4 << 16);
                let ramp = [
                    0x3800_0000,
                    y,
                    0x00ff_ffff,
                    y | 256,
                    0,
                    bottom,
                    0x00ff_ffff,
                    bottom | 256,
                ];
                execute(
                    &mut gpu,
                    &mut software,
                    packet(
                        &ramp,
                        DrawState {
                            draw_mode,
                            ..state()
                        },
                    ),
                );
            }
            let input = [Some(VRAM_WIDTH * 2 + 2), Some(VRAM_WIDTH * 2 + 10)];
            assert_eq!(gpu.read(input).unwrap(), [0x318c; 2]);
            assert_native_vram(&gpu.snapshot().unwrap(), &software);
            let native = DisplaySettings {
                source: DisplaySource::Native,
                ..scanout(scale, (256, 16))
            };
            assert_eq!(gpu.display(native).unwrap(), software.display_rgb(native));

            let rgb = gpu.display(scanout(scale, native.dimensions)).unwrap();
            let width = 256 * factor;
            assert_eq!(rgb.len(), width * 16 * factor * 3);
            let true_color = depth == ColorDepth::TrueColor;
            for (x, gray) in [(2, 100), (10, 103)] {
                assert_eq!(
                    pixel(&rgb, width, x * factor, 2 * factor),
                    [if true_color { gray } else { 99 }; 3]
                );
            }
            let row = |y| &rgb[width * y * factor * 3..width * (y * factor + 1) * 3];
            let levels: std::collections::BTreeSet<_> =
                row(9).as_chunks::<3>().0.iter().map(|p| p[0]).collect();
            if true_color {
                assert!(levels.len() >= 255);
                assert_eq!(row(9), row(13));
            } else {
                assert_eq!(levels.len(), 32);
                assert_ne!(row(9), row(13));
            }
        }
    }
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn blends_round_saturate_and_preserve_sample_mask() {
    let mut gpu = color_renderer(
        RenderScale::Native,
        ColorDepth::TrueColor,
        Rasterizer::default(),
    );
    let check_mask = DrawState {
        mask_check: true,
        ..state()
    };
    let force_mask = DrawState {
        mask_set: true,
        ..state()
    };
    let display = scanout(RenderScale::Native, (6, 1));
    for mode in 0..4 {
        super::draw(&mut gpu, &[0x6864_01c9, mode], state()); // [201, 1, 100]
        super::draw(
            &mut gpu,
            &[0x6a67_6769, mode],
            DrawState {
                draw_mode: mode << 5,
                ..state()
            },
        );
    }
    // Green bit 7 shares the bit position of a native mask, but is ordinary color in RGB888.
    for x in 4..6 {
        super::draw(
            &mut gpu,
            &[0x6800_8001, x],
            if x == 4 { check_mask } else { force_mask },
        );
        super::draw(&mut gpu, &[0x6867_6767, x], check_mask);
    }
    let first = gpu.display(display).unwrap();
    assert_eq!(
        first,
        [
            153, 52, 101, 255, 104, 203, 96, 0, 0, 227, 26, 125, 103, 103, 103, 1, 128, 0
        ]
    );
    super::draw(&mut gpu, &[0x6a67_6769, 0], state());
    assert_eq!(
        pixel(&gpu.display(display).unwrap(), 6, 0, 0),
        [129, 77, 102]
    );
    let words = gpu.read([Some(4), Some(5)]).unwrap();
    assert_eq!(words, [0x318c, 0x8200]);
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn direct_and_indexed_sprites_use_native_texels_for_modulation_transparency_and_feedback() {
    // All three texture formats encode the same texels: RGB, transparent zero, marked black.
    let mut software = Rasterizer::default();
    let color = 31 | (12 << 5) | (1 << 10);
    software.vram[64..67].copy_from_slice(&[color, 0, 0x8000]);
    software.vram[128] = 0x321;
    software.vram[192..194].copy_from_slice(&[0x0201, 3]);
    software.vram[257..260].copy_from_slice(&[color, 0, 0x8000]);
    let mut gpu = color_renderer(RenderScale::X2, ColorDepth::TrueColor, software.clone());
    for (row, page) in [(0, 0x101), (1, 2), (2, 0x83)] {
        let mode = DrawState {
            draw_mode: page,
            ..state()
        };
        let y = row << 16;
        execute(&mut gpu, &mut software, packet(&[0x7067_6767, y], state()));
        for (x, command, u) in [
            (0, 0x6d00_0000, 0),
            (1, 0x6c80_8080, 0),
            (2, 0x6c40_c0ff, 0),
            (3, 0x6d00_0000, 1),
            (4, 0x6d00_0000, 2),
            (5, 0x6f00_0000, 2),
            (6, 0x6f00_0000, 0),
        ] {
            execute(
                &mut gpu,
                &mut software,
                packet(&[command, y | x, (16 << 16) | u], mode),
            );
        }
    }
    // Draw 103 into the texture page, then sample it as an unmodulated sprite.
    execute(&mut gpu, &mut software, packet(&[0x6867_6767, 7], state()));
    execute(
        &mut gpu,
        &mut software,
        packet(
            &[0x6d00_0000, 8, 7],
            DrawState {
                draw_mode: 0x100,
                ..state()
            },
        ),
    );
    let rgb = gpu.display(scanout(RenderScale::X2, (9, 3))).unwrap();
    for y in 0..3 {
        for (x, expected) in [
            [255, 99, 8],
            [255, 99, 8],
            [255, 148, 4],
            [103; 3],
            [0; 3],
            [51; 3],
            [255, 99, 8],
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(pixel(&rgb, 18, x * 2, y * 2), expected, "cell ({x},{y})");
        }
    }
    assert_eq!(pixel(&rgb, 18, 14, 0), [103; 3]);
    assert_eq!(pixel(&rgb, 18, 16, 0), [99; 3]);
    assert_eq!(
        gpu.read([Some(4), Some(5)]).unwrap(),
        [0x8000, u32::from(software.vram[5])]
    );
    assert_native_vram(&gpu.snapshot().unwrap(), &software);
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn fill_copy_upload_and_packed_movie_preserve_each_representation() {
    let mut software = Rasterizer::default();
    software.vram[1022] = 0xffff;
    let mut gpu = color_renderer(RenderScale::X4, ColorDepth::TrueColor, software.clone());
    let force = DrawState {
        mask_set: true,
        ..state()
    };
    let protect = DrawState {
        mask_check: true,
        ..state()
    };
    let mut step = |cmd| execute(&mut gpu, &mut software, cmd);
    // Fill ignores destination protection and clears the mask. Sequential copy
    // wraps at x=1024 and propagates the source color plus a forced mask.
    step(packet(&[0x0267_6767, 1008, 0x0001_0010], protect));
    step(packet(&[0x8000_0000, 1022, 1023, 0x0001_0004], force));
    step(RasterCommand::Pixel(0, 0x7fff, protect));
    step(RasterCommand::Pixel(2, 0x1234, state()));
    let display = DisplaySettings {
        start: (1022, 0),
        ..scanout(RenderScale::X4, (5, 1))
    };
    let rgb = gpu.display(display).unwrap();
    for y in 0..4 {
        for x in 0..20 {
            assert_eq!(
                pixel(&rgb, 20, x, y),
                if x < 16 { [103; 3] } else { [165, 140, 33] }
            );
        }
    }
    assert_eq!(gpu.read([Some(1022), Some(1)]).unwrap(), [0x318c, 0xb18c]);
    let movie = DisplaySettings {
        rgb24: true,
        dimensions: (4, 1),
        ..display
    };
    assert_eq!(gpu.display(movie).unwrap(), software.display_rgb(movie));
    assert_eq!(gpu.display(display).unwrap(), rgb);
    let snapshot = gpu.snapshot().unwrap();
    assert_native_vram(&snapshot, &software);
    gpu.start(snapshot).unwrap();
    assert_eq!(pixel(&gpu.display(display).unwrap(), 20, 0, 0), [99; 3]);
    super::draw(&mut gpu, &[0x6867_6767, 1022], state());
    assert_eq!(pixel(&gpu.display(display).unwrap(), 20, 0, 0), [103; 3]);
}

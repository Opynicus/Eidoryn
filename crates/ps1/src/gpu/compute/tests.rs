//! Headless shader regressions, including Vulkan software implementations.
//! Run with `cargo test -p eidoryn-ps1 --features vulkan gpu::compute::tests -- --ignored`.

use super::ComputeRenderer;
use crate::gpu::{
    DisplaySettings, RenderScale, VRAM_WIDTH,
    rasterizer::{DrawState, RasterCommand, Rasterizer},
};

fn renderer(scale: RenderScale, initial: Rasterizer) -> ComputeRenderer {
    let backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.enumerate_adapters(backends))
        .into_iter()
        .next()
        .expect("the shader test requires a Vulkan adapter");
    let mut gpu =
        ComputeRenderer::from_adapter(&adapter, "Vulkan", std::sync::Arc::default(), scale)
            .expect("compile shaders and allocate scaled buffers");
    gpu.start(initial).expect("seed native and enhanced VRAM");
    gpu
}

fn state() -> DrawState {
    DrawState {
        draw_max: (1023, 511),
        ..DrawState::default()
    }
}

fn draw(gpu: &mut ComputeRenderer, words: &[u32], state: DrawState) {
    let mut packet = [0; 12];
    packet[..words.len()].copy_from_slice(words);
    gpu.execute(RasterCommand::Packet(packet, state)).unwrap();
}

fn scanout(scale: RenderScale, dimensions: (usize, usize)) -> DisplaySettings {
    DisplaySettings {
        dimensions,
        start: (0, 0),
        disabled: false,
        rgb24: false,
        scale,
    }
}

fn pixel(rgb: &[u8], width: usize, x: usize, y: usize) -> &[u8] {
    let offset = (y * width + x) * 3;
    &rgb[offset..offset + 3]
}

#[test]
#[ignore = "requires a Vulkan adapter (software Vulkan is sufficient)"]
fn transfers_and_feedback_keep_native_words_and_enhanced_coverage_in_sync() {
    for (scale, factor) in [(RenderScale::X2, 2), (RenderScale::X4, 4)] {
        let mut initial = Rasterizer::default();
        initial.vram[3] = 0x03e0;
        let mut gpu = renderer(scale, initial);
        let display = scanout(scale, (12, 1));
        let width = 12 * factor;
        let check_mask = DrawState {
            mask_check: true,
            ..state()
        };
        // The diagonal includes native center (3.5, 0.5), leaving some samples green.
        draw(
            &mut gpu,
            &[0x2000_00ff, 4, 4 | (4 << 16), 4 << 16],
            DrawState {
                mask_set: true,
                ..state()
            },
        );
        // A masked upload tests the native word once for the entire sample block.
        gpu.execute(RasterCommand::Pixel(3, 0x7c00, check_mask))
            .unwrap();
        // Direct-color feedback reads red native texels, including its preceding write.
        draw(
            &mut gpu,
            &[0x6500_0000, 4, 3, (1 << 16) | 2],
            DrawState {
                draw_mode: 0x100,
                ..state()
            },
        );
        // First copy a detailed pixel, then propagate it across overlapping destinations.
        draw(&mut gpu, &[0x8000_0000, 3, 8, 0x0001_0001], state());
        draw(&mut gpu, &[0x8000_0000, 8, 9, 0x0001_0003], state());
        let rgb = gpu.display(display).unwrap();
        assert_eq!(rgb.len(), width * factor * 3);
        assert_eq!(gpu.read([Some(3), Some(11)]).unwrap(), [0x801f; 2]);
        for y in 0..factor {
            for x in 0..factor {
                let expected = if x + y + 1 >= factor {
                    [255, 0, 0]
                } else {
                    [0, 255, 0]
                };
                for cell in [3, 8, 9, 10, 11] {
                    assert_eq!(pixel(&rgb, width, cell * factor + x, y), expected);
                }
                for cell in [4, 5] {
                    assert_eq!(pixel(&rgb, width, cell * factor + x, y), [255, 0, 0]);
                }
            }
        }

        // A translucent blue draw blends with green only in the unprotected samples.
        draw(&mut gpu, &[0x6aff_0000, 3], check_mask);
        let rgb = gpu.display(display).unwrap();
        assert_eq!(pixel(&rgb, width, 3 * factor, 0), [0, 123, 123]);
        assert_eq!(pixel(&rgb, width, 4 * factor - 1, factor - 1), [255, 0, 0]);
        assert_eq!(gpu.read([Some(3), None]).unwrap(), [0x801f, 0]);

        // An accepted upload replaces all samples; RGB24 reads the native packed bytes.
        gpu.execute(RasterCommand::Pixel(3, 0x1234, state()))
            .unwrap();
        gpu.execute(RasterCommand::Pixel(4, 0x5678, state()))
            .unwrap();
        let rgb = gpu.display(display).unwrap();
        for y in 0..factor {
            for x in 0..factor {
                assert_eq!(pixel(&rgb, width, 3 * factor + x, y), [165, 140, 33]);
            }
        }
        let rgb24 = DisplaySettings {
            rgb24: true,
            scale: RenderScale::Native,
            start: (3, 0),
            dimensions: (2, 1),
            ..display
        };
        assert_eq!(
            gpu.display(rgb24).unwrap(),
            [0x34, 0x12, 0x78, 0x56, 0x1f, 0x80]
        );
    }
}

#[test]
#[ignore = "requires a Vulkan adapter (software Vulkan is sufficient)"]
fn four_times_interpolates_at_subpixel_centers_without_overflow() {
    let scale = RenderScale::X4;
    let mut initial = Rasterizer::default();
    // Direct-color texels encode u, exposing the interpolation position.
    for u in 0..16 {
        initial.vram[64 + u] = (u + 1) as u16;
    }
    let mut gpu = renderer(scale, initial);
    draw(
        &mut gpu,
        &[0x2500_0000, 0, 0, 4, (0x101 << 16) | 16, 4 << 16, 0],
        state(),
    );
    let display = scanout(scale, (1, 1));
    let rgb = gpu.display(display).unwrap();
    assert_eq!(&rgb[..12], [8, 0, 0, 16, 0, 0, 24, 0, 0, 33, 0, 0]);
    assert_eq!(gpu.read([Some(0), None]).unwrap(), [3, 0]);

    // The largest legal triangle can overflow i32 when scaled weights are
    // multiplied by an 8-bit attribute. Clip its draw area to keep this test small.
    let corner = DrawState {
        draw_max: (1, 1),
        ..state()
    };
    draw(
        &mut gpu,
        &[0x30ff_ffff, 0, 0x00ff_0000, 1023, 0x0000_00ff, 511 << 16],
        corner,
    );
    assert_eq!(gpu.display(display).unwrap(), [255; 4 * 4 * 3]);

    let index = 255 * VRAM_WIDTH + 64 + 255;
    gpu.execute(RasterCommand::Pixel(index, 31 << 10, state()))
        .unwrap();
    draw(
        &mut gpu,
        &[0x2500_0000, 0, 0xffff, 1023, 0x0101_ffff, 511 << 16, 0xffff],
        corner,
    );
    assert_eq!(gpu.display(display).unwrap(), [0, 0, 255].repeat(4 * 4));
}

#[test]
#[ignore = "requires a Vulkan adapter (software Vulkan is sufficient)"]
fn maximum_enhanced_scanout_wraps_and_matches_the_advertised_size() {
    let scale = RenderScale::X4;
    let mut initial = Rasterizer::default();
    initial.vram[0] = 0x7fff;
    let mut gpu = renderer(scale, initial);
    // The final native cell wraps back to VRAM origin on both axes.
    let display = DisplaySettings {
        start: (1024 - 639, 512 - 479),
        ..scanout(scale, (640, 480))
    };
    let rgb = gpu.display(display).unwrap();
    let (width, height) = (2560, 1920);
    assert_eq!(rgb.len(), width * height * 3);
    assert_eq!(pixel(&rgb, width, width - 5, height - 1), [0; 3]);
    assert_eq!(pixel(&rgb, width, width - 1, height - 5), [0; 3]);
    for y in height - 4..height {
        for x in width - 4..width {
            assert_eq!(pixel(&rgb, width, x, y), [255; 3]);
        }
    }
    assert_eq!(gpu.snapshot().unwrap().vram.len(), 1024 * 512);
}

#[test]
#[ignore = "requires a Vulkan adapter (software Vulkan is sufficient)"]
fn wrapped_diagonal_copy_propagates_subpixels_across_dispatches() {
    let scale = RenderScale::X2;
    let mut gpu = renderer(scale, Rasterizer::default());
    let xy = |x: u32, y: u32| x | (y << 16);
    draw(
        &mut gpu,
        &[0x2000_00ff, xy(1004, 500), xy(1004, 504), xy(1000, 504)],
        state(),
    );
    // (1003,500) straddles the edge. Each copied row supplies the next row,
    // across both VRAM seams and the boundary between two 32-row dispatches.
    draw(
        &mut gpu,
        &[0x8000_0000, xy(1000, 500), xy(1001, 501), xy(128, 35)],
        state(),
    );
    let display = DisplaySettings {
        start: (1000, 500),
        ..scanout(scale, (39, 36))
    };
    let rgb = gpu.display(display).unwrap();
    for row in 0..=35 {
        let x = (3 + row) * 2;
        let y = row * 2;
        assert_eq!(pixel(&rgb, 78, x, y), [0; 3], "row {row}");
        assert_eq!(pixel(&rgb, 78, x + 1, y), [255, 0, 0], "row {row}");
        assert_eq!(pixel(&rgb, 78, x, y + 1), [255, 0, 0], "row {row}");
        assert_eq!(pixel(&rgb, 78, x + 1, y + 1), [255, 0, 0], "row {row}");
    }
    let last = (535 % 512) * VRAM_WIDTH + (1038 % 1024);
    assert_eq!(gpu.read([Some(last), None]).unwrap(), [31, 0]);
}

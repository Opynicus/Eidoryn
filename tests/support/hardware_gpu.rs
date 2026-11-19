//! Shared pixel-accurate hardware tests for all wgpu backends.
use super::HARDWARE_MODE;
use eidoryn_psx::{
    gpu::{AntiAliasing, ColorDepth, DisplayFrame, Gpu, RenderScale, RendererMode},
    machine::Machine,
    timers::Timers,
};

fn send(gpu: &mut Gpu, words: &[u32]) {
    for &word in words {
        gpu.gp0(word).unwrap();
    }
}
fn xy(x: u32, y: u32) -> u32 {
    x | (y << 16)
}

fn draw_fxaa_scene(gpu: &mut Gpu) {
    gpu.gp1(0x0300_0000).unwrap();
    gpu.gp1(0x0500_0000).unwrap();
    send(
        gpu,
        &[
            0xe407_ffff,
            0x28ff_ffff,
            xy(8, 16),
            xy(104, 48),
            xy(8, 28),
            xy(104, 60),
            0x6064_6464,
            xy(0, 90),
            xy(16, 16),
            0x6067_6767,
            xy(16, 90),
            xy(16, 16),
        ],
    );
}

fn frame_pixel(frame: &DisplayFrame, x: usize, y: usize) -> &[u8] {
    let offset = (y * frame.dimensions.0 + x) * 3;
    &frame.rgb[offset..offset + 3]
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn fxaa_smooths_only_presentation_across_all_scales_and_color_depths() {
    let mut software = Gpu::default();
    draw_fxaa_scene(&mut software);
    let native = software.display_rgb();
    let vram = software.vram();
    let mut gpu = Gpu::default();
    for scale in [RenderScale::Native, RenderScale::X2, RenderScale::X4] {
        let factor = scale.factor();
        for depth in [ColorDepth::Native, ColorDepth::TrueColor] {
            gpu.set_renderer_config(HARDWARE_MODE, scale, depth)
                .unwrap();
            gpu.gp1(0x0800_0000).unwrap();
            gpu.set_anti_aliasing(AntiAliasing::Off);
            draw_fxaa_scene(&mut gpu);
            let original = gpu.live_frame();
            gpu.set_anti_aliasing(AntiAliasing::Fxaa);
            let filtered = gpu.live_frame();
            assert_eq!(filtered.dimensions, original.dimensions);
            assert_eq!(filtered.native_dimensions, software.dimensions());
            assert_eq!(filtered.rgb.len(), original.rgb.len());
            assert_eq!(frame_pixel(&filtered, 40 * factor, 32 * factor), &[255; 3]);
            assert_eq!(frame_pixel(&filtered, 40 * factor, 12 * factor), &[0; 3]);
            for (x, color) in [(8, 100), (24, 103)] {
                let gray = if depth == ColorDepth::Native {
                    99
                } else {
                    color
                };
                assert_eq!(frame_pixel(&filtered, x * factor, 96 * factor), &[gray; 3]);
            }
            assert!(
                (14 * factor..61 * factor).any(|y| (9 * factor..102 * factor).any(|x| {
                    let gray = frame_pixel(&filtered, x, y)[0];
                    gray != 0 && gray != 255 && frame_pixel(&original, x, y)[0] != gray
                })),
                "diagonal did not gain intermediate coverage at {scale:?} {depth:?}"
            );
            assert_eq!(gpu.display_rgb(), native);
            assert_eq!(gpu.vram(), vram);
            send(&mut gpu, &[0xc000_0000, xy(15, 96), xy(2, 1)]);
            assert_eq!(gpu.read(), 0x318c_318c);
            gpu.set_anti_aliasing(AntiAliasing::Off);
            assert_eq!(gpu.live_frame().rgb, original.rgb);

            gpu.gp1(0x0800_0010).unwrap();
            let movie = gpu.live_frame();
            gpu.set_anti_aliasing(AntiAliasing::Fxaa);
            assert_eq!(gpu.live_frame().rgb, movie.rgb);
            assert_eq!(&*movie.rgb, gpu.display_rgb());
            gpu.gp1(0x0300_0001).unwrap();
            assert!(gpu.live_frame().rgb.iter().all(|&v| v == 0));
            assert_eq!(gpu.take_presentation_warning(), None);
        }
    }
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn fxaa_toggle_applies_after_the_latched_field_and_survives_state_restore() {
    let mut machine = Machine::demo();
    let gpu = &mut machine.bus.gpu;
    gpu.set_renderer(HARDWARE_MODE).unwrap();
    draw_fxaa_scene(gpu);
    let original = gpu.live_frame();
    let mut timers = Timers::default();
    while gpu.tick(1000, &mut timers) & 1 == 0 {}
    let at_vblank = (gpu.frames, gpu.commands, gpu.revision, gpu.status());
    gpu.set_anti_aliasing(AntiAliasing::Fxaa);
    assert_eq!(
        at_vblank,
        (gpu.frames, gpu.commands, gpu.revision, gpu.status())
    );
    assert_eq!(gpu.completed_frame().unwrap().rgb, original.rgb);
    while gpu.tick(1000, &mut timers) & 1 == 0 {}
    let filtered = gpu.completed_frame().unwrap().rgb.clone();
    assert_ne!(filtered, original.rgb);
    gpu.set_anti_aliasing(AntiAliasing::Off);
    assert_eq!(gpu.completed_frame().unwrap().rgb, filtered);
    let mut restored = Machine::load_state(&machine.save_state().unwrap()).unwrap();
    let gpu = &mut restored.bus.gpu;
    gpu.set_anti_aliasing(AntiAliasing::Fxaa);
    assert_eq!(gpu.anti_aliasing(), AntiAliasing::Off);
    gpu.set_renderer(HARDWARE_MODE).unwrap();
    assert_eq!(gpu.anti_aliasing(), AntiAliasing::Fxaa);
    assert_eq!(gpu.completed_frame().unwrap().rgb, filtered);
    assert_eq!(gpu.live_frame().rgb, filtered);
    gpu.set_renderer(RendererMode::Threaded).unwrap();
    assert_eq!(gpu.anti_aliasing(), AntiAliasing::Off);
    assert_eq!(gpu.live_frame().rgb, original.rgb);
    assert_eq!(gpu.completed_frame().unwrap().rgb, filtered);
}

fn pair() -> [Gpu; 2] {
    let mut serial = Gpu::default();
    let mut random = 0x3141_5926_u32;
    for value in serial.vram_mut() {
        *value = next(&mut random) as u16;
    }
    send(&mut serial, &[0xe400_0000 | 1023 | (511 << 10)]);
    serial.gp1(0x0300_0000).unwrap();
    let mut hardware = serial.clone();
    hardware.set_renderer(HARDWARE_MODE).unwrap();
    assert_eq!(hardware.renderer_mode(), HARDWARE_MODE);
    eprintln!("Hardware test: {}", hardware.renderer_device());
    [serial, hardware]
}
fn next(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}
fn equal(serial: &Gpu, hardware: &Gpu) {
    hardware.synchronize_renderer();
    assert_eq!(hardware.renderer_error(), None);
    let a = serial.vram();
    let b = hardware.vram();
    if let Some(i) = a.iter().zip(&b).position(|(a, b)| a != b) {
        panic!(
            "VRAM mismatch at ({}, {}): software={:04x}, {}={:04x}",
            i % 1024,
            i / 1024,
            a[i],
            hardware.renderer_mode().label(),
            b[i]
        );
    }
    assert_eq!(serial.status(), hardware.status());
    assert_eq!(serial.display_rgb(), hardware.display_rgb());
    assert_eq!(hardware.renderer_error(), None);
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn hardware_draws_match_software_for_blending_masks_textures_lines_and_triangles() {
    let [mut serial, mut hardware] = pair();
    let mut random = 0x1234_5678;
    for case in 0..400_u32 {
        let mut words = vec![
            0xe100_0000 | (case % 4) << 7 | ((case % 4) << 5) | 0x200 | (case & 3) << 12,
            0xe200_0000 | (next(&mut random) & 0xfffff),
            0xe300_0000 | 3 | (5 << 10),
            0xe400_0000 | 0x014a | (290 << 10),
            0xe500_0000 | (case & 7),
            0xe600_0000 | (case & 3),
        ];
        let mode = (case % 4) << 7 | ((case % 4) << 5);
        let color = next(&mut random) & 0x00ff_ffff;
        let pos = |seed: &mut u32| xy(next(seed) % 380, next(seed) % 330);
        let textured = case & 4 != 0;
        let shaded = case & 16 != 0;
        let quad = case & 8 != 0;
        // Cover all polygon opcodes with randomized winding, palette, window,
        // interpolation, semi-transparency and dithering.
        words.push(0x2000_0000 | ((case & 31) << 24) | color);
        for vertex in 0..if quad { 4 } else { 3 } {
            if shaded && vertex != 0 {
                words.push(next(&mut random) & 0x00ff_ffff);
            }
            words.push(pos(&mut random));
            if textured {
                words.push(
                    (next(&mut random) & 65535)
                        | if vertex == 1 {
                            mode << 16
                        } else {
                            (next(&mut random) & 65535) << 16
                        },
                );
            }
        }
        // Textured rectangles exercise UV flips and clipped texture origins.
        words.extend([
            0x6400_0000 | ((case & 3) << 24) | color,
            pos(&mut random),
            next(&mut random),
            xy(19, 23),
        ]);
        words.extend([
            0x5200_0000 | color,
            xy(0x7f8, 4),
            next(&mut random) & 0x00ff_ffff,
            xy(42, 35),
        ]);
        words.extend([
            0x4800_0000 | color,
            xy(35, 33),
            xy(18, 41),
            xy(8, 9),
            0x5000_5000,
        ]);
        send(&mut serial, &words);
        send(&mut hardware, &words);
        if case % 20 == 19 {
            equal(&serial, &hardware);
        }
    }
    equal(&serial, &hardware);
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn transfers_wrap_and_preserve_feedback_and_readback_order() {
    let [mut serial, mut hardware] = pair();
    for gpu in [&mut serial, &mut hardware] {
        // Fill wraps in both axes and ignores mask state.
        send(gpu, &[0xe600_0003, 0x0200_37fa, xy(1019, 510), xy(32, 4)]);
        send(gpu, &[0xe600_0000, 0xa000_0000, xy(1023, 511), xy(3, 2)]);
        gpu.gp0(0x5678_1234).unwrap();
    }
    equal(&serial, &hardware); // partial CPU upload barrier
    for gpu in [&mut serial, &mut hardware] {
        send(gpu, &[0xdef0_9abc, 0x4321_8765]);
        send(gpu, &[0xc000_0000, xy(1023, 511), xy(3, 2)]);
    }
    for expected in [0x5678_1234, 0xdef0_9abc, 0x4321_8765] {
        assert_eq!(serial.read(), expected);
        assert_eq!(hardware.read(), expected);
    }
    for mask in 0..4 {
        for gpu in [&mut serial, &mut hardware] {
            send(
                gpu,
                &[
                    0xe600_0000 | mask,
                    0x8000_0000,
                    xy(20, 20),
                    xy(21, 21),
                    xy(17, 19),
                    0x8000_0000,
                    xy(1020, 510),
                    xy(1018, 511),
                    xy(9, 5),
                    0x8000_0000,
                    xy(0, 0),
                    xy(512, 256),
                    xy(120, 110),
                    0xe100_0100, // direct-color page at (0,0): self-texture feedback
                    0x6500_0000,
                    xy(1, 0),
                    0,
                    xy(63, 1),
                    0xe100_0000, // indexed texture and palette alias the destination
                    0x6500_0000,
                    xy(1, 1),
                    0,
                    xy(31, 17),
                ],
            );
        }
        equal(&serial, &hardware);
    }
    for gpu in [&mut serial, &mut hardware] {
        gpu.gp1(0x0800_0013).unwrap(); // 640x240 RGB24, wrap byte stream
        gpu.gp1(0x0500_0000 | 1023 | (511 << 10)).unwrap();
    }
    equal(&serial, &hardware);
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn hardware_vblank_snapshot_precedes_buffer_clear_and_survives_reset_and_switch() {
    let mut gpu = Gpu::default();
    gpu.set_renderer(HARDWARE_MODE).unwrap();
    gpu.gp1(0x0300_0000).unwrap();
    send(&mut gpu, &[0x0200_00ff, 0, xy(256, 240)]);
    let mut timers = Timers::default();
    while gpu.tick(1000, &mut timers) & 1 == 0 {}
    send(&mut gpu, &[0x0200_0000, 0, xy(256, 240)]);
    gpu.gp1(0x0300_0001).unwrap();
    let completed = gpu.completed_frame().unwrap().rgb.clone();
    assert_eq!(&completed[..6], &[255, 0, 0, 255, 0, 0]);
    assert!(gpu.display_rgb().iter().all(|c| *c == 0));
    gpu.gp1(0).unwrap();
    assert_eq!(gpu.renderer_mode(), HARDWARE_MODE);
    assert_eq!(gpu.completed_frame().unwrap().rgb, completed);
    send(&mut gpu, &[0x0200_ff00, 0, xy(16, 1)]);
    let checkpoint = gpu.clone();
    assert_eq!(checkpoint.renderer_mode(), RendererMode::Serial);
    assert_eq!(checkpoint.vram()[0], 31 << 5);
    for mode in [RendererMode::Threaded, HARDWARE_MODE, RendererMode::Serial] {
        gpu.set_renderer(mode).unwrap();
        assert_eq!(gpu.renderer_mode(), mode);
        assert_eq!(gpu.vram(), checkpoint.vram());
    }
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn scale_switch_preserves_guest_vram_and_latched_field_dimensions() {
    let [mut serial, mut hardware] = pair();
    for scale in [RenderScale::X2, RenderScale::X4, RenderScale::Native] {
        hardware
            .set_renderer_with_scale(HARDWARE_MODE, scale)
            .unwrap();
        assert_eq!(hardware.render_scale(), scale);
        for gpu in [&mut serial, &mut hardware] {
            gpu.gp1(0x0800_0001).unwrap(); // RGB15, 320x240
            send(
                gpu,
                &[
                    0x3000_00ff,
                    xy(0, 0),
                    0x00ff_0000,
                    xy(16, 7),
                    0x0000_ff00,
                    xy(2, 8),
                ],
            );
        }
        equal(&serial, &hardware);
        hardware.latch_display();
        hardware.gp1(0x0800_0013).unwrap(); // switch to 640x240 packed RGB24
        let completed = hardware.completed_frame().unwrap();
        assert_eq!(completed.native_dimensions, (320, 240));
        assert_eq!(
            completed.dimensions,
            (320 * scale.factor(), 240 * scale.factor())
        );
        assert_eq!(
            completed.rgb.len(),
            completed.dimensions.0 * completed.dimensions.1 * 3
        );
        let live = hardware.live_frame();
        assert_eq!(live.dimensions, (640, 240));
        assert_eq!(live.native_dimensions, live.dimensions);
        assert_eq!(&*live.rgb, hardware.display_rgb());
    }
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn color_depth_changes_scanout_while_gpuread_matches_software_at_each_scale() {
    let mut software = Gpu::default();
    software.gp1(0x0300_0000).unwrap();
    let draw = [0xe407_ffff, 0x6864_6464, xy(0, 0), 0x6867_6767, xy(1, 0)];
    let read_pair = [0xc000_0000, xy(0, 0), xy(2, 1)];
    send(&mut software, &draw);
    send(&mut software, &read_pair);
    let guest_pair = software.read();
    assert_eq!(guest_pair, 0x318c_318c);
    let mut gpu = Gpu::default();
    gpu.gp1(0x0300_0000).unwrap();
    for scale in [RenderScale::Native, RenderScale::X2, RenderScale::X4] {
        for depth in [ColorDepth::Native, ColorDepth::TrueColor] {
            gpu.set_renderer_config(HARDWARE_MODE, scale, depth)
                .unwrap();
            gpu.gp1(0x0800_0000).unwrap();
            send(&mut gpu, &draw);
            send(&mut gpu, &read_pair);
            assert_eq!(gpu.read(), guest_pair);
            assert_eq!(gpu.vram(), software.vram());
            assert_eq!(gpu.display_rgb(), software.display_rgb());
            let frame = gpu.live_frame();
            let factor = scale.factor();
            let (w, h) = software.dimensions();
            assert_eq!(frame.dimensions, (w * factor, h * factor));
            let grays = if depth == ColorDepth::TrueColor {
                [100, 103]
            } else {
                [99, 99]
            };
            for (x, gray) in grays.into_iter().enumerate() {
                let offset = 3 * x * factor;
                assert_eq!(&frame.rgb[offset..offset + 3], &[gray; 3]);
            }
            gpu.gp1(0x0800_0010).unwrap();
            let movie = gpu.live_frame();
            assert_eq!(movie.dimensions, (w, h));
            assert_eq!(&movie.rgb[..3], &[0x8c, 0x31, 0x8c]);
            assert_eq!(&*movie.rgb, gpu.display_rgb());
            assert_eq!(gpu.renderer_error(), None);
        }
    }
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn true_color_v1_preview_survives_rebuild_and_failed_switch() {
    let mut machine = Machine::demo();
    let gpu = &mut machine.bus.gpu;
    gpu.set_renderer_config(HARDWARE_MODE, RenderScale::X2, ColorDepth::TrueColor)
        .unwrap();
    gpu.gp1(0x0300_0000).unwrap();
    gpu.gp1(0x0500_0000).unwrap();
    let point = [0xe407_ffff, 0x6867_6767, xy(0, 0)];
    send(gpu, &point);
    let mut timers = Timers::default();
    while gpu.tick(1000, &mut timers) & 1 == 0 {}
    let unavailable = if cfg!(target_os = "macos") {
        RendererMode::Dx12
    } else {
        RendererMode::Metal
    };
    assert!(
        gpu.set_renderer_config(unavailable, RenderScale::X4, ColorDepth::Native)
            .is_err()
    );
    assert_eq!(
        (gpu.renderer_mode(), gpu.render_scale(), gpu.color_depth()),
        (HARDWARE_MODE, RenderScale::X2, ColorDepth::TrueColor)
    );
    gpu.set_renderer_config(HARDWARE_MODE, RenderScale::X2, ColorDepth::TrueColor)
        .unwrap();
    assert_eq!(&gpu.live_frame().rgb[..3], &[103; 3]);
    assert_eq!(&gpu.completed_frame().unwrap().rgb[..3], &[103; 3]);

    let bytes = machine.save_state().unwrap();
    let mut restored = Machine::load_state(&bytes).unwrap();
    let gpu = &mut restored.bus.gpu;
    let preview = gpu.completed_frame().unwrap();
    assert_eq!(preview.dimensions, gpu.dimensions());
    assert_eq!(&preview.rgb[..3], &[103; 3]);
    gpu.set_renderer_config(HARDWARE_MODE, RenderScale::X4, ColorDepth::TrueColor)
        .unwrap();
    assert_eq!(gpu.vram()[0], 0x318c);
    assert_eq!(&gpu.live_frame().rgb[..3], &[99; 3]);
    send(gpu, &point);
    assert_eq!(&gpu.live_frame().rgb[..3], &[103; 3]);
    assert_eq!(gpu.renderer_error(), None);
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn bounded_batches_handle_large_uploads_and_maximum_interpolation_without_overflow() {
    let [mut serial, mut hardware] = pair();
    for gpu in [&mut serial, &mut hardware] {
        send(gpu, &[0xa000_0000, xy(997, 497), xy(0, 130)]); // zero width means 1024
        for n in 0..1024 * 130 / 2_u32 {
            gpu.gp0(n.wrapping_mul(7919)).unwrap();
        }
        send(
            gpu,
            &[
                0xe100_0200,
                0xe600_0000,
                0x3000_00ff,
                xy(0, 0),
                0x00ff_ffff,
                xy(1023, 0),
                0x0000_ff00,
                xy(0, 511),
                0x3000_ff00,
                xy(1023, 511),
                0x00ff_ffff,
                xy(0, 511),
                0x0000_00ff,
                xy(1023, 0),
                // Oversized primitives must be discarded before interpolation.
                0x2000_00ff,
                xy(0x07ff, 0),
                xy(1023, 0),
                xy(0, 500),
                // Negative positions and offsets, a flipped/wrapped UV sprite.
                0xe500_0000 | 0x07f9 | (0x07fd << 11),
                0xe100_310f,
                0x6500_0000,
                xy(0, 0),
                xy(253, 251),
                xy(31, 29),
                0xe500_0000,
            ],
        );
    }
    equal(&serial, &hardware);
}

#[test]
#[ignore = "requires a physical GPU with the selected backend"]
fn runner_can_enable_and_disable_hardware_without_resetting_the_guest() {
    use eidoryn_psx::{
        machine::Machine,
        runner::{Command, Runner},
    };
    use std::time::{Duration, Instant};
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    let runner = Runner::new(machine, false).unwrap();
    let initial = runner.take_snapshot().unwrap();
    assert!(initial.cpu.instructions > 0);
    for mode in [RendererMode::Threaded, HARDWARE_MODE, RendererMode::Serial] {
        runner.send(Command::SetRenderer(mode)).unwrap();
        let start = Instant::now();
        loop {
            if let Some(snapshot) = runner.take_snapshot() {
                assert_eq!(snapshot.renderer, mode);
                assert_eq!(snapshot.cpu.instructions, initial.cpu.instructions);
                assert!(!snapshot.running);
                assert_eq!(snapshot.rgb, initial.rgb);
                assert_eq!(snapshot.message, None);
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
#[ignore = "requires physical GPUs supporting every compiled platform backend"]
fn switching_hardware_apis_preserves_queued_draws_and_latched_frames() {
    let [mut serial, mut hardware] = pair();
    for gpu in [&mut serial, &mut hardware] {
        send(gpu, &[0x6000_00ff, xy(4, 5), xy(12, 13)]);
        let mut timers = Timers::default();
        while gpu.tick(1000, &mut timers) & 1 == 0 {}
        // These writes are queued after scanout and before the backend switch.
        send(gpu, &[0x6000_ff00, xy(4, 5), xy(12, 13)]);
        send(gpu, &[0x8000_0000, xy(4, 5), xy(6, 6), xy(12, 13)]);
    }
    let completed = serial.completed_frame().unwrap().rgb.clone();
    let modes = [
        #[cfg(all(feature = "dx12", target_os = "windows"))]
        RendererMode::Dx12,
        #[cfg(feature = "vulkan")]
        RendererMode::Vulkan,
        #[cfg(feature = "opengl")]
        RendererMode::OpenGl,
        #[cfg(all(feature = "metal", target_os = "macos"))]
        RendererMode::Metal,
        RendererMode::Hardware,
        HARDWARE_MODE,
    ];
    for mode in modes {
        hardware.set_renderer(mode).unwrap();
        assert_eq!(hardware.renderer_mode(), mode);
        assert_eq!(hardware.completed_frame().unwrap().rgb, completed);
        equal(&serial, &hardware);
    }
}

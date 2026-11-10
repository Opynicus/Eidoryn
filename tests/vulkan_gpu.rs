//! Explicit hardware tests: `cargo test --test vulkan_gpu -- --ignored --test-threads=1`
//! A missing physical Vulkan device is a failure, never a skipped software run.
#![cfg(feature = "vulkan")]
use eidoryn_psx::{
    gpu::{Gpu, RendererMode},
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

fn pair() -> [Gpu; 2] {
    let mut serial = Gpu::default();
    let mut random = 0x3141_5926_u32;
    for value in serial.vram_mut() {
        *value = next(&mut random) as u16;
    }
    send(&mut serial, &[0xe400_0000 | 1023 | (511 << 10)]);
    serial.gp1(0x0300_0000).unwrap();
    let mut hardware = serial.clone();
    hardware.set_renderer(RendererMode::Vulkan).unwrap();
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
            "VRAM mismatch at ({}, {}): software={:04x}, Vulkan={:04x}",
            i % 1024,
            i / 1024,
            a[i],
            b[i]
        );
    }
    assert_eq!(serial.status(), hardware.status());
    assert_eq!(serial.display_rgb(), hardware.display_rgb());
    assert_eq!(hardware.renderer_error(), None);
}

#[test]
#[ignore = "requires a physical Vulkan GPU"]
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
#[ignore = "requires a physical Vulkan GPU"]
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
#[ignore = "requires a physical Vulkan GPU"]
fn hardware_vblank_snapshot_precedes_buffer_clear_and_survives_reset_and_switch() {
    let mut gpu = Gpu::default();
    gpu.set_renderer(RendererMode::Vulkan).unwrap();
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
    assert_eq!(gpu.renderer_mode(), RendererMode::Vulkan);
    assert_eq!(gpu.completed_frame().unwrap().rgb, completed);
    send(&mut gpu, &[0x0200_ff00, 0, xy(16, 1)]);
    let checkpoint = gpu.clone();
    assert_eq!(checkpoint.renderer_mode(), RendererMode::Serial);
    assert_eq!(checkpoint.vram()[0], 31 << 5);
    for mode in [
        RendererMode::Threaded,
        RendererMode::Vulkan,
        RendererMode::Serial,
    ] {
        gpu.set_renderer(mode).unwrap();
        assert_eq!(gpu.renderer_mode(), mode);
        assert_eq!(gpu.vram(), checkpoint.vram());
    }
}

#[test]
#[ignore = "requires a physical Vulkan GPU"]
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
#[ignore = "requires a physical Vulkan GPU"]
fn runner_can_enable_and_disable_vulkan_without_resetting_the_guest() {
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
    for mode in [
        RendererMode::Threaded,
        RendererMode::Vulkan,
        RendererMode::Serial,
    ] {
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

use eidoryn_psx::{
    gpu::{Gpu, RendererMode},
    machine::Machine,
    runner::{Command, Runner},
    timers::Timers,
};
use std::time::{Duration, Instant};

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
    // Seed textures, palettes, mask bits and background before starting the worker.
    for (i, pixel) in serial.vram_mut().iter_mut().enumerate() {
        *pixel = (i as u16).wrapping_mul(7919);
    }
    send(&mut serial, &[0xe400_0000 | 1023 | (511 << 10)]);
    serial.gp1(0x0300_0000).unwrap();
    let mut threaded = serial.clone();
    threaded.set_renderer(RendererMode::Threaded).unwrap();
    [serial, threaded]
}

#[test]
fn ordered_batches_preserve_draw_state_blending_texture_feedback_and_overlap() {
    let [mut serial, mut threaded] = pair();
    for i in 0..300_u32 {
        let x = i * 7 % 200;
        let y = i * 11 % 200;
        let mode = (i % 3) << 7 | ((i % 4) << 5) | 0x200;
        // Each block crosses batched work with changed state, texture feedback,
        // an overlapping copy, Gouraud triangle and streamed polyline.
        let words = [
            0xe100_0000 | mode,
            0xe200_0000 | (i & 15),
            0xe500_0000 | (i & 3),
            0xe600_0000 | (i & 3),
            0x6200_0080 | (i << 8),
            xy(x, y),
            xy(17, 13),
            0x6700_0000,
            xy(x + 1, y + 1),
            xy(x, y),
            xy(8, 8),
            0x8000_0000,
            xy(x, y),
            xy(x + 2, y + 2),
            xy(12, 12),
            0x3000_00ff,
            xy(x, y),
            0x0000_ff00,
            xy(x + 14, y),
            0x00ff_0000,
            xy(x, y + 15),
            0x4800_00ff,
            xy(x, y),
            xy(x + 10, y + 5),
            xy(x + 20, y + 10),
            0x5000_5000,
        ];
        send(&mut serial, &words);
        send(&mut threaded, &words);
        assert_eq!(serial.status(), threaded.status());
    }
    assert_eq!(serial.commands, threaded.commands);
    assert_eq!(serial.revision, threaded.revision);
    assert_eq!(serial.vram(), threaded.vram());
    assert_eq!(serial.display_rgb(), threaded.display_rgb());
    assert_eq!(threaded.renderer_mode(), RendererMode::Threaded);
}

#[test]
fn partial_uploads_downloads_and_rgb24_scanout_are_synchronization_barriers() {
    let [mut serial, mut threaded] = pair();
    for gpu in [&mut serial, &mut threaded] {
        send(gpu, &[0xe600_0000, 0xa000_0000, xy(1023, 511), xy(3, 2)]);
        gpu.gp0(0x5678_1234).unwrap();
    }
    // A readback must flush an incomplete upload, even below the batch size.
    assert_eq!(serial.vram(), threaded.vram());
    for gpu in [&mut serial, &mut threaded] {
        send(gpu, &[0xdef0_9abc, 0x4321_8765]);
        send(gpu, &[0xc000_0000, xy(1023, 511), xy(3, 2)]);
    }
    for expected in [0x5678_1234, 0xdef0_9abc, 0x4321_8765] {
        assert_eq!(serial.read(), expected);
        assert_eq!(threaded.read(), expected);
    }
    for gpu in [&mut serial, &mut threaded] {
        gpu.gp1(0x0800_0013).unwrap(); // 640-wide, 24-bit display.
        gpu.gp1(0x0500_0000 | 1023 | (511 << 10)).unwrap();
    }
    assert_eq!(serial.display_rgb(), threaded.display_rgb());
}

#[test]
fn queued_draws_latch_at_vblank_before_later_clears_and_reset() {
    let mut gpu = Gpu::default();
    gpu.set_renderer(RendererMode::Threaded).unwrap();
    gpu.gp1(0x0300_0000).unwrap();
    send(&mut gpu, &[0x0200_00ff, 0, xy(256, 240)]);
    let mut timers = Timers::default();
    while gpu.tick(1000, &mut timers) & 1 == 0 {}
    // Submit the clear before anyone consumes the asynchronous field.
    send(&mut gpu, &[0x0200_0000, 0, xy(256, 240)]);
    gpu.gp1(0x0800_0025).unwrap(); // Change resolution before consuming the old field.
    gpu.gp1(0x0500_0100).unwrap(); // Swap the displayed buffer.
    gpu.gp1(0x0300_0001).unwrap();
    assert!(gpu.display_rgb().iter().all(|c| *c == 0));
    assert_eq!(gpu.completed_frame().unwrap().dimensions, (256, 240));
    let completed = gpu.completed_frame().unwrap().rgb.clone();
    assert_eq!(&completed[..6], &[255, 0, 0, 255, 0, 0]);
    assert_eq!(gpu.completed_frame().unwrap().rgb, completed);
    gpu.gp1(0).unwrap();
    assert_eq!(gpu.renderer_mode(), RendererMode::Threaded);
    assert_eq!(gpu.completed_frame().unwrap().rgb, completed);
    assert!(gpu.vram().iter().all(|p| *p == 0));
}

#[test]
fn unconsumed_fields_can_be_replaced_without_stalling_the_gpu_worker() {
    let mut gpu = Gpu::default();
    gpu.set_renderer(RendererMode::Threaded).unwrap();
    gpu.gp1(0x0300_0000).unwrap();
    let mut timers = Timers::default();
    for color in [0x0000_00ff, 0x0000_ff00, 0x00ff_0000, 0x00ff_ffff] {
        send(&mut gpu, &[0x0200_0000 | color, 0, xy(256, 240)]);
        gpu.tick(1_200_000, &mut timers);
    }
    assert!(gpu.frames >= 8);
    assert!(gpu.completed_frame().unwrap().rgb.iter().all(|c| *c == 255));
}

#[test]
fn cloning_and_mode_switches_preserve_pending_work_without_sharing_vram() {
    let mut gpu = Gpu::default();
    gpu.set_renderer(RendererMode::Threaded).unwrap();
    send(&mut gpu, &[0x0200_00ff, 0, xy(16, 1)]);
    let checkpoint = gpu.clone();
    assert_eq!(checkpoint.renderer_mode(), RendererMode::Serial);
    for _ in 0..4 {
        gpu.set_renderer(RendererMode::Serial).unwrap();
        assert_eq!(gpu.vram(), checkpoint.vram());
        gpu.set_renderer(RendererMode::Threaded).unwrap();
    }
    send(&mut gpu, &[0x0200_ff00, 0, xy(16, 1)]);
    assert_eq!(gpu.vram()[0], 31 << 5);
    assert_eq!(checkpoint.vram()[0], 31);
    gpu.vram_mut()[0] = 17;
    assert_eq!(gpu.renderer_mode(), RendererMode::Serial);
    assert_eq!(gpu.vram()[0], 17);
}

#[test]
fn worker_reset_and_media_replacement_keep_threaded_renderer() {
    let mut machine = Machine::demo();
    machine
        .bus
        .gpu
        .set_renderer(RendererMode::Threaded)
        .unwrap();
    let runner = Runner::new(machine, false).unwrap();
    // Consume the initial snapshot so each following publication acknowledges
    // its command, including when a reset leaves the same visible state.
    assert_eq!(
        runner.take_snapshot().unwrap().renderer,
        RendererMode::Threaded
    );
    for command in [
        Command::Slice,
        Command::Reset,
        Command::Demo,
        Command::RequestClose,
    ] {
        runner.send(command).unwrap();
        let start = Instant::now();
        loop {
            if let Some(snapshot) = runner.take_snapshot() {
                assert_eq!(snapshot.renderer, RendererMode::Threaded);
                assert!(snapshot.message.is_none(), "{:?}", snapshot.message);
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn runtime_renderer_switch_preserves_game_and_reset_uses_selected_mode() {
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    let runner = Runner::new(machine, false).unwrap();
    let initial = runner.take_snapshot().unwrap();
    assert!(initial.cpu.instructions > 0);
    for mode in [
        RendererMode::Threaded,
        RendererMode::Serial,
        RendererMode::Threaded,
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
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    runner.send(Command::Reset).unwrap();
    let start = Instant::now();
    loop {
        if let Some(snapshot) = runner.take_snapshot() {
            assert_eq!(snapshot.renderer, RendererMode::Threaded);
            assert_eq!(snapshot.message, None);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(not(feature = "vulkan"))]
#[test]
fn unavailable_hardware_backend_preserves_current_renderer_and_vram() {
    let [_, mut gpu] = pair();
    send(&mut gpu, &[0x0200_00ff, 0, xy(16, 1)]);
    let before = gpu.vram();
    assert!(
        gpu.set_renderer(RendererMode::Vulkan)
            .unwrap_err()
            .to_string()
            .contains("--features vulkan")
    );
    assert_eq!(gpu.renderer_mode(), RendererMode::Threaded);
    assert_eq!(gpu.vram(), before);
}

#[cfg(not(feature = "opengl"))]
#[test]
fn unavailable_opengl_preserves_software_modes_and_can_keep_drawing() {
    for mode in [RendererMode::Serial, RendererMode::Threaded] {
        let mut gpu = Gpu::default();
        gpu.set_renderer(mode).unwrap();
        send(&mut gpu, &[0x0200_00ff, 0, xy(16, 1)]);
        let before = gpu.vram();
        assert!(
            gpu.set_renderer(RendererMode::OpenGl)
                .unwrap_err()
                .to_string()
                .contains("--features opengl")
        );
        assert_eq!(gpu.renderer_mode(), mode);
        assert_eq!(gpu.vram(), before);
        assert_eq!(gpu.renderer_error(), None);
        send(&mut gpu, &[0x0200_ff00, 0, xy(16, 1)]);
        assert_eq!(gpu.vram()[0], 31 << 5);
        assert_eq!(gpu.renderer_error(), None);
    }
}

use super::*;
use crate::video::{Pixel, Video};

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn lcd_shader_matches_software_and_device_loss_recovers_the_same_frame() {
    let adapter = adapters().into_iter().next().expect("compute adapter");
    let mut gpu = Compositor::from_adapter(&adapter).unwrap();
    eprintln!("GBA shader comparison: {}", gpu.name);
    let mut rgb = vec![0; WIDTH * HEIGHT * 3];
    let mut software = rgb.clone();
    let mut io = [0; 1024];
    for y in 0..HEIGHT {
        let mode = (y % 4) as u16;
        io[0x50..0x52].copy_from_slice(&(0x1001 | mode << 6).to_le_bytes());
        io[0x52] = (y % 32) as u8;
        io[0x53] = (31 - y % 32) as u8;
        io[0x54] = (y % 32) as u8;
        let effects = Effects::read(&io);
        let masks = std::array::from_fn(|x| if x % 3 == 0 { 0 } else { 0x3f });
        let line = std::array::from_fn(|x| {
            let mut stack = Stack::backdrop(31 << 10);
            stack.insert(Pixel {
                rank: 4,
                color: ((x + y) * 0x487) as u16,
                layer: 16,
                semi: false,
            });
            stack.insert(Pixel {
                rank: 0,
                color: ((x * 117 + y) ^ 0x7476) as u16,
                layer: 1,
                semi: x % 5 == 0,
            });
            stack
        });
        assert!(gpu.capture(y, &effects, &line, &masks));
        for (x, stack) in line.into_iter().enumerate() {
            let color = stack.color(&effects, masks[x]);
            let dst = (y * WIDTH + x) * 3;
            for (shift, byte) in [0, 5, 10].into_iter().zip(&mut software[dst..dst + 3]) {
                let channel = ((color >> shift) & 31u16) as u8;
                *byte = (channel << 3) | (channel >> 2);
            }
        }
    }
    gpu.finish(&mut rgb)
        .expect("dispatch and read back shader result");
    assert_eq!(rgb, software);

    let mut video = Video::new();
    gpu.error.set("simulated device loss".into()).unwrap();
    video.accelerator = Some(gpu);
    let mut reference = Video::new();
    for y in 0..HEIGHT {
        // HBlank changes both palette and blend coefficients. Forced blank dominates both.
        io[0] = if y % 9 == 0 { 0x80 } else { 0 };
        io[0x50] = 0xbf;
        io[0x54] = (y % 17) as u8;
        reference.palette[..2].copy_from_slice(&((y * 31) as u16).to_le_bytes());
        video.palette[..2].copy_from_slice(&reference.palette[..2]);
        reference.render_line(y, &io);
        video.render_line(y, &io);
    }
    assert_eq!(video.pixels, reference.pixels);
    assert!(video.graphics_device().is_none());
    assert_eq!(
        video.graphics_error.as_deref(),
        Some("simulated device loss")
    );
}
#[test]
#[ignore = "requires a local GBA ROM in EIDORYN_GBA_TEST_ROM and a compute adapter"]
fn local_cartridge_lcd_matches_software_across_live_switches() {
    let path = std::env::var_os("EIDORYN_GBA_TEST_ROM").expect("path to local GBA ROM");
    let bytes = std::fs::read(path).unwrap();
    let mut reference = crate::Machine::from_rom(bytes.clone()).unwrap();
    let mut machine = crate::Machine::from_rom(bytes).unwrap();
    let adapter = adapters().into_iter().next().expect("compute adapter");
    let mut displayed = false;
    for frame in 0..420 {
        if matches!(frame, 5 | 300) {
            if physical_adapter(&adapter.get_info()) {
                machine.set_hardware_acceleration(true).unwrap();
            } else {
                machine.memory.video.accelerator =
                    Some(Compositor::from_adapter(&adapter).unwrap());
            }
            eprintln!(
                "Local cartridge GPU: {}",
                machine.graphics_device().unwrap()
            );
        } else if frame == 290 {
            machine.set_hardware_acceleration(false).unwrap();
        }
        let buttons = if frame > 90 && frame % 60 < 6 { 8 } else { 0 };
        for console in [&mut reference, &mut machine] {
            console.set_buttons(buttons);
            console.run_frame().unwrap();
        }
        assert_eq!(machine.pixels(), reference.pixels(), "frame {frame}");
        assert!(machine.take_graphics_error().is_none());
        let pixels = machine.pixels();
        displayed |= pixels[..pixels.len() - 3] != pixels[3..];
    }
    assert!(displayed, "expected a visible game scene during the test");
}

#[test]
#[ignore = "requires a compute adapter (software adapters are sufficient)"]
fn mid_field_snapshot_preserves_deferred_rows_and_the_active_renderer() {
    let machine = || crate::Machine::from_rom(vec![0; crate::HEADER_LEN]).unwrap();
    let mut reference = machine();
    let mut accelerated = machine();
    let adapter = adapters().into_iter().next().expect("compute adapter");
    let compositor = Compositor::from_adapter(&adapter).unwrap();
    let device = compositor.name.clone();
    accelerated.memory.video.accelerator = Some(compositor);

    for console in [&mut reference, &mut accelerated] {
        console.memory.halted = true;
        console.step().unwrap(); // first HBlank; scanline zero has been drawn
    }
    assert_ne!(&accelerated.pixels()[..3], &reference.pixels()[..3]);
    let state = accelerated.save_state().unwrap();
    assert_eq!(state, reference.save_state().unwrap());
    accelerated.run_frame().unwrap();
    accelerated.load_state(&state).unwrap();
    assert_eq!(accelerated.graphics_device(), Some(device.as_str()));
    assert_eq!(accelerated.pixels(), reference.pixels());

    for _ in 0..2 {
        accelerated.run_frame().unwrap();
        reference.run_frame().unwrap();
        assert_eq!(
            accelerated.save_state().unwrap(),
            reference.save_state().unwrap()
        );
        assert!(accelerated.take_graphics_error().is_none());
    }
}

use eidoryn_psx::{gpu::Gpu, machine::Machine, runner::Snapshot};
use std::sync::Arc;

fn next_field(machine: &mut Machine) {
    let previous = machine
        .bus
        .gpu
        .completed_frame()
        .map(|frame| Arc::clone(&frame.rgb));
    for _ in 0..600 {
        machine.bus.tick(1000).unwrap();
        if let Some(frame) = machine.bus.gpu.completed_frame()
            && previous
                .as_ref()
                .is_none_or(|rgb| !Arc::ptr_eq(rgb, &frame.rgb))
        {
            return;
        }
    }
    panic!("No completed field within 600,000 CPU clocks");
}

fn visible_machine() -> Machine {
    let mut machine = Machine::demo();
    machine.bus.gpu.gp1(0x0300_0000).unwrap();
    machine.bus.gpu.vram_mut().fill(31); // red
    next_field(&mut machine);
    machine
}

#[test]
fn vblank_latches_pixels_before_guest_clears_or_swaps_buffers() {
    let mut machine = visible_machine();
    assert_ne!(machine.bus.read32(0x1f80_1070).unwrap() & 1, 0);
    let finished = Snapshot::capture(&machine, true, None, 0.0);
    assert_eq!(&finished.rgb[..3], &[255, 0, 0]);

    // Reproduce a host slice ending after the guest clears its framebuffer.
    for word in [0x0200_0000, 0, 256 | (240 << 16)] {
        machine.bus.gpu.gp0(word).unwrap();
    }
    assert!(machine.bus.gpu.display_rgb().iter().all(|c| *c == 0));
    let during_clear = Snapshot::capture(&machine, true, None, 0.0);
    assert!(Arc::ptr_eq(&finished.rgb, &during_clear.rgb));

    // Switch buffers and video mode in the same VBlank. RGB and dimensions
    // must change together only when the next visible field has completed.
    machine.bus.gpu.gp1(0x0500_0100).unwrap();
    machine.bus.gpu.gp1(0x0800_0001).unwrap();
    machine.bus.gpu.gp1(0x0300_0001).unwrap();
    let paused = Snapshot::capture(&machine, false, None, 0.0);
    assert!(paused.has_same_frame(&finished));
    assert_eq!(paused.dimensions, (256, 240));
    machine.bus.gpu.vram_mut().fill(31 << 5); // finished green drawing
    machine.bus.gpu.gp1(0x0300_0000).unwrap();
    next_field(&mut machine);
    let next = Snapshot::capture(&machine, true, None, 0.0);
    assert_eq!(next.dimensions, (320, 240));
    assert_eq!(next.rgb.len(), 320 * 240 * 3);
    assert_eq!(&next.rgb[..3], &[0, 255, 0]);
    assert_eq!(&finished.rgb[..3], &[255, 0, 0]);
}

#[test]
fn completed_black_fields_are_presented_including_display_disable() {
    let mut machine = visible_machine();
    machine.bus.gpu.vram_mut().fill(0);
    next_field(&mut machine);
    assert!(
        Snapshot::capture(&machine, true, None, 0.0)
            .rgb
            .iter()
            .all(|c| *c == 0)
    );
    machine.bus.gpu.vram_mut().fill(0x7fff);
    next_field(&mut machine);
    assert!(
        Snapshot::capture(&machine, true, None, 0.0)
            .rgb
            .iter()
            .all(|c| *c == 255)
    );
    machine.bus.gpu.gp1(0x0300_0001).unwrap();
    next_field(&mut machine);
    assert!(
        Snapshot::capture(&machine, true, None, 0.0)
            .rgb
            .iter()
            .all(|c| *c == 0)
    );
}

#[test]
fn gpu_reset_preserves_scanned_pixels_until_next_field() {
    let mut machine = visible_machine();
    let before = Snapshot::capture(&machine, true, None, 0.0);
    machine.bus.gpu.gp1(0).unwrap();
    assert!(Snapshot::capture(&machine, true, None, 0.0).has_same_frame(&before));
    next_field(&mut machine);
    assert!(
        Snapshot::capture(&machine, true, None, 0.0)
            .rgb
            .iter()
            .all(|c| *c == 0)
    );
}

#[test]
fn demo_yield_publishes_its_explicit_guest_frame() {
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    assert_eq!(machine.bus.gpu.frames, 0);
    let first = Snapshot::capture(&machine, true, None, 0.0);
    assert_eq!(&*first.rgb, machine.bus.gpu.display_rgb());
    machine.run_slice().unwrap();
    let second = Snapshot::capture(&machine, true, None, 0.0);
    assert!(!first.has_same_frame(&second));
}

#[test]
fn display_conversion_preserves_pixels_for_all_modes_and_wraps() {
    let mut gpu = Gpu::default();
    for (index, pixel) in gpu.vram_mut().iter_mut().enumerate() {
        *pixel = index.wrapping_mul(7919) as u16;
    }
    gpu.gp1(0x0300_0000).unwrap();
    // Both axes cross the VRAM edge, including an odd horizontal halfword origin.
    let (origin_x, origin_y) = (1009, 509);
    gpu.gp1(0x0500_0000 | origin_x | (origin_y << 10)).unwrap();
    let vram = gpu.vram();
    let raw: Vec<u8> = vram.iter().flat_map(|pixel| pixel.to_le_bytes()).collect();
    for width in [0, 1, 2, 3, 0x40] {
        for height in [0, 0x24] {
            for depth in [0, 0x10] {
                gpu.gp1(0x0800_0000 | width | height | depth).unwrap();
                let (w, h) = gpu.dimensions();
                let output = gpu.display_rgb();
                assert_eq!(output.len(), w * h * 3);
                for y in 0..h {
                    for x in 0..w {
                        let row = (origin_y as usize + y) % 512;
                        let expected = if depth != 0 {
                            std::array::from_fn(|channel| {
                                raw[row * 2048 + (origin_x as usize * 2 + x * 3 + channel) % 2048]
                            })
                        } else {
                            let color = vram[row * 1024 + (origin_x as usize + x) % 1024];
                            std::array::from_fn(|channel| {
                                let value = (color >> (channel * 5)) & 31;
                                ((value * 8) + (value / 4)) as u8
                            })
                        };
                        assert_eq!(&output[(y * w + x) * 3..][..3], &expected as &[u8; 3]);
                    }
                }
            }
        }
    }
    gpu.gp1(0x0300_0001).unwrap();
    assert!(gpu.display_rgb().iter().all(|byte| *byte == 0));
}

#[test]
fn frame_reuse_ignores_cpu_state_but_detects_pixel_and_dimension_changes() {
    let machine = Machine::demo();
    let first = Snapshot::capture(&machine, true, None, 60.0);
    let mut next = Snapshot::capture(&machine, false, None, 0.0);
    next.cpu.pc = next.cpu.pc.wrapping_add(4);
    assert!(next.has_same_frame(&first));
    std::sync::Arc::make_mut(&mut next.rgb)[0] ^= 1;
    assert!(!next.has_same_frame(&first));
    std::sync::Arc::make_mut(&mut next.rgb)[0] ^= 1;
    next.dimensions.0 += 1;
    assert!(!next.has_same_frame(&first));
}

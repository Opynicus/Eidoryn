use super::*;

fn write<const N: usize>(display: &mut Display, addr: u32, bytes: [u8; N]) {
    for (i, byte) in bytes.into_iter().enumerate() {
        display.write8(addr + i as u32, byte);
    }
}

#[test]
fn lcd_order_brightness_and_power_apply_to_the_correct_engine() {
    let mut d = Display::default();
    for engine in [0, 0x1000] {
        write(&mut d, 0x0400_0000 + engine, (1u32 << 16).to_le_bytes());
    }
    write(&mut d, 0x0500_0000, 0x001fu16.to_le_bytes());
    write(&mut d, 0x0500_0400, 0x7c00u16.to_le_bytes());
    d.render();
    assert_eq!(&d.pixels()[..3], &[255, 0, 0]);
    assert_eq!(&d.pixels()[FRAME..FRAME + 3], &[0, 0, 255]);
    write(&mut d, 0x0400_0304, 0x0203u16.to_le_bytes());
    write(&mut d, 0x0400_006c, 0x8010u16.to_le_bytes());
    d.render();
    assert_eq!(&d.pixels()[..3], &[0, 0, 255]);
    assert_eq!(&d.pixels()[FRAME..FRAME + 3], &[0, 0, 0]);
    d.write8(0x0400_0304, 0);
    d.render();
    assert!(d.pixels().iter().all(|&v| v == 255));
}

#[test]
fn vram_banks_remap_without_losing_data_and_overlap_on_the_bus() {
    let mut d = Display::default();
    for (bank, value) in [(0, 0x53), (1, 0xa2)] {
        d.write8(0x0400_0240 + bank, 0x80);
        d.write8(0x0680_0100 + bank * 0x20000, value);
        d.write8(0x0400_0240 + bank, 0x81);
    }
    assert_eq!(d.read8(0x0600_0100), 0xf3);
    d.write8(0x0600_0100, 0x89);
    for bank in [0, 1] {
        d.write8(0x0400_0240 + bank, 0x80);
        assert_eq!(d.read8(0x0680_0100 + bank * 0x20000), 0x89);
    }
    d.write8(0x0400_0242, 0x8a); // C at ARM7 offset 0x20000
    assert_eq!(d.arm7_read8(0x0400_0240), 1);
    d.arm7_write8(0x0602_0000, 0xab);
    assert_eq!(d.read8(0x0620_0000), 0);
    d.write8(0x0400_0242, 0x84); // Move C to the sub BG engine
    assert_eq!(d.read8(0x0620_0000), 0xab);
    assert_eq!(d.arm7_read8(0x0602_0000), 0);
}

#[test]
fn direct_color_affine_background_and_sub_text_share_physical_frame() {
    let mut d = Display::default();
    d.write8(0x0400_0240, 0x81);
    write(&mut d, 0x0400_0000, 0x10405u32.to_le_bytes()); // mode 5, BG2
    write(&mut d, 0x0400_000c, 0x4084u16.to_le_bytes()); // direct 256x256 bitmap
    for addr in [0x0400_0020, 0x0400_0026] {
        write(&mut d, addr, 256u16.to_le_bytes());
    }
    write(&mut d, 0x0600_0000, 0x801fu16.to_le_bytes());
    d.write8(0x0400_0242, 0x84);
    write(&mut d, 0x0400_1000, 0x10100u32.to_le_bytes()); // sub text, BG0
    write(&mut d, 0x0400_1008, 0x0100u16.to_le_bytes()); // map at 0x800
    write(&mut d, 0x0620_0800, 1u16.to_le_bytes()); // first map entry uses tile 1
    d.write8(0x0620_0020, 0x02); // palette 2 on first pixel
    write(&mut d, 0x0500_0404, 0x03e0u16.to_le_bytes());
    d.render();
    assert_eq!(&d.pixels()[..6], &[255, 0, 0, 0, 0, 0]);
    assert_eq!(&d.pixels()[FRAME..FRAME + 6], &[0, 255, 0, 0, 0, 0]);
}

#[test]
fn flipped_sprite_uses_its_object_bank_and_transparent_palette_index() {
    let mut d = Display::default();
    d.write8(0x0400_0240, 0x82); // Bank A is mapped to main engine OBJ.
    write(&mut d, 0x0400_0000, 0x11010u32.to_le_bytes());
    write(&mut d, 0x0500_0000, 0x7c00u16.to_le_bytes()); // Blue backdrop
    write(&mut d, 0x0500_0202, 0x001fu16.to_le_bytes()); // Red sprite pixel
    write(&mut d, 0x0700_0002, 0x1000u16.to_le_bytes()); // Horizontal flip
    write(&mut d, 0x0700_0004, 1u16.to_le_bytes()); // Tile 1
    d.write8(0x0640_0020, 0x01);
    d.render();
    assert_eq!(&d.pixels()[..3], &[0, 0, 255]);
    assert_eq!(&d.pixels()[7 * 3..8 * 3], &[255, 0, 0]);
    assert_eq!(&d.pixels()[8 * 3..9 * 3], &[0, 0, 255]);
}

#[test]
fn packed_geometry_fifo_draws_whole_quad_only_after_swap() {
    let mut d = Display::default();
    write(&mut d, 0x0400_0000, 0x10108u32.to_le_bytes()); // Main 3D on BG0
    let fifo = |d: &mut Display, value: u32| write(d, 0x0400_0400, value.to_le_bytes());
    // POLYGON_ATTR, BEGIN(quads), COLOR, VTX_XY. X and Y are signed 4.12.
    fifo(&mut d, 0x2520_4029);
    for value in [0x001f_00c0, 1, 0x03e0, 0xf800_f800] {
        fifo(&mut d, value);
    }
    fifo(&mut d, 0x4125_2525); // VTX_XY three times, END (zero parameters)
    for vertex in [0x0800_f800, 0x0800_0800, 0xf800_0800] {
        fifo(&mut d, vertex);
    }
    d.render();
    let center = (96 * W + 128) * 3;
    assert_eq!(&d.pixels()[center..center + 3], &[0, 0, 0]);
    fifo(&mut d, 0x50);
    fifo(&mut d, 0);
    d.render();
    assert_eq!(&d.pixels()[center..center + 3], &[0, 255, 0]);
    assert_eq!(
        d.pixels()[..FRAME]
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|&&v| v == [0, 255, 0])
            .count(),
        128 * 96
    );
    assert_eq!(d.read8(0x0400_0603) & 6, 6); // Empty GX FIFO has both status bits set.
}

#[test]
fn geometry_clip_and_depth_reject_covered_pixels_without_writing_outside_viewport() {
    let mut d = Display::default();
    write(&mut d, 0x0400_0000, 0x10108u32.to_le_bytes());
    let cmd =
        |d: &mut Display, op: u32, value: u32| write(d, 0x0400_0400 + 4 * op, value.to_le_bytes());
    cmd(&mut d, 0x60, 0x9fdf_2020); // viewport x=32..223, y=32..159
    // Submit the nearer red triangle, then a blue triangle behind it.
    for (depth, color) in [(0u32, 0x001f), (0x0400, 0x7c00)] {
        cmd(&mut d, 0x29, 0x001f_00c0);
        cmd(&mut d, 0x40, 0);
        cmd(&mut d, 0x20, color);
        for pos in [0xf000_e000, 0xf000_2000, 0x2000_0000] {
            cmd(&mut d, 0x23, pos);
            cmd(&mut d, 0x23, depth);
        }
    }
    cmd(&mut d, 0x50, 0);
    d.render();
    let pixel = |x, y| {
        let i: usize = (y * W + x) * 3;
        &d.pixels()[i..i + 3]
    };
    assert_eq!(pixel(128usize, 96usize), &[255, 0, 0]);
    assert_eq!(pixel(0, 96), &[0, 0, 0]);
    assert_eq!(pixel(128, 0), &[0, 0, 0]);
}

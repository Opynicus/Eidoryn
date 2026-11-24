use super::{Video, WIDTH};

fn write(bytes: &mut [u8], addr: usize, value: u16) {
    bytes[addr..addr + 2].copy_from_slice(&value.to_le_bytes());
}

fn pixel(video: &Video, x: usize, y: usize) -> [u8; 3] {
    let offset = (y * WIDTH + x) * 3;
    video.pixels[offset..offset + 3].try_into().unwrap()
}

#[test]
fn backdrop_fades_use_five_bit_channels_and_saturate_coefficients() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut v.palette, 0, 31 | (16 << 5));
    write(&mut io, 0x50, 0x00bf);
    io[0x54] = 8;
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 0, 0), [255, 189, 123]);
    io[0x54] = 31;
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 0, 0), [255; 3]);
    write(&mut io, 0x50, 0x00ff);
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 0, 0), [0; 3]);
}

fn solid_text_bg(v: &mut Video, io: &mut [u8], bg: usize, color: u16) {
    let tile = bg + 1;
    let screen = 24 + bg;
    write(io, 8 + bg * 2, (screen as u16) << 8);
    write(&mut v.vram, screen * 2048, tile as u16);
    v.vram[tile * 32..tile * 32 + 32].fill((tile * 0x11) as u8);
    write(&mut v.palette, tile * 2, color);
}

#[test]
fn alpha_uses_the_visible_neighbor_and_window_zero_takes_priority() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut io, 0, 0x6700); // WIN0, WIN1, BG0..2
    for (bg, color) in [31, 31 << 5, 31 << 10].into_iter().enumerate() {
        solid_text_bg(&mut v, &mut io, bg, color);
    }
    write(&mut io, 0x50, 0x0441); // BG0 over BG2
    write(&mut io, 0x52, 0x0808);
    write(&mut io, 0x40, 0x0204); // WIN0 covers x 2,3
    write(&mut io, 0x42, 0x0006); // WIN1 covers x 0 through 5
    write(&mut io, 0x44, 0x0001);
    write(&mut io, 0x46, 0x0001);
    write(&mut io, 0x48, 0x2527); // Only WIN1 hides BG1
    write(&mut io, 0x4a, 0x0005); // Outside both, effects disabled
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 0, 0), [123, 0, 123]);
    assert_eq!(pixel(&v, 2, 0), [255, 0, 0]);
    assert_eq!(pixel(&v, 6, 0), [255, 0, 0]);
    // Transparent tile data uncovers BG2 even in WIN0.
    v.vram[64..96].fill(0);
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 2, 0), [123, 0, 123]);
}

#[test]
fn object_window_is_a_stencil_and_semitransparent_objects_force_alpha() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut io, 0, 0x9100); // OBJ window, OBJ, BG0
    solid_text_bg(&mut v, &mut io, 0, 31);
    for obj in v.oam.as_chunks_mut::<8>().0 {
        write(obj, 0, 0x0200);
    }
    write(&mut v.oam, 0, 0x0800); // OBJ window at x=0
    write(&mut v.oam, 8, 0x0400); // semi-transparent OBJ at x=0
    write(&mut v.oam, 12, 1);
    v.vram[0x10000] = 0x01; // window only on first dot
    v.vram[0x10020] = 0x11;
    write(&mut v.palette, 0x202, 31 << 10);
    write(&mut io, 0x4a, 0x1110); // BG0 visible only inside object window
    write(&mut io, 0x50, 0x0100); // BG0 selected as second target, normal effect off
    write(&mut io, 0x52, 0x0808);
    v.render_line(0, &io);
    assert_eq!(pixel(&v, 0, 0), [123, 0, 123]);
    assert_eq!(pixel(&v, 1, 0), [0, 0, 255]);
}

#[test]
fn wrapped_window_preserves_the_outside_region_between_its_ends() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut io, 0, 0x2100);
    solid_text_bg(&mut v, &mut io, 0, 31);
    write(&mut io, 0x40, 0xfa04);
    write(&mut io, 0x44, 0xff01);
    io[0x48] = 1;
    v.render_line(0, &io);
    v.render_line(1, &io);
    assert_eq!(pixel(&v, 0, 0), [255, 0, 0]);
    assert_eq!(pixel(&v, 4, 0), [0; 3]);
    assert_eq!(pixel(&v, 0, 1), [0; 3]);
}

#[test]
fn mosaic_samples_background_and_object_blocks_before_window_clipping() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut io, 0, 0x3100); // WIN0, OBJ and BG0
    solid_text_bg(&mut v, &mut io, 0, 31);
    io[8] |= 0x40;
    io[0x4c] = 0x12; // BG blocks are 3 by 2.
    io[0x4d] = 0x23; // OBJ blocks are 4 by 3.
    write(&mut io, 0x40, 0x0208);
    write(&mut io, 0x44, 0x0008);
    io[0x48] = 0x11;
    v.vram[32..64].fill(0);
    v.vram[32] = 1;
    for obj in v.oam.as_chunks_mut::<8>().0 {
        write(obj, 0, 0x0200);
    }
    write(&mut v.oam, 0, 0x1000);
    write(&mut v.oam, 2, 1);
    write(&mut v.palette, 0x202, 31 << 10);
    v.vram[0x10001] = 0x10; // OBJ texel 3, at screen x=4.
    v.render_line(1, &io);
    assert_eq!(pixel(&v, 0, 1), [0; 3]);
    assert_eq!(pixel(&v, 2, 1), [255, 0, 0]);
    assert_eq!(pixel(&v, 3, 1), [0; 3]);
    assert_eq!(pixel(&v, 4, 1), [0, 0, 255]);
    assert_eq!(pixel(&v, 7, 1), [0, 0, 255]);
    v.render_line(2, &io);
    assert_eq!(pixel(&v, 2, 2), [0; 3]);
    assert_eq!(pixel(&v, 4, 2), [0, 0, 255]);
}

#[test]
fn affine_bitmap_mosaic_samples_the_top_left_of_the_screen_space_block() {
    let mut v = Video::new();
    let mut io = [0; 1024];
    write(&mut io, 0, 0x0403);
    write(&mut io, 0x0c, 0x40);
    io[0x4c] = 0x11;
    write(&mut io, 0x20, 0x100);
    write(&mut io, 0x22, 0x100);
    write(&mut io, 0x26, 0x100);
    write(&mut v.vram, 0, 31);
    write(&mut v.vram, 4, 31 << 5);
    v.advance_reference(&io);
    v.render_line(1, &io);
    assert_eq!(pixel(&v, 1, 1), [255, 0, 0]);
    assert_eq!(pixel(&v, 3, 1), [0, 255, 0]);
    v.advance_reference(&io);
    v.render_line(2, &io);
    assert_eq!(pixel(&v, 1, 2), [0; 3]);
}

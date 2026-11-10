use eidoryn_psx::gpu::Gpu;

fn draw(gpu: &mut Gpu, words: &[u32]) {
    for word in words {
        gpu.gp0(*word).unwrap();
    }
}
fn xy(x: u32, y: u32) -> u32 {
    (y << 16) | x
}
fn gpu() -> Gpu {
    let mut gpu = Gpu::default();
    draw(&mut gpu, &[0xe300_0000, 0xe400_0000 | 1023 | (511 << 10)]);
    gpu
}

#[test]
fn modulated_quad_decodes_clut_tpage_and_preserves_transparent_texels() {
    let mut gpu = gpu();
    gpu.vram_mut()[64] = 0x21;
    gpu.vram_mut()[1024 + 64] = 3;
    gpu.vram_mut()[10 * 1024 + 1] = 31;
    gpu.vram_mut()[10 * 1024 + 2] = 31 << 5;
    gpu.vram_mut()[10 * 1024 + 3] = 0x8000;
    gpu.vram_mut()[41 * 1024 + 11] = 0x1234;
    let packet = [
        0x2c80_8080,
        xy(10, 40),
        640 << 16,
        xy(12, 40),
        (1 << 16) | 2,
        xy(10, 42),
        2 << 8,
        xy(12, 42),
        2 | (2 << 8),
    ];
    let before = gpu.commands;
    draw(&mut gpu, &packet[..8]);
    assert_eq!(gpu.commands, before);
    draw(&mut gpu, &packet[8..]);
    assert_eq!(gpu.commands, before + 1);
    assert_eq!(gpu.status() & 0x1ff, 1);
    assert_eq!(gpu.vram()[40 * 1024 + 10], 31);
    assert_eq!(gpu.vram()[40 * 1024 + 11], 31 << 5);
    assert_eq!(gpu.vram()[41 * 1024 + 10], 0x8000); // Masked black is opaque.
    assert_eq!(gpu.vram()[41 * 1024 + 11], 0x1234); // Zero texel leaves background intact.
}

#[test]
fn eight_bit_sprite_applies_texture_window_and_raw_color() {
    let mut gpu = gpu();
    draw(&mut gpu, &[0xe100_0082, 0xe200_0401]); // Page X=128, 8-bit, redirect U bit3 to 1.
    gpu.vram_mut()[132] = 0x0201;
    gpu.vram_mut()[20 * 1024 + 1] = 31;
    gpu.vram_mut()[20 * 1024 + 2] = 31 << 10;
    draw(
        &mut gpu,
        &[0x6500_0000, xy(30, 40), (20 * 64) << 16, xy(2, 1)],
    );
    assert_eq!(gpu.vram()[40 * 1024 + 30], 31);
    assert_eq!(gpu.vram()[40 * 1024 + 31], 31 << 10);
}

#[test]
fn texture_mask_controls_semitransparency_and_all_four_blend_modes() {
    let mut gpu = gpu();
    gpu.vram_mut()[192] = 0x8010;
    gpu.vram_mut()[193] = 0x0010;
    gpu.vram_mut()[50 * 1024 + 30..50 * 1024 + 32].fill(20);
    draw(
        &mut gpu,
        &[0xe100_0103, 0x6700_0000, xy(30, 50), 0, xy(2, 1)],
    );
    assert_eq!(gpu.vram()[50 * 1024 + 30], 0x8012);
    assert_eq!(gpu.vram()[50 * 1024 + 31], 16);
    for (mode, expected) in [18, 31, 4, 24].into_iter().enumerate() {
        gpu.vram_mut()[60 * 1024 + 30] = 20;
        draw(
            &mut gpu,
            &[
                0xe100_0000 | ((mode as u32) << 5),
                0x6200_0080,
                xy(30, 60),
                xy(1, 1),
            ],
        );
        assert_eq!(gpu.vram()[60 * 1024 + 30], expected);
    }
}

#[test]
fn textured_gouraud_triangle_packet_and_sprite_flips_use_correct_attributes() {
    let mut gpu = gpu();
    gpu.vram_mut()[192] = 31;
    draw(
        &mut gpu,
        &[
            0x3480_8080,
            xy(10, 50),
            0,
            0x0080_8080,
            xy(14, 50),
            0x0103_0000,
            0x0080_8080,
            xy(10, 54),
            0,
        ],
    );
    assert_eq!(gpu.vram()[50 * 1024 + 10], 31);
    gpu.vram_mut()[192] = 31;
    gpu.vram_mut()[193] = 31 << 5;
    gpu.vram_mut()[1024 + 192] = 31 << 10;
    gpu.vram_mut()[1024 + 193] = 0x7fff;
    draw(
        &mut gpu,
        &[0xe100_3103, 0x6500_0000, xy(30, 70), 0x0101, xy(2, 2)],
    );
    assert_eq!(gpu.vram()[70 * 1024 + 30], 0x7fff);
    assert_eq!(gpu.vram()[70 * 1024 + 31], 31 << 10);
    assert_eq!(gpu.vram()[71 * 1024 + 30], 31 << 5);
    assert_eq!(gpu.vram()[71 * 1024 + 31], 31);
}

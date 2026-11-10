use eidoryn_psx::{bus::Bus, gpu::Gpu, mdec::Mdec};

fn scale(mdec: &mut Mdec, diagonal: bool) {
    mdec.write_data(0x6000_0000).unwrap();
    let values: [i16; 64] = std::array::from_fn(|n| {
        if if diagonal { n / 8 == n % 8 } else { n < 8 } {
            0x4000
        } else {
            0
        }
    });
    for pair in values.as_chunks::<2>().0 {
        mdec.write_data(u32::from(pair[0] as u16) | (u32::from(pair[1] as u16) << 16))
            .unwrap();
    }
}
fn block(dc: i16) -> u32 {
    0xfe00_0000 | (dc as u32 & 0x3ff)
}
fn drain(mdec: &mut Mdec) -> Vec<u8> {
    let mut bytes = Vec::new();
    while mdec.status() & (1 << 31) == 0 {
        assert!(bytes.len() < 16_384, "unexpected unbounded output");
        bytes.extend(mdec.read_data().to_le_bytes());
    }
    bytes
}

#[test]
fn grayscale_depth_signed_output_and_reset_preserves_programmed_scale() {
    let mut mdec = Mdec::default();
    assert_eq!(mdec.status(), 0x8004_0000);
    scale(&mut mdec, false); // A legal custom scale maps a DC coefficient to value / 16.
    for (command, expected, count) in [
        (0x2800_0001, 0x90, 64),
        (0x2c00_0001, 0x10, 64),
        (0x2000_0001, 0x99, 32),
    ] {
        mdec.write_control(0xe000_0000);
        mdec.write_data(command).unwrap();
        assert!(mdec.dma_input_ready());
        mdec.write_data(block(128)).unwrap(); // DC = 128 * 2; output Y = 16.
        assert!(mdec.dma_output_ready());
        assert_eq!(drain(&mut mdec), vec![expected; count]);
        assert!(!mdec.dma_output_ready());
        assert_eq!(mdec.status() & 0xffff, 0xffff);
    }
}

#[test]
fn quantized_ac_run_uses_zigzag_but_zero_scale_uses_linear_positions() {
    let mut mdec = Mdec::default();
    scale(&mut mdec, true);
    mdec.write_data(0x4000_0000).unwrap();
    for _ in 0..16 {
        mdec.write_data(0x1010_1010).unwrap();
    }
    for (header, lit_pixel) in [(0x0410, 8), (0x0080, 2)] {
        mdec.write_data(0x2800_0002).unwrap();
        mdec.write_data(header | (0x0480 << 16)).unwrap();
        mdec.write_data(0xfe00_fe00).unwrap();
        let image = drain(&mut mdec);
        assert_eq!(image.len(), 64);
        for (n, value) in image.into_iter().enumerate() {
            assert_eq!(
                value,
                if n == 0 || n == lit_pixel { 144 } else { 128 },
                "pixel {n}"
            );
        }
    }
}

#[test]
fn color_blocks_pack_rgb24_and_rgb555_with_stp() {
    let mut mdec = Mdec::default();
    scale(&mut mdec, false);
    mdec.write_data(0x3000_0006).unwrap();
    for dc in [128, -128, 0, 0, 0, 0] {
        mdec.write_data(block(dc)).unwrap();
    }
    let rgb = drain(&mut mdec);
    assert_eq!(rgb.len(), 768);
    for pixel in rgb.as_chunks::<3>().0 {
        assert_eq!(*pixel, [150, 122, 100]);
    }
    mdec.write_data(0x3a00_0006).unwrap();
    for dc in [0, 0, 128, 0, -128, 256] {
        mdec.write_data(block(dc)).unwrap();
    }
    let pixels = drain(&mut mdec);
    assert_eq!(pixels.len(), 512);
    for (quadrant, expected) in [0xca52_u16, 0xc210, 0xb9ce, 0xd294].into_iter().enumerate() {
        for pixel in pixels[quadrant * 128..(quadrant + 1) * 128]
            .as_chunks::<2>()
            .0
        {
            assert_eq!(*pixel, expected.to_le_bytes());
        }
    }
}

#[test]
fn dma0_pauses_at_full_fifo_and_dma1_reorders_four_quadrants_then_signals_irq() {
    let mut bus = Bus::default();
    scale(&mut bus.mdec, false);
    bus.write32(0x1f80_1824, 0x6000_0000).unwrap();
    let data: Vec<u32> = (0..12)
        .flat_map(|_| [0, 0, 128, 0, -128, 256].map(block))
        .collect();
    for (n, value) in data.iter().enumerate() {
        bus.write32(0x2000 + n as u32 * 4, *value).unwrap();
    }
    bus.write32(0x1f80_1820, 0x3800_0000 | data.len() as u32)
        .unwrap();
    bus.write32(0x1f80_10f0, 0x88).unwrap();
    bus.write32(0x1f80_10f4, 0x0083_0000).unwrap();
    bus.write32(0x1f80_1074, 8).unwrap();
    bus.write32(0x1f80_1080, 0x2000).unwrap();
    bus.write32(0x1f80_1084, (12 << 16) | 6).unwrap();
    bus.write32(0x1f80_1088, 0x0100_0201).unwrap();
    assert_ne!(bus.read32(0x1f80_1088).unwrap() & (1 << 24), 0);
    assert_ne!(bus.mdec.status() & (1 << 30), 0);
    assert!(!bus.irq_pending());
    bus.write32(0x1f80_1090, 0x4000).unwrap();
    bus.write32(0x1f80_1094, (48 << 16) | 32).unwrap();
    bus.write32(0x1f80_1098, 0x0100_0200).unwrap();
    for _ in 0..100 {
        bus.tick(1).unwrap();
    }
    assert_eq!(bus.read32(0x1f80_1088).unwrap() & (1 << 24), 0);
    assert_eq!(bus.read32(0x1f80_1098).unwrap() & (1 << 24), 0);
    assert_eq!(bus.read32(0x1f80_10f4).unwrap() & 0x8300_0000, 0x8300_0000);
    assert!(bus.irq_pending());
    assert_eq!(bus.mdec.blocks_decoded, 72);
    for frame in 0..12 {
        for y in 0..16 {
            for x in 0..16 {
                let address = 0x4000 + frame * 512 + (y * 16 + x) * 2;
                assert_eq!(
                    bus.read16(address).unwrap(),
                    [0x4a52, 0x4210, 0x39ce, 0x5294][((y / 8) * 2 + x / 8) as usize]
                );
            }
        }
    }
}

#[test]
fn gpu_rgb24_scanout_uses_byte_packing_and_halfword_display_origin() {
    let mut gpu = Gpu::default();
    gpu.vram_mut()[511 * 1024 + 1..511 * 1024 + 4].copy_from_slice(&[0x3412, 0x7856, 0xbc9a]);
    gpu.vram_mut()[1..4].copy_from_slice(&[0x2211, 0x4433, 0x6655]);
    gpu.gp1(0x0500_0001 | (511 << 10)).unwrap();
    gpu.gp1(0x0800_0010).unwrap();
    gpu.gp1(0x0300_0000).unwrap();
    let bytes = gpu.display_rgb();
    assert_eq!(&bytes[..6], &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]);
    assert_eq!(
        &bytes[256 * 3..256 * 3 + 6],
        &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]
    );
    assert_ne!(gpu.status() & (1 << 21), 0);
}

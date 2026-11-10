use eidoryn_psx::gpu::Gpu;

fn xy(x: i16, y: i16) -> u32 {
    u32::from(x as u16) | (u32::from(y as u16) << 16)
}

fn gpu() -> Gpu {
    let mut gpu = Gpu::default();
    gpu.gp0(0xe400_0000 | 1023 | (511 << 10)).unwrap();
    gpu
}

fn send(gpu: &mut Gpu, words: &[u32]) {
    for word in words {
        gpu.gp0(*word).unwrap();
    }
}

#[test]
fn lines_include_both_endpoints_and_degenerate_line_uses_first_color() {
    let mut gpu = gpu();
    // Unused command bits do not change the flat-line packet length.
    send(&mut gpu, &[0x4500_00ff, xy(2, 3), xy(6, 3)]);
    assert_eq!(
        &gpu.vram()[3 * 1024 + 1..3 * 1024 + 8],
        &[0, 31, 31, 31, 31, 31, 0]
    );
    send(
        &mut gpu,
        &[0x5000_ff00, xy(10, 12), 0x00ff_0000, xy(10, 12)],
    );
    assert_eq!(gpu.vram()[12 * 1024 + 10], 0x3e0);
    assert_eq!(gpu.vram().iter().filter(|p| **p != 0).count(), 6);
}

#[test]
fn gouraud_lines_interpolate_rgb_and_reverse_consistently() {
    let mut forward = gpu();
    let mut reverse = gpu();
    send(
        &mut forward,
        &[0x5000_00ff, xy(2, 5), 0x00ff_0000, xy(6, 5)],
    );
    send(
        &mut reverse,
        &[0x50ff_0000, xy(6, 5), 0x0000_00ff, xy(2, 5)],
    );
    assert_eq!(
        &forward.vram()[5 * 1024 + 2..5 * 1024 + 7],
        &[0x001f, 0x1c17, 0x3c0f, 0x5c07, 0x7c00]
    );
    assert_eq!(forward.vram(), reverse.vram());
}

#[test]
fn line_clipping_signed_offset_and_size_rejection_bound_raster_work() {
    let mut gpu = gpu();
    gpu.gp0(0xe300_0000 | 2 | (2 << 10)).unwrap();
    gpu.gp0(0xe400_0000 | 8 | (8 << 10)).unwrap();
    gpu.gp0(0xe500_0000 | 4 | (4 << 11)).unwrap();
    send(&mut gpu, &[0x40ff_ffff, xy(-4, -4), xy(6, 6)]);
    for n in 0..12 {
        assert_eq!(
            gpu.vram()[n * 1024 + n],
            if (2..=8).contains(&n) { 0x7fff } else { 0 }
        );
    }
    let before = gpu.vram();
    send(&mut gpu, &[0x4000_00ff, xy(-512, 0), xy(512, 0)]);
    send(&mut gpu, &[0x4000_00ff, xy(0, -256), xy(0, 256)]);
    assert_eq!(gpu.vram(), before);
}

#[test]
fn lines_apply_semitransparency_masks_and_dither_even_when_flat() {
    let mut gpu = gpu();
    gpu.vram_mut()[0..3].copy_from_slice(&[0x2108, 0x8001, 0]);
    gpu.gp0(0xe600_0003).unwrap(); // Set mask; reject existing masked pixels.
    send(&mut gpu, &[0x5200_0080, xy(0, 0), 0x0000_0080, xy(2, 0)]);
    assert_eq!(&gpu.vram()[0..3], &[0x908c, 0x8001, 0x8008]);
    gpu.gp0(0xe600_0000).unwrap();
    gpu.gp0(0xe100_0200).unwrap();
    send(&mut gpu, &[0x4008_0808, xy(0, 4), xy(3, 4)]);
    assert_eq!(&gpu.vram()[4096..4100], &[0, 0x421, 0, 0x421]);
}

#[test]
fn flat_and_shaded_polylines_stream_and_terminate_only_on_vertex_boundaries() {
    let mut gpu = gpu();
    send(&mut gpu, &[0x4800_00ff, xy(2, 2), xy(4, 2)]);
    assert_eq!(gpu.vram()[2 * 1024 + 4], 31); // Drawn before a terminator arrives.
    send(&mut gpu, &[xy(4, 4), 0x5abc_5def]);
    assert_eq!(gpu.vram()[4 * 1024 + 4], 31);
    send(&mut gpu, &[0x5800_00ff, xy(8, 8), 0x0000_ff00, xy(10, 8)]);
    // 50005000 is a legal vertex (both low 11-bit coordinates are zero).
    // Only the COLOR word can terminate a shaded polyline.
    send(&mut gpu, &[0x00ff_0000, 0x5000_5000, 0x5000_5000]);
    assert_eq!(gpu.vram()[0], 0x7c00);
    send(&mut gpu, &[0x6800_ff00, xy(20, 20)]); // Parser is back at a new command.
    assert_eq!(gpu.vram()[20 * 1024 + 20], 0x3e0);
}

#[test]
fn command_buffer_reset_aborts_a_polyline_with_pending_color() {
    let mut gpu = gpu();
    send(
        &mut gpu,
        &[0x5800_00ff, xy(2, 2), 0x0000_ff00, xy(4, 2), 0x00ff_0000],
    );
    gpu.gp1(0x0100_0000).unwrap();
    send(&mut gpu, &[0x6800_00ff, xy(12, 12)]);
    assert_eq!(gpu.vram()[12 * 1024 + 12], 31);
}

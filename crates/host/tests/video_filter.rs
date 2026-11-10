use eidoryn_host::video_filter::{CrtFilter, OverlayKey};

#[test]
fn overlay_allocation_is_bounded_and_disabled_mode_allocates_nothing() {
    assert!(OverlayKey::new(1920, 1440, 240, CrtFilter::Off).is_none());
    for (w, h, lines) in [(0, 480, 240), (640, 0, 240), (640, 480, 0)] {
        assert!(OverlayKey::new(w, h, lines, CrtFilter::Classic).is_none());
    }
    for (w, h) in [
        (640, 480),
        (7680, 5760),
        (u32::MAX, u32::MAX),
        (1, u32::MAX),
    ] {
        let key = OverlayKey::new(w, h, 240, CrtFilter::Classic).unwrap();
        let (width, height) = key.dimensions();
        assert!((1..=2048).contains(&width));
        assert!((1..=2048).contains(&height));
        assert!(u64::from(width) * u64::from(height) * 4 <= 16 * 1024 * 1024);
    }
    assert_eq!(
        OverlayKey::new(7680, 5760, 240, CrtFilter::Classic)
            .unwrap()
            .dimensions(),
        (2048, 1536)
    );
}

#[test]
fn overlay_preserves_black_and_neutral_colors_with_bounded_attenuation() {
    let soft = OverlayKey::new(960, 720, 240, CrtFilter::Soft)
        .unwrap()
        .bgra();
    let classic = OverlayKey::new(960, 720, 240, CrtFilter::Classic)
        .unwrap()
        .bgra();
    assert_eq!(soft.len(), 960 * 720 * 4);
    for (soft, classic) in soft
        .as_chunks::<4>()
        .0
        .iter()
        .zip(classic.as_chunks::<4>().0)
    {
        // Neutral black alpha overlays cannot introduce light or a color cast.
        assert_eq!(&soft[..3], &[0, 0, 0]);
        assert_eq!(&classic[..3], &[0, 0, 0]);
        assert!(classic[3] >= soft[3]);
        assert!(classic[3] < 128); // Every pixel retains over half its signal.
    }
    let alpha = |x: usize, y: usize| classic[(y * 960 + x) * 4 + 3];
    assert!(alpha(1, 1) > alpha(481, 361)); // Vignette darkens screen edges.
    assert!(alpha(481, 360) > alpha(481, 361) + 10); // Visible beam/scanline contrast.
    let doubled = OverlayKey::new(640, 480, 240, CrtFilter::Classic)
        .unwrap()
        .bgra();
    let alpha = |y: usize| doubled[(y * 640 + 320) * 4 + 3];
    assert!(alpha(240).abs_diff(alpha(241)) >= 20); // No phase cancellation at 2x.
}

#[test]
fn unresolved_scanlines_fade_out_in_small_and_interlaced_views() {
    // Both have <=1 destination pixel per source line: only the smooth
    // vignette remains, avoiding a periodic alias pattern as windows shrink.
    let low_res = OverlayKey::new(320, 240, 240, CrtFilter::Classic)
        .unwrap()
        .bgra();
    let interlaced = OverlayKey::new(320, 240, 480, CrtFilter::Classic)
        .unwrap()
        .bgra();
    assert_eq!(low_res, interlaced);
    let alpha = |x: usize, y: usize| low_res[(y * 320 + x) * 4 + 3];
    for x in 0..320 {
        assert!(alpha(x, 120).abs_diff(alpha(319 - x, 120)) <= 1);
    }
}

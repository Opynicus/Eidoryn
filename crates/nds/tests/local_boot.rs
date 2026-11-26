//! Run a user-supplied homebrew that draws graphics during boot.
use eidoryn_nds::{HEIGHT, Machine, SCREEN_HEIGHT, WIDTH};

#[test]
#[ignore = "set EIDORYN_NDS_TEST_ROM to a local graphics homebrew"]
fn local_homebrew_draws_visible_graphics() {
    let path = std::env::var_os("EIDORYN_NDS_TEST_ROM").expect("set EIDORYN_NDS_TEST_ROM");
    let mut machine = Machine::from_rom(std::fs::read(path).unwrap()).unwrap();
    let frames: usize = std::env::var("EIDORYN_NDS_BOOT_FRAMES")
        .map_or(Ok(60), |s| s.parse())
        .unwrap();
    for _ in 0..frames {
        machine.run_frame().unwrap();
    }
    let frame = machine.pixels();
    assert_eq!(frame.len(), WIDTH * HEIGHT * 3);
    let (top, bottom) = frame.split_at(WIDTH * SCREEN_HEIGHT * 3);
    let counts = [top, bottom].map(|lcd| {
        let pixels = lcd.as_chunks::<3>().0;
        pixels.iter().filter(|&pixel| pixel != &pixels[0]).count()
    });
    eprintln!(
        "After {frames} frames, pixels differing from each LCD's top-left pixel: {counts:?}, ARM9/ARM7 PCs: {:x?}",
        machine.pcs()
    );
    assert!(counts.iter().any(|&n| n > 0), "both screens are flat");
}

#[path = "../../../tests/support/gba.rs"]
mod fixture;

use super::super::{
    Desktop, WorkspacePage,
    library::Workspace,
    tests::{click_control, desktop, press_keys, wait_for_snapshot},
    video::fit_video,
};
use eidoryn_psx::library::Library;
use gpui::{Context, TestAppContext, VisualTestContext, px, size};
use std::{
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

fn prepare_library(dir: &Path) -> Library {
    let ps1 = dir.join("PS Disc.iso");
    std::fs::write(&ps1, vec![0; 2048 * 32]).unwrap();
    let gba = dir.join("GBA Cart.gba");
    fixture::write_cartridge(&gba);
    let library = Library::open(&dir.join("library.db")).unwrap();
    for path in [ps1, gba] {
        library
            .scan_files(&[path], &AtomicBool::default(), |_| {})
            .unwrap();
    }
    library
}

fn wait_for_gba(
    view: &gpui::Entity<Desktop>,
    cx: &mut VisualTestContext,
    ready: impl Fn(&Desktop) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        view.update(cx, Desktop::receive_snapshot);
        if view.read_with(cx, |v, _| ready(v)) {
            cx.run_until_parked();
            return;
        }
        assert!(
            Instant::now() < deadline,
            "GBA worker did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui::test]
fn topbar_filters_a_shared_library_and_launches_without_ps1_bios(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let library = prepare_library(temp.path());
    let cartridge = temp.path().join("GBA Cart.gba").canonicalize().unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _: &mut Context<Desktop>| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(temp.path().join("settings.json"));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    assert!(cx.debug_bounds("library-entry-1").is_some());
    assert!(cx.debug_bounds("library-entry-2").is_none());
    click_control(cx, "core-gba");
    assert!(cx.debug_bounds("library-entry-1").is_none());
    assert!(cx.debug_bounds("library-entry-2").is_some());
    assert!(cx.debug_bounds("nav-debug").is_none());
    click_control(cx, "library-entry-2");
    click_control(cx, "lib-start");
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.running && !v.gba.snapshot.rgb.is_empty()
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.page, WorkspacePage::Game);
        assert_eq!(v.gba.snapshot.path.as_deref(), Some(cartridge.as_path()));
        assert_eq!(v.snapshot.media_revision, 0);
        assert_eq!(
            v.image.as_ref().unwrap().image.size(0),
            size(240.into(), 160.into())
        );
    });
    cx.simulate_keystrokes("x");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[255, 0, 0]));
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("x").unwrap(),
    });
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[0, 0, 255]));
    press_keys(cx, "f11");
    cx.run_until_parked();
    let stage = cx.debug_bounds("fullscreen-display").unwrap();
    let picture = fit_video(stage, view.read_with(cx, |v, _| v.video_aspect()));
    assert_eq!(picture.size.width, picture.size.height * 1.5);
    press_keys(cx, "f11");
    cx.run_until_parked();
    click_control(cx, "core-ps1");
    wait_for_gba(&view, cx, |v| !v.gba.snapshot.running);
    assert!(cx.debug_bounds("nav-debug").is_some());
    click_control(cx, "transport-run");
    wait_for_snapshot(&view, cx, |s| s.running);
    click_control(cx, "core-gba");
    wait_for_snapshot(&view, cx, |s| !s.running);
    assert!(view.read_with(cx, |v, _| v.gba.snapshot.path.is_some() && !v.running()));
}

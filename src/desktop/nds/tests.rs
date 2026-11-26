use super::super::{
    Desktop, WorkspacePage,
    library::Workspace,
    tests::{click_control, desktop, press_keys},
};
use eidoryn_psx::{library::Library, systems::checkpoints::QuickSlot};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[path = "../../../tests/support/nds.rs"]
mod fixture;

fn wait_for(
    view: &gpui::Entity<Desktop>,
    cx: &mut VisualTestContext,
    ready: impl Fn(&Desktop) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        view.update(cx, Desktop::receive_snapshot);
        if view.read_with(cx, |v, _| ready(v)) {
            cx.run_until_parked();
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Nintendo DS worker timed out: {:?}",
            view.read_with(cx, |v, _| v.nds.snapshot.error.clone())
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui::test]
fn shared_library_starts_dual_screen_session_with_stylus_and_quick_slot(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("Test.nds");
    fixture::write_cartridge(&rom);
    let expected_path = rom.canonicalize().unwrap();
    let library = Library::open(&dir.path().join("library.db")).unwrap();
    library
        .scan_files(std::slice::from_ref(&rom), &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(dir.path().join("settings.json"));
    });
    cx.simulate_resize(size(px(1280.), px(960.)));
    cx.run_until_parked();
    click_control(cx, "core-nds");
    assert!(cx.debug_bounds("open-nds").is_some());
    click_control(cx, "core-gba");
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-start");
    wait_for(&view, cx, |v| {
        v.is_nds() && v.running() && v.nds.screen.get().is_some()
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.page, WorkspacePage::Game);
        assert_eq!(v.nds.snapshot.path.as_ref(), Some(&expected_path));
        assert_eq!(
            v.image.as_ref().unwrap().image.size(0),
            size(256.into(), 384.into())
        );
        assert!(v.gba.runner.is_none());
    });
    assert!(!cx.did_prompt_for_paths());
    let bounds = view.read_with(cx, |v, _| v.nds.screen.get().unwrap());
    let stylus = bounds.origin + point(bounds.size.width * 0.5, bounds.size.height * 0.75);
    cx.simulate_mouse_down(stylus, MouseButton::Left, Modifiers::none());
    let (x, y) = view.read_with(cx, |v, _| v.nds.pen).unwrap();
    assert!(x.abs_diff(128) <= 1 && y.abs_diff(96) <= 1);
    let bottom = eidoryn_nds::WIDTH * eidoryn_nds::SCREEN_HEIGHT * 3;
    wait_for(&view, cx, |v| {
        let pixel = &v.nds.snapshot.rgb[bottom..bottom + 3];
        pixel[0] > 0 && pixel[1] > 0 && pixel[2] == 0
    });
    let sidebar = cx.debug_bounds("nav-debug").unwrap().center();
    cx.simulate_mouse_move(sidebar, MouseButton::Left, Modifiers::none());
    assert!(view.read_with(cx, |v, _| v.nds.pen.is_none()));
    cx.simulate_mouse_up(sidebar, MouseButton::Left, Modifiers::none());
    assert!(view.read_with(cx, |v, _| v.nds.pen.is_none()));
    wait_for(&view, cx, |v| {
        v.nds.snapshot.rgb[bottom..bottom + 3] == [0, 0, 255]
    });
    cx.simulate_keystrokes("s");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 1 << 10);
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("s").unwrap(),
    });
    cx.simulate_keystrokes("x");
    wait_for(&view, cx, |v| v.nds.snapshot.rgb.starts_with(&[0, 255, 0]));
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("x").unwrap(),
    });
    wait_for(&view, cx, |v| v.nds.snapshot.rgb.starts_with(&[255, 0, 0]));
    click_control(cx, "quick-save");
    wait_for(&view, cx, |v| {
        matches!(v.nds.snapshot.quick_slot, QuickSlot::Saved(_))
    });
    press_keys(cx, "f11");
    cx.run_until_parked();
    assert!(cx.debug_bounds("fullscreen-display").is_some());
    assert!(cx.debug_bounds("nds-touchscreen").is_some());
    press_keys(cx, "f11");
    cx.run_until_parked();
    click_control(cx, "nav-debug");
    click_control(cx, "debug-toggle");
    assert!(cx.debug_bounds("debug-nds").is_some());
    press_keys(cx, "f10");
    wait_for(&view, cx, |v| !v.running());
    view.read_with(cx, |v, _| {
        assert!(v.nds.snapshot.arm9_pc >= 0x0200_0000);
        assert!(v.nds.snapshot.arm7_pc >= 0x0200_0000);
    });
    press_keys(cx, "f8");
    cx.run_until_parked();
    assert!(cx.debug_bounds("tab-rewind").is_none());
    click_control(cx, "tab-controller");
    assert!(cx.debug_bounds("nds-binding-x").is_some());
    assert!(cx.debug_bounds("gba-binding-a").is_none());
    press_keys(cx, "escape");
    cx.run_until_parked();
    click_control(cx, "core-ps1");
    wait_for(&view, cx, |v| !v.nds.snapshot.running);
    click_control(cx, "nav-console");
    click_control(cx, "recent-game-0");
    wait_for(&view, cx, |v| {
        v.is_nds() && v.page == WorkspacePage::Game && v.nds.snapshot.media_revision == 2
    });
}

#[cfg(debug_assertions)] // GPUI records the clickable element bounds in debug builds.
#[gpui::test]
#[ignore = "requires the US Order of Ecclesia cartridge"]
fn ecclesia_library_launch_displays_the_language_menu(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let rom = std::env::var_os("EIDORYN_ECCLESIA_ROM").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/Castlevania - Order of Ecclesia (USA) (En,Fr).nds")
        },
        std::path::PathBuf::from,
    );
    let library = Library::open(&dir.path().join("library.db")).unwrap();
    library
        .scan_files(std::slice::from_ref(&rom), &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(dir.path().join("settings.json"));
    });
    cx.simulate_resize(size(px(1280.), px(960.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-start");
    let selected = |v: &Desktop, row: usize| {
        let width = eidoryn_nds::WIDTH;
        let offset = (eidoryn_nds::SCREEN_HEIGHT + row) * width * 3;
        v.nds
            .snapshot
            .rgb
            .get(offset..offset + width * 3)
            .is_some_and(|pixels| {
                pixels
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .filter(|&&[r, g, b]| r > 190 && g > 110 && b < 75)
                    .count()
                    >= 65
            })
    };
    wait_for(&view, cx, |v| {
        v.page == WorkspacePage::Game && selected(v, 79)
    });
    // The desktop game view has keyboard focus straight after library launch.
    cx.simulate_keystrokes("down");
    wait_for(&view, cx, |v| selected(v, 113));
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("down").unwrap(),
    });
    assert!(cx.debug_bounds("nds-touchscreen").is_some());
}

#[path = "../../../tests/support/gba_firmware.rs"]
mod firmware;
#[path = "../../../tests/support/gba.rs"]
mod fixture;

use super::super::{
    Desktop, WorkspacePage,
    library::Workspace,
    tests::{click_control, desktop, press_keys, wait_for_snapshot},
    video::fit_video,
};
use eidoryn_psx::{library::Library, preferences::Preferences};
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
fn a_missing_gba_bios_is_cleared_and_a_stale_picker_cannot_launch(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let bios = temp.path().join("gba.bin");
    let config = temp.path().join("settings.json");
    let rom = temp.path().join("game.gba");
    fixture::write_cartridge(&rom);
    let library = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    view.update(cx, |v, _| v.library = Workspace::new(Some(library)));
    click_control(cx, "core-gba");
    view.update(cx, |v, cx| {
        v.preferences_path = Some(config.clone());
        v.preferences.gba_bios_path = Some(bios.clone());
        v.gba.bios = Some(bios.clone());
        v.save_preferences();
        v.open_gba(rom.clone(), "Game".into(), cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.gba.bios.is_none() && v.gba.runner.is_none()));
    assert!(Preferences::load(&config).unwrap().gba_bios_path.is_none());
    click_control(cx, "core-ps1");
    click_control(cx, "core-gba");
    std::fs::write(&bios, firmware::firmware()).unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![bios.clone()]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.gba.bios.is_none() && v.gba.runner.is_none()));

    click_control(cx, "open-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.gba.runner.is_none()));
    cx.simulate_path_prompt_response(|_| Some(vec![bios]));
    cx.run_until_parked();
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.path.as_ref() == Some(&rom) && v.running()
    });
}

#[gpui::test]
fn library_launch_prompts_for_gba_firmware_and_remembers_the_selected_default(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let bios = temp.path().join("gba.bin");
    std::fs::write(&bios, firmware::firmware()).unwrap();
    let ps1_bios = temp.path().join("ps1.bin");
    std::fs::write(&ps1_bios, vec![0; eidoryn_psx::bus::BIOS_SIZE]).unwrap();
    let config = temp.path().join("settings.json");
    let library = prepare_library(temp.path());
    let cartridge = temp.path().join("GBA Cart.gba").canonicalize().unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _: &mut Context<Desktop>| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(config.clone());
        v.bios = Some(ps1_bios.clone());
        v.preferences.bios_path = Some(ps1_bios.clone());
        v.save_preferences();
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
    assert!(view.read_with(cx, |v, _| v.gba.runner.is_none()));
    cx.simulate_path_prompt_response(|_| Some(vec![ps1_bios.clone()]));
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |v, _| v.notice.as_deref().unwrap().contains("16 KiB")
            && v.gba.runner.is_none())
    );
    click_control(cx, "lib-start");
    cx.simulate_path_prompt_response(|_| Some(vec![bios.clone()]));
    cx.run_until_parked();
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
    assert!(Preferences::load(&config).unwrap().gba_bios_path.is_none());
    press_keys(cx, "f8");
    cx.run_until_parked();
    click_control(cx, "bios-details");
    click_control(cx, "set-default-bios");
    let saved = Preferences::load(&config).unwrap();
    assert_eq!(saved.gba_bios_path, Some(bios.canonicalize().unwrap()));
    assert_eq!(saved.bios_path, Some(ps1_bios));
    press_keys(cx, "escape");
    cx.run_until_parked();
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

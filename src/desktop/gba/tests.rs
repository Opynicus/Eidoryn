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
    cx.run_until_parked();
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

fn install_firmware(dir: &Path) {
    let folder = dir.join("firmware");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("gba_bios.bin"), firmware::firmware()).unwrap();
}

#[gpui::test]
fn first_gba_launch_imports_firmware_and_resumes_the_cartridge(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("game.gba");
    fixture::write_cartridge(&rom);
    let source = temp.path().join("dump.bin");
    let bios = firmware::firmware();
    std::fs::write(&source, &bios).unwrap();
    let library = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    view.update(cx, |v, _| v.library = Workspace::new(Some(library)));
    click_control(cx, "core-gba");
    assert!(cx.debug_bounds("open-bios").is_none());
    click_control(cx, "open-gba");
    click_control(cx, "core-ps1");
    click_control(cx, "core-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.gba.runner.is_none()));

    click_control(cx, "open-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
    wait_for_gba(&view, cx, |v| v.gba.firmware_setup.is_some());
    click_control(cx, "gba-firmware-project");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://github.com/Cult-of-GBA/BIOS")
    );
    click_control(cx, "import-gba-firmware");
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(cx.debug_bounds("gba-firmware-setup").is_some());
    assert!(view.read_with(cx, |v, _| v.page == WorkspacePage::Console && !v.running()));
    click_control(cx, "cancel-gba-firmware");
    assert!(cx.debug_bounds("gba-firmware-setup").is_none());

    click_control(cx, "open-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
    wait_for_gba(&view, cx, |v| v.gba.firmware_setup.is_some());
    click_control(cx, "import-gba-firmware");
    cx.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
    cx.run_until_parked();
    assert!(cx.debug_bounds("gba-import-error").is_some());
    let installed = temp.path().join("firmware/gba_bios.bin");
    assert!(!installed.exists());
    click_control(cx, "import-gba-firmware");
    cx.simulate_path_prompt_response(|opts| {
        assert!(opts.files && !opts.multiple && !opts.directories);
        Some(vec![source.clone()])
    });
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.path.as_ref() == Some(&rom) && v.running()
    });
    assert!(cx.debug_bounds("gba-firmware-setup").is_none());
    assert_eq!(std::fs::read(installed).unwrap(), bios);
    std::fs::remove_file(source).unwrap();
    let second_rom = temp.path().join("second.gba");
    fixture::write_cartridge(&second_rom);
    view.update(cx, |v, cx| {
        v.open_gba(second_rom.clone(), "Second game".into(), cx)
    });
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.path.as_ref() == Some(&second_rom) && v.running()
    });
    assert!(!cx.did_prompt_for_paths());
}

#[gpui::test]
fn library_firmware_import_is_cancelled_by_switching_cores(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let library = prepare_library(temp.path());
    let source = temp.path().join("firmware.bin");
    std::fs::write(&source, firmware::firmware()).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(library)));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "core-gba");
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-2");
    click_control(cx, "lib-start");
    wait_for_gba(&view, cx, |v| v.gba.firmware_setup.is_some());
    click_control(cx, "import-gba-firmware");
    click_control(cx, "core-ps1");
    click_control(cx, "core-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![source.clone()]));
    cx.run_until_parked();
    assert!(cx.debug_bounds("gba-firmware-setup").is_none());
    assert!(!temp.path().join("firmware/gba_bios.bin").exists());
    assert!(view.read_with(cx, |v, _| v.page == WorkspacePage::Library && !v.running()));
    click_control(cx, "library-entry-2");
    click_control(cx, "lib-start");
    wait_for_gba(&view, cx, |v| v.gba.firmware_setup.is_some());
    click_control(cx, "import-gba-firmware");
    cx.simulate_path_prompt_response(|_| Some(vec![source]));
    wait_for_gba(&view, cx, |v| v.page == WorkspacePage::Game && v.running());
}

#[gpui::test]
fn library_launch_uses_shared_gba_firmware_and_keeps_the_ps1_selection(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    install_firmware(temp.path());
    let ps1_bios = temp.path().join("ps1.bin");
    std::fs::write(&ps1_bios, vec![0; eidoryn_psx::bus::BIOS_SIZE]).unwrap();
    let library = prepare_library(temp.path());
    let cartridge = temp.path().join("GBA Cart.gba").canonicalize().unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _: &mut Context<Desktop>| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(temp.path().join("settings.json"));
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
    press_keys(cx, "f8");
    cx.run_until_parked();
    assert!(cx.debug_bounds("tab-appearance").is_some());
    assert!(cx.debug_bounds("tab-firmware").is_none());
    assert!(cx.debug_bounds("open-bios").is_none());
    assert!(view.read_with(cx, |v, _| v.preferences.bios_path.as_ref()
        == Some(&ps1_bios)));
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

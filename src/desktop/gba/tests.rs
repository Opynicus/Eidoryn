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
    wait_for_gba_with_timeout(view, cx, Duration::from_secs(5), ready);
}

fn wait_for_gba_with_timeout(
    view: &gpui::Entity<Desktop>,
    cx: &mut VisualTestContext,
    timeout: Duration,
    ready: impl Fn(&Desktop) -> bool,
) {
    cx.run_until_parked();
    let deadline = Instant::now() + timeout;
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
fn direct_open_starts_a_cartridge_without_bios_in_each_session(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("game.gba");
    fixture::write_cartridge(&rom);

    for _ in 0..2 {
        let library = Library::open(&temp.path().join("library.db")).unwrap();
        let (view, visual) = cx.add_window_view(desktop);
        visual.simulate_resize(size(px(1280.), px(900.)));
        visual.run_until_parked();
        view.update(visual, |v, _| v.library = Workspace::new(Some(library)));
        click_control(visual, "core-gba");
        click_control(visual, "open-gba");
        visual.simulate_path_prompt_response(|_| Some(vec![rom.clone()]));
        wait_for_gba(&view, visual, |v| {
            v.page == WorkspacePage::Game
                && v.running()
                && v.gba.snapshot.path.as_ref() == Some(&rom)
                && v.gba.snapshot.rgb.starts_with(&[0, 0, 255])
        });
        assert!(!visual.did_prompt_for_paths());
        assert!(visual.opened_url().is_none());
        visual.update(|window, _| window.remove_window());
        drop(view);
        visual.run_until_parked();
    }
    assert!(!temp.path().join("firmware").exists());
}

#[gpui::test]
fn switching_cores_discards_an_open_cartridge_picker(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("game.gba");
    fixture::write_cartridge(&rom);
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "core-gba");
    click_control(cx, "open-gba");
    click_control(cx, "core-ps1");
    click_control(cx, "core-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![rom]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.gba.runner.is_none()));
}

#[gpui::test]
fn shared_library_routes_a_cartridge_to_hle_and_preserves_the_ps1_bios(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
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
    click_control(cx, "library-entry-2");
    click_control(cx, "core-gba");
    assert!(cx.debug_bounds("library-entry-1").is_some());
    assert!(cx.debug_bounds("library-entry-2").is_some());
    assert!(cx.debug_bounds("nav-debug").is_some());
    click_control(cx, "core-ps1");
    click_control(cx, "lib-start");
    cx.run_until_parked();
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.running && !v.gba.snapshot.rgb.is_empty()
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.page, WorkspacePage::Game);
        assert_eq!(v.gba.snapshot.path.as_deref(), Some(cartridge.as_path()));
        assert_eq!(v.snapshot.media_revision, 0);
        assert_eq!(v.preferences.recent_discs[0].path, cartridge);
        assert_eq!(
            v.image.as_ref().unwrap().image.size(0),
            size(240.into(), 160.into())
        );
    });
    assert!(!cx.did_prompt_for_paths());
    assert!(!temp.path().join("firmware").exists());
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
    click_control(cx, "nav-console");
    click_control(cx, "recent-game-0");
    wait_for_snapshot(&view, cx, |s| !s.running);
    wait_for_gba(&view, cx, |v| {
        v.is_gba() && v.gba.snapshot.media_revision == 2 && v.page == WorkspacePage::Game
    });
    assert_eq!(
        view.read_with(cx, |v, _| v.preferences.recent_discs.len()),
        1
    );
}

#[gpui::test]
fn remapped_gba_buttons_drive_the_cartridge_and_leave_ps1_bindings_intact(cx: &mut TestAppContext) {
    use eidoryn_psx::{
        keybindings::{GbaButton, PadButton},
        preferences::Preferences,
    };

    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("controls.gba");
    let settings = dir.path().join("settings.json");
    fixture::write_cartridge(&rom);
    let library = Library::open(&dir.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(1000.)));
    view.update(cx, |v, cx| {
        v.preferences_path = Some(settings.clone());
        v.library = Workspace::new(Some(library));
        v.open_gba(rom, "Controls".into(), cx);
    });
    wait_for_gba(&view, cx, |v| {
        v.running() && v.gba.snapshot.rgb.starts_with(&[0, 0, 255])
    });
    click_control(cx, "edit-gba-controls");
    assert!(cx.debug_bounds("tab-controller").is_some());
    assert!(cx.debug_bounds("turbo-cross").is_none());

    click_control(cx, "gba-binding-a");
    press_keys(cx, "z");
    cx.run_until_parked();
    assert!(cx.debug_bounds("bindings-conflict").is_some());
    click_control(cx, "bindings-apply");
    assert_eq!(
        view.read_with(cx, |v, _| v
            .preferences
            .gba_keybindings
            .key(GbaButton::A)
            .to_owned()),
        "x"
    );
    click_control(cx, "gba-binding-a");
    press_keys(cx, "f5");
    cx.run_until_parked();
    assert!(cx.debug_bounds("bindings-conflict").is_some());
    click_control(cx, "gba-binding-a");
    press_keys(cx, "j");
    click_control(cx, "bindings-apply");
    let saved = Preferences::load(&settings).unwrap();
    assert_eq!(saved.gba_keybindings.key(GbaButton::A), "j");
    assert_eq!(saved.keybindings.key(PadButton::Cross), "x");
    press_keys(cx, "escape");
    cx.simulate_keystrokes("x");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.simulate_keystrokes("j");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[255, 0, 0]));
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("ctrl-j").unwrap(),
    });
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[0, 0, 255]));
    cx.simulate_keystrokes("j");
    press_keys(cx, "f8");
    wait_for_gba(&view, cx, |v| {
        v.pressed_buttons == 0 && v.gba.snapshot.rgb.starts_with(&[0, 0, 255])
    });
    press_keys(cx, "escape");
    cx.simulate_keystrokes("j");
    click_control(cx, "core-ps1");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
}

#[gpui::test]
fn debugger_inspects_and_steps_the_selected_cartridge(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("debug.gba");
    fixture::write_cartridge(&rom);
    let library = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    view.update(cx, |v, cx| {
        v.library = Workspace::new(Some(library));
        v.open_gba(rom, "Probe".into(), cx);
    });
    wait_for_gba(&view, cx, |v| v.gba.snapshot.path.is_some());
    click_control(cx, "nav-debug");
    click_control(cx, "debug-toggle");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.debug.is_some());
    assert!(cx.debug_bounds("debug-instructions").is_some());
    assert!(cx.debug_bounds("debug-registers").is_some());
    press_keys(cx, "ctrl-r");
    wait_for_gba(&view, cx, |v| {
        !v.running()
            && v.gba
                .snapshot
                .debug
                .as_ref()
                .is_some_and(|d| d.registers[15] == 0x0800_0000)
    });
    press_keys(cx, "f10");
    wait_for_gba(&view, cx, |v| {
        !v.running()
            && v.gba
                .snapshot
                .debug
                .as_ref()
                .is_some_and(|d| d.registers[15] == 0x0800_00c0)
    });
    press_keys(cx, "f9");
    wait_for_gba(&view, cx, |v| {
        !v.running()
            && v.gba
                .snapshot
                .debug
                .as_ref()
                .is_some_and(|d| d.display_control == 0x0403)
    });
    view.read_with(cx, |v, _| {
        assert_eq!(v.snapshot_mode, super::super::SnapshotMode::Display);
        assert_eq!(v.snapshot.disassembly.len(), 0);
    });
    click_control(cx, "nav-game");
    assert!(view.read_with(cx, |v, _| !v.gba.capturing_debug
        && v.gba.snapshot.debug.is_none()));
}

fn cartridge_window<'a>(
    cx: &'a mut TestAppContext,
    dir: &Path,
    record_rewind: bool,
) -> (gpui::Entity<Desktop>, &'a mut VisualTestContext) {
    let rom = dir.join("Game.gba");
    fixture::write_cartridge(&rom);
    let library = Library::open(&dir.join("games.sqlite")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(1050.)));
    view.update(cx, |v, cx| {
        v.preferences_path = Some(dir.join("settings.json"));
        v.preferences.rewind_enabled = record_rewind;
        v.library = Workspace::new(Some(library));
        v.open_gba(rom, "Game".into(), cx);
    });
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[0, 0, 255]));
    (view, cx)
}

#[gpui::test]
fn gba_quick_slot_history_and_rewind_follow_the_selected_core(cx: &mut TestAppContext) {
    use eidoryn_psx::systems::checkpoints::QuickSlot;
    let temp = tempfile::tempdir().unwrap();
    let (view, cx) = cartridge_window(cx, temp.path(), true);
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rewind_states > 0);
    cx.simulate_keystrokes("ctrl-z");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rewinding);
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("ctrl-z").unwrap(),
    });
    wait_for_gba(&view, cx, |v| {
        v.rewind_sources == 0 && !v.gba.snapshot.rewinding && v.running()
    });
    press_keys(cx, "f5");
    wait_for_gba(&view, cx, |v| !v.running());
    click_control(cx, "quick-save");
    wait_for_gba(&view, cx, |v| {
        matches!(v.gba.snapshot.quick_slot, QuickSlot::Saved(_))
    });
    press_keys(cx, "ctrl-s");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.save_states.len() == 1);
    click_control(cx, "manage-states");
    assert!(cx.debug_bounds("recent-snapshot-0").is_some());
    click_control(cx, "manage-all-snapshots");
    assert!(cx.debug_bounds("load-snapshot-0").is_some());
    click_control(cx, "core-ps1");
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::SaveStates);
    assert!(cx.debug_bounds("load-snapshot-0").is_none());
    click_control(cx, "core-gba");
    click_control(cx, "quick-load");
    wait_for_gba(&view, cx, |v| {
        v.gba
            .snapshot
            .success_message
            .as_deref()
            .is_some_and(|s| s.contains("Loaded"))
    });
    assert!(!view.read_with(cx, |v, _| v.running()));
    click_control(cx, "delete-snapshot-0");
    click_control(cx, "confirm-delete-snapshot");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.save_states.is_empty());
}

#[gpui::test]
fn cartridge_save_controls_sync_export_and_confirm_import(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (view, cx) = cartridge_window(cx, temp.path(), false);
    let sav = view.read_with(cx, |v, _| v.gba.snapshot.battery_path.clone().unwrap());
    click_control(cx, "gba-sync-save");
    wait_for_gba(&view, cx, |_| sav.is_file());
    let original = std::fs::read(&sav).unwrap();
    assert_eq!(original.len(), 32 * 1024);
    let exported = temp.path().join("portable.sav");
    click_control(cx, "gba-export-save");
    cx.simulate_new_path_selection(|_| Some(exported.clone()));
    wait_for_gba(&view, cx, |_| exported.is_file());
    assert_eq!(std::fs::read(&exported).unwrap(), original);
    let mut replacement = original.clone();
    replacement[0] = 7;
    replacement[321] = 0x42;
    std::fs::write(&exported, replacement).unwrap();
    for confirm in [false, true] {
        click_control(cx, "gba-import-save");
        cx.simulate_path_prompt_response(|_| Some(vec![exported.clone()]));
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        let text = view.read_with(cx, |v, _| {
            v.tr(if confirm {
                eidoryn_psx::i18n::Text::ImportCartridgeSave
            } else {
                eidoryn_psx::i18n::Text::RenameCancel
            })
        });
        cx.simulate_prompt_answer(text);
        cx.run_until_parked();
        if !confirm {
            assert_eq!(std::fs::read(&sav).unwrap(), original);
        }
    }
    wait_for_gba(&view, cx, |v| {
        v.gba.snapshot.rgb.get(3..6) == Some(&[66, 0, 0])
    });
    assert_eq!(std::fs::read(sav).unwrap()[321], 0x42);
}

#[gpui::test]
fn legacy_gba_button_keeps_priority_over_a_new_snapshot_shortcut(cx: &mut TestAppContext) {
    use eidoryn_psx::{keybindings::GbaButton, shortcuts::ShortcutAction};
    let temp = tempfile::tempdir().unwrap();
    let (view, cx) = cartridge_window(cx, temp.path(), false);
    view.update(cx, |v, _| {
        v.preferences
            .gba_keybindings
            .rebind(GbaButton::A, "j")
            .unwrap();
        v.preferences.shortcuts.set(
            ShortcutAction::Snapshot,
            Some("j".to_owned().try_into().unwrap()),
        );
        v.preferences
            .save(&temp.path().join("settings.json"))
            .unwrap();
    });
    cx.simulate_keystrokes("j");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rgb.starts_with(&[255, 0, 0]));
    assert!(view.read_with(cx, |v, _| v.gba.snapshot.save_states.is_empty()));
}

#[gpui::test]
fn switching_cores_while_importing_keeps_the_original_save(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (view, cx) = cartridge_window(cx, temp.path(), false);
    let sav = view.read_with(cx, |v, _| v.gba.snapshot.battery_path.clone().unwrap());
    click_control(cx, "gba-sync-save");
    wait_for_gba(&view, cx, |_| sav.is_file());
    let original = std::fs::read(&sav).unwrap();
    let replacement = temp.path().join("another.sav");
    std::fs::write(&replacement, vec![55; original.len()]).unwrap();
    click_control(cx, "gba-import-save");
    click_control(cx, "core-ps1");
    click_control(cx, "core-gba");
    cx.simulate_path_prompt_response(|_| Some(vec![replacement]));
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert_eq!(std::fs::read(sav).unwrap(), original);
}

#[cfg(debug_assertions)] // GPUI records the clickable bounds in debug builds.
#[gpui::test]
#[ignore = "requires a local cartridge in EIDORYN_GBA_TEST_ROM"]
fn local_library_cartridge_supports_rewind_and_quick_load(cx: &mut TestAppContext) {
    use eidoryn_psx::preferences::GbaRenderer;
    use eidoryn_psx::systems::checkpoints::QuickSlot;
    let hardware = std::env::var_os("EIDORYN_GBA_TEST_HARDWARE").is_some();
    let rom = std::path::PathBuf::from(std::env::var_os("EIDORYN_GBA_TEST_ROM").unwrap())
        .canonicalize()
        .unwrap();
    let data = tempfile::tempdir().unwrap();
    let library = Library::open(&data.path().join("library.db")).unwrap();
    library
        .scan_files(std::slice::from_ref(&rom), &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.library = Workspace::new(Some(library));
        v.preferences_path = Some(data.path().join("settings.json"));
        v.preferences.rewind_enabled = true;
        v.preferences.gba_renderer = GbaRenderer::from_hardware(hardware);
    });
    cx.simulate_resize(size(px(1280.), px(960.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-start");
    wait_for_gba_with_timeout(&view, cx, Duration::from_secs(60), |v| {
        let s = &v.gba.snapshot;
        assert!(s.error.is_none(), "{}: {:?}", rom.display(), s.error);
        assert!(
            s.graphics_error.is_none(),
            "{}: {:?}",
            rom.display(),
            s.graphics_error
        );
        let (rgb, _) = s.rgb.as_chunks::<3>();
        s.rewind_states >= 2
            && rgb
                .first()
                .is_some_and(|first| rgb.iter().filter(|c| *c != first).take(300).count() == 300)
    });
    assert!(view.read_with(cx, |v, _| v.is_gba()
        && v.running()
        && v.gba.snapshot.graphics_device.is_some() == hardware
        && v.page == WorkspacePage::Game
        && v.gba.snapshot.path.as_ref() == Some(&rom)));
    cx.simulate_keystrokes("ctrl-z");
    wait_for_gba(&view, cx, |v| v.gba.snapshot.rewinding);
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("ctrl-z").unwrap(),
    });
    wait_for_gba(&view, cx, |v| v.running() && !v.gba.snapshot.rewinding);
    press_keys(cx, "f5");
    wait_for_gba(&view, cx, |v| !v.running());
    let saved = view.read_with(cx, |v, _| v.gba.snapshot.rgb.clone());
    click_control(cx, "quick-save");
    wait_for_gba(&view, cx, |v| {
        matches!(v.gba.snapshot.quick_slot, QuickSlot::Saved(_))
    });
    press_keys(cx, "f5");
    wait_for_gba(&view, cx, Desktop::running);
    click_control(cx, "quick-load");
    wait_for_gba(&view, cx, |v| {
        v.gba
            .snapshot
            .success_message
            .as_deref()
            .is_some_and(|s| s.contains("Loaded"))
    });
    press_keys(cx, "f5");
    wait_for_gba(&view, cx, |v| !v.running());
    click_control(cx, "quick-load");
    wait_for_gba(&view, cx, |v| {
        !v.running()
            && v.gba.snapshot.rgb == saved
            && v.gba
                .snapshot
                .success_message
                .as_deref()
                .is_some_and(|s| s.contains("Loaded"))
    });
    assert!(!cx.did_prompt_for_paths());
}

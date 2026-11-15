use super::*;
use eidoryn_psx::keybindings::{KeyBindings, PadButton};
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn video_updates_reuse_sidebar_but_ui_changes_invalidate_it(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let sidebar = view.read_with(cx, |view, _| view.sidebar.clone().unwrap());
    let initial = sidebar.read_with(cx, |sidebar, _| sidebar.renders);
    assert!(initial > 0);
    for _ in 0..5 {
        view.update(cx, |view, cx| {
            view.snapshot.video_frames += 1;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.renders), initial);

    for change in 0..4 {
        let before = sidebar.read_with(cx, |sidebar, _| sidebar.renders);
        view.update(cx, |view, cx| {
            match change {
                0 => view.language = eidoryn_psx::i18n::Language::En,
                1 => view.themes.select("ocean").unwrap(),
                2 => view.page = WorkspacePage::Debug,
                _ => view.graphics.open = true,
            }
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.renders) > before);
    }
    let before = sidebar.read_with(cx, |sidebar, _| sidebar.renders);
    cx.simulate_resize(size(px(980.), px(680.)));
    cx.run_until_parked();
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.renders) > before);
    let compact_width = cx.debug_bounds("nav-console").unwrap().size.width;
    cx.simulate_resize(size(px(1250.), px(680.)));
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds("nav-console").unwrap().size.width,
        compact_width
    );
    cx.simulate_resize(size(px(1251.), px(680.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("nav-console").unwrap().size.width > compact_width);
}

#[gpui::test]
fn sidebar_clicks_navigate_between_workspaces(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    for (selector, expected) in [
        ("nav-debug", WorkspacePage::Debug),
        ("nav-save-states", WorkspacePage::SaveStates),
        ("nav-console", WorkspacePage::Console),
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .expect("navigation button is visible");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        assert_eq!(view.read_with(cx, |view, _| view.page), expected);
    }
}

#[gpui::test]
fn ctrl_l_does_not_leave_the_game_or_release_input(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.run_until_parked();
    cx.simulate_keystrokes("f6 z");
    assert!(view.read_with(cx, |view, _| view.game_view));
    let pressed = view.read_with(cx, |view, _| view.pressed_buttons);
    assert_ne!(pressed, 0);
    cx.simulate_keystrokes("ctrl-l");
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.page, WorkspacePage::Console);
        assert!(view.game_view);
        assert_eq!(view.pressed_buttons, pressed);
    });
    assert!(cx.debug_bounds("save-states-page").is_none());
}

#[gpui::test]
fn snapshot_dropdown_selects_and_loads_the_chosen_state(cx: &mut TestAppContext) {
    use eidoryn_psx::systems::ps1::{input::PadInput, state_file::StateStore};
    let root = test_config_path().with_extension("states");
    let mut machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "UI test").unwrap();
    let store = StateStore::new(&root, &machine);
    machine.cpu.registers[5] = 17;
    let older = store.save(&machine, &PadInput::default()).unwrap();
    machine.cpu.registers[5] = 29;
    let newer = store.save(&machine, &PadInput::default()).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    view.update(cx, |view, cx| {
        view.game_view = true;
        view.snapshot.save_states = store.list().unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    for selector in ["save-state", "save-state-picker", "load-state"] {
        assert!(cx.debug_bounds(selector).is_some());
    }
    let picker = cx.debug_bounds("save-state-picker").unwrap();
    cx.simulate_click(picker.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let option = cx.debug_bounds("save-state-option-1").unwrap();
    cx.simulate_click(option.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.selected_save_state.clone()),
        Some(older)
    );
    assert!(!view.read_with(cx, |v, _| v.save_state_picker_open));
    cx.simulate_click(picker.center(), gpui::Modifiers::none());
    press_keys(cx, "home enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.selected_save_state.clone()),
        Some(newer)
    );
    assert!(!view.read_with(cx, |v, _| v.save_state_picker_open));
    let load = cx.debug_bounds("load-state").unwrap();
    cx.simulate_click(load.center(), gpui::Modifiers::none());
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(snapshot) = view.read_with(cx, |v, _| v.runner.take_snapshot())
            && snapshot.success_message.is_some()
        {
            assert_eq!(snapshot.cpu.registers[5], 29);
            assert!(!snapshot.running);
            assert!(snapshot.message.is_none());
            break;
        }
        assert!(Instant::now() < deadline, "selected snapshot did not load");
        std::thread::sleep(Duration::from_millis(1));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn snapshot_feedback_and_selection_follow_the_current_game(cx: &mut TestAppContext) {
    use eidoryn_psx::systems::ps1::state_file::SaveStateInfo;
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(1000.)));
    view.update(cx, |view, cx| {
        view.snapshot.success_message = Some("Saved state".into());
        view.snapshot.save_states = vec![SaveStateInfo {
            path: "game-a/old.ps1state".into(),
            name: "A".into(),
        }];
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("success-notice").is_some());
    assert!(cx.debug_bounds("error-notice").is_none());
    assert_eq!(
        view.read_with(cx, |v, _| v.selected_save_state.clone()),
        Some("game-a/old.ps1state".into())
    );
    view.update(cx, |view, cx| {
        view.save_state_picker_open = true;
        view.snapshot.title = "Game B".into();
        view.snapshot.save_states = vec![SaveStateInfo {
            path: "game-b/new.ps1state".into(),
            name: "B".into(),
        }];
        view.snapshot.message = Some("Load failed".into());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("error-notice").is_some());
    assert!(cx.debug_bounds("success-notice").is_none());
    assert_eq!(
        view.read_with(cx, |v, _| v.selected_save_state.clone()),
        Some("game-b/new.ps1state".into())
    );
    assert!(!view.read_with(cx, |v, _| v.save_state_picker_open));
    view.update(cx, |view, cx| {
        view.snapshot.save_states.clear();
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.selected_save_state.is_none()));
    for selector in ["save-state-picker", "load-state"] {
        let center = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_click(center, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(!view.read_with(cx, |v, _| v.save_state_picker_open));
    }
}

#[gpui::test]
fn selected_bios_is_persisted_after_ack_and_clear_keeps_the_session(cx: &mut TestAppContext) {
    use eidoryn_psx::preferences::Preferences;
    let bios = TestConfig(test_config_path().with_extension("bios"));
    std::fs::write(&bios.0, vec![0; 512 * 1024]).unwrap();
    let bios_path = std::fs::canonicalize(&bios.0).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let config = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1400.), px(1000.)));
    view.update(cx, |view, cx| {
        view.save_preferences();
        view.command(
            Command::Load {
                path: bios.0.clone(),
                bios: true,
            },
            cx,
        );
    });
    assert!(view.read_with(cx, |v, _| v.bios.is_none()));
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
    wait_for_snapshot(&view, cx, |snapshot| snapshot.media_revision == 1);
    assert_eq!(
        Preferences::load(&config.0).unwrap().bios_path,
        Some(bios_path.clone())
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.bios.clone()),
        Some(bios_path.clone())
    );

    let missing = test_config_path().with_extension("bios");
    view.update(cx, |view, cx| {
        view.command(
            Command::Load {
                path: missing,
                bios: true,
            },
            cx,
        );
    });
    wait_for_snapshot(&view, cx, |snapshot| snapshot.message.is_some());
    assert_eq!(
        Preferences::load(&config.0).unwrap().bios_path,
        Some(bios_path.clone())
    );
    assert_eq!(view.read_with(cx, |v, _| v.snapshot.media_revision), 1);

    cx.run_until_parked();
    let clear = cx.debug_bounds("clear-default-bios").unwrap();
    cx.simulate_click(clear.center(), gpui::Modifiers::none());
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
    assert_eq!(view.read_with(cx, |v, _| v.bios.clone()), Some(bios_path));
    assert!(bios.0.is_file());
    view.update(cx, Desktop::receive_snapshot);
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
}

#[gpui::test]
fn missing_default_bios_is_removed_before_a_disc_is_loaded(cx: &mut TestAppContext) {
    use eidoryn_psx::{i18n::Text, preferences::Preferences};
    let missing = test_config_path().with_extension("bios");
    let disc = TestConfig(test_config_path().with_extension("iso"));
    std::fs::write(&disc.0, vec![0; 2048]).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let config = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    view.update(cx, |view, cx| {
        view.bios = Some(missing.clone());
        view.preferences.bios_path = Some(missing);
        view.save_preferences();
        view.load_selected_disc(disc.0.clone(), false, cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.bios.is_none());
        assert!(view.preferences.bios_path.is_none());
        assert_eq!(view.notice.as_deref(), Some(view.tr(Text::MissingBios)));
        assert_eq!(view.snapshot.media_revision, 0);
        assert_eq!(view.preferences.recent_discs, []);
    });
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
    assert!(disc.0.is_file());
}

#[gpui::test]
fn recent_games_clicks_record_only_successful_loads_and_move_them_to_front(
    cx: &mut TestAppContext,
) {
    use eidoryn_psx::preferences::Preferences;
    let bios = TestConfig(test_config_path().with_extension("bios"));
    std::fs::write(&bios.0, vec![0; 512 * 1024]).unwrap();
    let discs = [
        TestConfig(test_config_path().with_extension("iso")),
        TestConfig(test_config_path().with_extension("iso")),
    ];
    for disc in &discs {
        std::fs::write(&disc.0, vec![0; 2048]).unwrap();
    }
    let (view, cx) = cx.add_window_view(desktop);
    let config = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1400.), px(1000.)));
    view.update(cx, |view, _| {
        view.bios = Some(bios.0.clone());
        view.save_preferences();
    });
    for (index, disc) in discs.iter().enumerate() {
        let before = Preferences::load(&config.0).unwrap().recent_discs;
        view.update(cx, |view, cx| {
            view.load_selected_disc(disc.0.clone(), false, cx);
        });
        assert_eq!(Preferences::load(&config.0).unwrap().recent_discs, before);
        wait_for_snapshot(&view, cx, |snapshot| {
            snapshot.media_revision == index as u64 + 1
        });
        let saved = Preferences::load(&config.0).unwrap();
        assert_eq!(saved.recent_discs.len(), index + 1);
        assert_eq!(
            saved.recent_discs[0].path,
            std::fs::canonicalize(&disc.0).unwrap()
        );
        assert!(!saved.recent_discs[0].fast_boot);
    }
    for (selector, revision) in [("recent-game-1", 3), ("recent-game-0", 4)] {
        cx.run_until_parked();
        let recent = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(recent.center(), gpui::Modifiers::none());
        wait_for_snapshot(&view, cx, |snapshot| snapshot.media_revision == revision);
        let saved = Preferences::load(&config.0).unwrap();
        assert_eq!(saved.recent_discs.len(), 2);
        assert_eq!(
            saved.recent_discs[0].path,
            std::fs::canonicalize(&discs[0].0).unwrap()
        );
        assert_eq!(
            saved.recent_discs[1].path,
            std::fs::canonicalize(&discs[1].0).unwrap()
        );
    }
    let before = Preferences::load(&config.0).unwrap().recent_discs;
    let invalid = TestConfig(test_config_path().with_extension("iso"));
    std::fs::write(&invalid.0, b"invalid image").unwrap();
    view.update(cx, |view, cx| {
        view.load_selected_disc(invalid.0.clone(), false, cx);
    });
    wait_for_snapshot(&view, cx, |snapshot| snapshot.message.is_some());
    assert_eq!(view.read_with(cx, |v, _| v.snapshot.media_revision), 4);
    assert_eq!(Preferences::load(&config.0).unwrap().recent_discs, before);
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
}

#[gpui::test]
fn missing_recent_games_are_removed_and_clearing_history_keeps_files(cx: &mut TestAppContext) {
    use eidoryn_psx::{i18n::Text, preferences::Preferences};
    let discs = [
        TestConfig(test_config_path().with_extension("iso")),
        TestConfig(test_config_path().with_extension("iso")),
    ];
    for disc in &discs {
        std::fs::write(&disc.0, vec![0; 2048]).unwrap();
    }
    let (view, cx) = cx.add_window_view(desktop);
    let config = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1400.), px(1000.)));
    view.update(cx, |view, cx| {
        for disc in &discs {
            view.preferences.remember_disc(&disc.0, false).unwrap();
        }
        view.save_preferences();
        cx.notify();
    });
    std::fs::remove_file(&discs[1].0).unwrap();
    cx.run_until_parked();
    let recent = cx.debug_bounds("recent-game-0").unwrap();
    cx.simulate_click(recent.center(), gpui::Modifiers::none());
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.notice.as_deref(),
            Some(view.tr(Text::MissingRecentGame))
        );
        assert_eq!(view.snapshot.media_revision, 0);
    });
    let saved = Preferences::load(&config.0).unwrap();
    assert_eq!(saved.recent_discs.len(), 1);
    assert_eq!(
        saved.recent_discs[0].path,
        std::fs::canonicalize(&discs[0].0).unwrap()
    );
    cx.run_until_parked();
    let clear = cx.debug_bounds("clear-recent-games").unwrap();
    cx.simulate_click(clear.center(), gpui::Modifiers::none());
    assert_eq!(Preferences::load(&config.0).unwrap().recent_discs, []);
    assert!(discs[0].0.is_file());
}

fn wait_for_snapshot(
    view: &gpui::Entity<Desktop>,
    cx: &mut VisualTestContext,
    ready: impl Fn(&Snapshot) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        view.update(cx, Desktop::receive_snapshot);
        if view.read_with(cx, |view, _| ready(&view.snapshot)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not acknowledge media selection"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui::test]
fn debugger_is_optional_and_hidden_views_release_display_resources(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.image.is_some()));
    assert!(!view.read_with(cx, |v, _| v.debugger_enabled));
    let nav = cx.debug_bounds("nav-debug").unwrap();
    cx.simulate_click(nav.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.image.is_none()));
    assert!(cx.debug_bounds("debug-instructions").is_none());
    let toggle = cx.debug_bounds("debug-toggle").unwrap();
    cx.simulate_click(toggle.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot_mode),
        SnapshotMode::Debug
    );
    // The UI waits for an acknowledged debug snapshot before showing registers.
    assert!(cx.debug_bounds("debug-instructions").is_none());
    view.update(cx, |v, cx| {
        v.snapshot = Snapshot::capture(&Machine::demo(), false, None, 0.);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("debug-instructions").is_some());
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot_mode),
        SnapshotMode::Display
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot.disassembly.capacity()),
        0
    );
    assert!(view.read_with(cx, |v, _| v.image.is_some()));
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot_mode),
        SnapshotMode::Debug
    );
    let toggle = cx.debug_bounds("debug-toggle").unwrap();
    cx.simulate_click(toggle.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.debugger_enabled));
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot_mode),
        SnapshotMode::Display
    );
    assert!(cx.debug_bounds("debug-instructions").is_none());
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot.disassembly.capacity()),
        0
    );
    assert!(view.read_with(cx, |v, _| v.image.is_none()));
}

pub(super) fn desktop(window: &mut gpui::Window, cx: &mut gpui::Context<Desktop>) -> Desktop {
    let machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "UI test").unwrap();
    let snapshot = Snapshot::capture_with_mode(&machine, false, None, 0., SnapshotMode::Display);
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    let mut themes = ThemeCatalog::default();
    themes.select("midnight").unwrap();
    Desktop {
        sidebar: None,
        fonts: fonts::Fonts::new(Arc::clone(cx.text_system())),
        language: eidoryn_psx::i18n::Language::ZhCn,
        preferences: eidoryn_psx::preferences::Preferences::default(),
        preferences_path: Some(test_config_path()),
        theme_paths: std::collections::HashMap::new(),
        clean_fullscreen: false,
        settings_focus: cx.focus_handle(),
        runner: Runner::with_snapshot_mode(
            machine,
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        )
        .unwrap(),
        graphics: graphics::GraphicsControls::new(snapshot.renderer, false)
            .with_backends(Vec::new()),
        snapshot,
        image: None,
        icon: OnceCell::new(),
        debugger_enabled: false,
        snapshot_mode: SnapshotMode::Display,
        crt: CrtState::new(eidoryn_psx::video_filter::CrtFilter::Off),
        last_redraw: Instant::now(),
        focus,
        notice: None,
        audio: AudioControls {
            buffer: AudioBuffer::default(),
            output: None,
            volume: 75,
            muted: true,
            dragging: false,
        },
        bios: None,
        game_view: false,
        page: WorkspacePage::Console,
        themes,
        settings_tab: SettingsTab::Graphics,
        pressed_buttons: 0,
        selected_save_state: None,
        save_state_picker_open: false,
        renaming_state: None,
        rewind_sources: 0,
        rebinding: None,
        keybindings_draft: None,
        probe: None,
        _subscriptions: Vec::new(),
    }
}

#[gpui::test]
fn settings_capture_input_and_game_view_returns_to_debug(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    view.update(cx, |view, cx| {
        view.page = WorkspacePage::Debug;
        cx.notify();
    });
    cx.simulate_keystrokes("f6");
    assert!(view.read_with(cx, |view, _| view.game_view));
    cx.simulate_keystrokes("f8");
    assert!(view.read_with(cx, |view, _| view.graphics.open));
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), 0);
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(cx, |view, _| view.graphics.open));
    assert!(view.read_with(cx, |view, _| view.game_view));
    cx.simulate_keystrokes("f6");
    assert!(!view.read_with(cx, |view, _| view.game_view));
    assert_eq!(
        view.read_with(cx, |view, _| view.page),
        WorkspacePage::Debug
    );
    cx.simulate_keystrokes("f7");
    assert_eq!(
        view.read_with(cx, |view, _| view.crt_filter()),
        eidoryn_psx::video_filter::CrtFilter::Soft
    );
}

#[gpui::test]
fn workspace_renders_across_sizes_and_theme_changes(cx: &mut TestAppContext) {
    let window = cx.add_window(desktop);
    let _cleanup = TestConfig(
        window
            .read_with(cx, |v, _| v.preferences_path.clone().unwrap())
            .unwrap(),
    );
    let mut visual = VisualTestContext::from_window(*window, cx);
    for (width, height, rem) in [
        (640., 480., 20.),
        (980., 680., 16.),
        (1280., 900., 16.),
        (1600., 1000., 16.),
    ] {
        visual.simulate_resize(size(px(width), px(height)));
        for theme in ["midnight", "lavender", "ocean", "forest"] {
            for language in [
                eidoryn_psx::i18n::Language::En,
                eidoryn_psx::i18n::Language::ZhCn,
            ] {
                for (page, game, settings, tab) in [
                    (WorkspacePage::Console, false, false, SettingsTab::Graphics),
                    (WorkspacePage::Debug, false, false, SettingsTab::Graphics),
                    (
                        WorkspacePage::SaveStates,
                        false,
                        false,
                        SettingsTab::Graphics,
                    ),
                    (WorkspacePage::Console, true, false, SettingsTab::Graphics),
                    (WorkspacePage::Console, false, true, SettingsTab::Graphics),
                    (WorkspacePage::Console, false, true, SettingsTab::Appearance),
                    (WorkspacePage::Console, false, true, SettingsTab::Controller),
                    (WorkspacePage::Console, false, true, SettingsTab::Rewind),
                ] {
                    window
                        .update(&mut visual, |view, window, cx| {
                            window.set_rem_size(px(rem));
                            view.language = language;
                            view.themes.select(theme).unwrap();
                            view.page = page;
                            view.debugger_enabled = page == WorkspacePage::Debug;
                            if view.debugger_enabled {
                                view.snapshot =
                                    Snapshot::capture(&Machine::demo(), false, None, 0.);
                            }
                            if page == WorkspacePage::SaveStates {
                                view.snapshot.save_states =
                                    ["长游戏名".repeat(10), "Long_Game_Title_".repeat(8)]
                                        .into_iter()
                                        .map(|title| {
                                            eidoryn_psx::systems::ps1::state_file::SaveStateInfo {
                                                path: std::path::PathBuf::from(
                                                    "layout-test.ps1state",
                                                ),
                                                name: format!(
                                                    "{title}_2026-11-15T12-30-10.123456789Z"
                                                ),
                                            }
                                        })
                                        .collect();
                            }
                            view.game_view = game;
                            view.graphics.open = settings;
                            view.settings_tab = tab;
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    if page == WorkspacePage::SaveStates {
                        for selector in ["save-state", "save-state-picker", "load-state"] {
                            let bounds = visual
                                .debug_bounds(selector)
                                .expect("snapshot controls share one page");
                            assert!(bounds.origin.x >= px(0.) && bounds.right() <= px(width));
                            assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
                        }
                    }
                }
            }
        }
    }
}

#[gpui::test]
fn console_cards_fit_and_reflow_across_window_sizes(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    for width in [980., 1250., 1251., 1280., 1600.] {
        cx.simulate_resize(size(px(width), px(1400.)));
        for language in [
            eidoryn_psx::i18n::Language::En,
            eidoryn_psx::i18n::Language::ZhCn,
        ] {
            view.update(cx, |view, cx| {
                view.language = language;
                view.themes.select("midnight").unwrap();
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let [display, stage, transport, session, memory] = [
                "display-card",
                "display-stage",
                "transport",
                "session-card",
                "memory-card",
            ]
            .map(|selector| {
                let bounds = cx
                    .debug_bounds(selector)
                    .expect("console element is visible");
                assert!(
                    bounds.origin.x >= px(0.) && bounds.right() <= px(width),
                    "{selector} overflows the {width}px window in {language:?}: {bounds:?}"
                );
                bounds
            });
            let nav = cx.debug_bounds("nav-console").unwrap();
            assert!(display.origin.x >= nav.right());
            for child in [stage, transport] {
                assert!(child.origin.x >= display.origin.x && child.right() <= display.right());
                assert!(child.origin.y >= display.origin.y && child.bottom() <= display.bottom());
            }
            let volume = cx.debug_bounds("volume-slider").unwrap();
            assert!(volume.origin.x >= transport.origin.x && volume.right() <= transport.right());
            if width <= 1250. {
                assert!(session.origin.y >= display.bottom());
                assert_eq!(session.origin.y, memory.origin.y);
                assert!(session.right() <= memory.origin.x);
            } else {
                assert!(session.origin.x >= display.right());
                assert_eq!(session.origin.x, memory.origin.x);
                assert!(memory.origin.y >= session.bottom());
            }
        }
    }
}

#[gpui::test]
fn debugger_columns_keep_details_readable(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    for width in [980., 1280., 1600.] {
        cx.simulate_resize(size(px(width), px(1400.)));
        for language in [
            eidoryn_psx::i18n::Language::En,
            eidoryn_psx::i18n::Language::ZhCn,
        ] {
            view.update(cx, |view, cx| {
                view.language = language;
                view.page = WorkspacePage::Debug;
                view.debugger_enabled = true;
                view.snapshot = Snapshot::capture(&Machine::demo(), false, None, 0.);
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let [instructions, registers, gpu] =
                ["debug-instructions", "debug-registers", "debug-gpu"].map(|selector| {
                    let bounds = cx.debug_bounds(selector).expect("debugger card is visible");
                    assert!(
                        bounds.origin.x >= px(0.) && bounds.right() <= px(width),
                        "{selector} overflows the {width}px window in {language:?}: {bounds:?}"
                    );
                    bounds
                });
            assert!(
                registers.size.width >= px(250.),
                "registers are too narrow in the {width}px window: {registers:?}"
            );
            assert_eq!(registers.origin.x, gpu.origin.x);
            assert!(gpu.origin.y >= registers.bottom());
            if width < 1000. {
                assert!(registers.origin.y >= instructions.bottom());
            } else {
                assert!(registers.origin.x >= instructions.right());
            }
        }
    }
}

#[gpui::test]
fn keyboard_navigation_switches_language_and_traps_drawer_focus(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let path = view.read_with(cx, |v, _| v.preferences_path.clone().unwrap());
    let _cleanup = TestConfig(path.clone());
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    press_keys(cx, "tab tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Debug);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    press_keys(cx, "escape f8 tab tab tab enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.settings_tab),
        SettingsTab::Appearance
    );
    press_keys(cx, "tab tab tab enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.language),
        eidoryn_psx::i18n::Language::En
    );
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .language
            .as_deref(),
        Some("en")
    );
    for _ in 0..24 {
        press_keys(cx, "tab");
        view.update_in(cx, |v, window, cx| {
            assert!(v.settings_focus.contains_focused(window, cx));
        });
    }
    view.update(cx, |view, cx| view.select_theme("ocean", cx));
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .theme,
        "ocean"
    );
    press_keys(cx, "escape f12");
    assert!(view.read_with(cx, |v, _| v.clean_fullscreen));
    assert!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .clean_fullscreen
    );
    press_keys(cx, "f11");
    assert!(cx.debug_bounds("pure-display").is_some());
    assert!(cx.debug_bounds("game-controls").is_none());
    press_keys(cx, "f12");
    assert!(cx.debug_bounds("game-controls").is_some());
    assert!(cx.debug_bounds("pure-display").is_none());
    press_keys(cx, "escape");
}

#[gpui::test]
fn turbo_settings_persist_and_input_releases_at_ui_boundaries(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(1400.)));
    let path = view.read_with(cx, |v, _| v.preferences_path.clone().unwrap());
    let _cleanup = TestConfig(path.clone());
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x8000);
    press_keys(cx, "f8");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.run_until_parked();
    let tab = cx.debug_bounds("tab-controller").unwrap();
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.settings_tab),
        SettingsTab::Controller
    );
    let square = cx.debug_bounds("turbo-square").unwrap();
    cx.simulate_click(square.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let rate = cx.debug_bounds("turbo-rate-15").unwrap();
    cx.simulate_click(rate.center(), gpui::Modifiers::none());
    let saved = eidoryn_psx::preferences::Preferences::load(&path).unwrap();
    assert_eq!(saved.turbo_buttons, 0x8000);
    assert_eq!(saved.turbo_hz, 15);
    press_keys(cx, "z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    press_keys(cx, "escape");
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x8000);
    view.update(cx, Desktop::stop_input);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    // A key-up after losing focus is harmless; another press/release still works.
    press_keys(cx, "z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    press_keys(cx, "f8 tab tab tab tab enter");
    // Skip fourteen bindings and their three actions to reach the turbo controls.
    press_keys(cx, &"tab ".repeat(20));
    press_keys(cx, "enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.settings_tab),
        SettingsTab::Controller
    );
    assert_eq!(view.read_with(cx, |v, _| v.preferences.turbo_buttons), 0);
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .turbo_buttons,
        0
    );
}

#[gpui::test]
fn f5_resumes_with_live_input_without_space_pausing_the_game(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        // The built-in demo yields after each drawn frame. Loading its EXE
        // instead can rasterize a whole 20,000-instruction slice before the
        // worker can receive the next key command or acknowledgement barrier.
        let mut machine = Machine::demo();
        machine.bus.sio.set_buttons(0x8000);
        view.snapshot.pad_buttons = 0x8000;
        view.runner = Runner::with_snapshot_mode(
            machine,
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        )
        .unwrap();
        view
    });
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    for (index, (keys, running)) in [
        ("space", false),
        ("f5", true),
        ("space", true),
        ("tab f5", false),
        ("escape f5", true),
    ]
    .into_iter()
    .enumerate()
    {
        press_keys(cx, keys);
        // Acknowledge all preceding key commands before checking the worker's state.
        let mode = if index % 2 == 0 {
            SnapshotMode::Debug
        } else {
            SnapshotMode::Display
        };
        view.read_with(cx, |v, _| {
            v.runner.send(Command::SetSnapshotMode(mode)).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(snapshot) = view.read_with(cx, |v, _| v.runner.take_snapshot())
                && snapshot.mode == mode
            {
                assert_eq!(snapshot.running, running, "after {keys}");
                if index > 0 {
                    assert_eq!(snapshot.pad_buttons, 0, "restored buttons remain stuck");
                }
                view.update(cx, |v, _| v.snapshot.running = snapshot.running);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not acknowledge {keys}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[gpui::test]
fn custom_bindings_persist_drive_input_and_reset(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let path = view.read_with(cx, |v, _| v.preferences_path.clone().unwrap());
    let _cleanup = TestConfig(path.clone());
    cx.simulate_resize(size(px(1280.), px(1000.)));
    cx.simulate_keystrokes("z");
    press_keys(cx, "f8 tab tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.run_until_parked();
    let square = cx.debug_bounds("binding-15").unwrap();
    cx.simulate_click(square.center(), gpui::Modifiers::none());
    assert_eq!(
        view.read_with(cx, |v, _| v.rebinding),
        Some(PadButton::Square)
    );
    press_keys(cx, "space");
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), None);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .keybindings,
        KeyBindings::default()
    );
    cx.run_until_parked();
    let apply = cx.debug_bounds("bindings-apply").unwrap();
    cx.simulate_click(apply.center(), gpui::Modifiers::none());
    let saved = eidoryn_psx::preferences::Preferences::load(&path).unwrap();
    assert_eq!(saved.keybindings.key(PadButton::Square), "space");
    press_keys(cx, "escape");
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.simulate_keystrokes("space up");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x8010);
    // Releasing a held key still works if modifiers changed in the meantime.
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("ctrl-space").unwrap(),
    });
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x0010);
    press_keys(cx, "f8 tab tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.run_until_parked();
    let reset = cx.debug_bounds("bindings-reset").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::none());
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .keybindings,
        saved.keybindings
    );
    cx.run_until_parked();
    let apply = cx.debug_bounds("bindings-apply").unwrap();
    cx.simulate_click(apply.center(), gpui::Modifiers::none());
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .keybindings,
        KeyBindings::default()
    );
    press_keys(cx, "escape");
    cx.simulate_keystrokes("space");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x8000);
    view.update(cx, Desktop::stop_input);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
}

#[gpui::test]
fn binding_capture_handles_cancel_and_keyboard_activation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _subscriptions = view.update_in(cx, |view, window, cx| {
        let subscriptions = [
            cx.on_blur(&view.focus, window, |view, _, cx| {
                view.stop_input(cx);
            }),
            cx.observe_window_activation(window, |view, window, cx| {
                if !window.is_window_active() {
                    view.stop_input(cx);
                }
            }),
        ];
        window.activate_window();
        subscriptions
    });
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(1000.)));
    press_keys(cx, "f8 tab tab tab tab enter tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), Some(PadButton::Up));
    for key in ["f5", "tab", "f8", "f11", "f12", "ctrl-r", "shift-j"] {
        press_keys(cx, key);
        assert_eq!(view.read_with(cx, |v, _| v.rebinding), Some(PadButton::Up));
        assert!(view.read_with(cx, |v, _| v.graphics.open));
        assert!(!view.read_with(cx, |v, _| v.clean_fullscreen));
    }
    press_keys(cx, "escape");
    assert!(view.read_with(cx, |v, _| v.graphics.open));
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), None);
    assert!(view.read_with(cx, |v, _| v.preferences.keybindings
        == KeyBindings::default()));

    // Free Enter, then bind it using the key-up that also activates focused UI controls.
    for (selector, key) in [("binding-3", "j"), ("binding-14", "enter")] {
        cx.run_until_parked();
        let binding = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(binding.center(), gpui::Modifiers::none());
        press_keys(cx, key);
        assert_eq!(view.read_with(cx, |v, _| v.rebinding), None);
        assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    }
    cx.run_until_parked();
    let apply = cx.debug_bounds("bindings-apply").unwrap();
    cx.simulate_click(apply.center(), gpui::Modifiers::none());
    assert_eq!(
        view.read_with(cx, |v, _| v
            .preferences
            .keybindings
            .key(PadButton::Cross)
            .to_owned()),
        "enter"
    );
    let up = cx.debug_bounds("binding-4").unwrap();
    cx.simulate_click(up.center(), gpui::Modifiers::none());
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("w").unwrap(),
        is_held: true,
        prefer_character_input: false,
    });
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), Some(PadButton::Up));
    cx.deactivate_window();
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), None);
    cx.update(|window, _| window.activate_window());
    press_keys(cx, "escape");
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x4000);
    press_keys(cx, "enter");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
}

#[gpui::test]
fn duplicate_bindings_remain_drafts_until_confirmed(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let path = view.read_with(cx, |v, _| v.preferences_path.clone().unwrap());
    let _cleanup = TestConfig(path.clone());
    cx.simulate_resize(size(px(1280.), px(1400.)));
    press_keys(cx, "f8 tab tab tab tab enter");
    cx.run_until_parked();
    let cross = cx.debug_bounds("binding-14").unwrap();
    cx.simulate_click(cross.center(), gpui::Modifiers::none());
    press_keys(cx, "z");
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), None);
    assert!(view.read_with(
        cx,
        |v, _| v.keybindings_draft.as_ref().unwrap().key(PadButton::Cross) == "z"
    ));
    assert!(cx.debug_bounds("bindings-conflict").is_some());

    // Saving another setting must not write unconfirmed or conflicting bindings.
    let turbo = cx.debug_bounds("turbo-square").unwrap();
    cx.simulate_click(turbo.center(), gpui::Modifiers::none());
    let before = std::fs::read(&path).unwrap();
    let apply = cx.debug_bounds("bindings-apply").unwrap();
    cx.simulate_click(apply.center(), gpui::Modifiers::none());
    assert!(view.read_with(cx, |v, _| v.notice.is_some()));
    assert!(view.read_with(cx, |v, _| v.preferences.keybindings
        == KeyBindings::default()));
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // Complete the swap without assigning either button to an intermediate key.
    cx.run_until_parked();
    let square = cx.debug_bounds("binding-15").unwrap();
    cx.simulate_click(square.center(), gpui::Modifiers::none());
    press_keys(cx, "x");
    cx.run_until_parked();
    let apply = cx.debug_bounds("bindings-apply").unwrap();
    cx.simulate_click(apply.center(), gpui::Modifiers::none());
    assert!(view.read_with(cx, |v, _| v.notice.is_none()
        && v.keybindings_draft.is_none()));
    let saved = eidoryn_psx::preferences::Preferences::load(&path).unwrap();
    assert_eq!(saved.keybindings.key(PadButton::Cross), "z");
    assert_eq!(saved.keybindings.key(PadButton::Square), "x");
    assert_eq!(saved.turbo_buttons, 0x8000);
    press_keys(cx, "escape");
    cx.simulate_keystrokes("x");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0x8000);
    press_keys(cx, "x z");
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);

    // Reset and discard also leave the last confirmed map intact.
    press_keys(cx, "f8 tab tab tab tab enter");
    cx.run_until_parked();
    let reset = cx.debug_bounds("bindings-reset").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::none());
    let cancel = cx.debug_bounds("bindings-cancel").unwrap();
    cx.simulate_click(cancel.center(), gpui::Modifiers::none());
    assert!(view.read_with(cx, |v, _| v.keybindings_draft.is_none()));
    cx.simulate_click(reset.center(), gpui::Modifiers::none());
    press_keys(cx, "escape");
    assert!(view.read_with(cx, |v, _| v.keybindings_draft.is_none()));
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path).unwrap(),
        saved
    );
}

fn test_config_path() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "eidoryn-ui-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}
struct TestConfig(std::path::PathBuf);
impl Drop for TestConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(super) fn press_keys(cx: &mut VisualTestContext, keys: &str) {
    let os = cx.read(|cx| cx.platform_info().os);
    for key in keys.split_whitespace() {
        cx.simulate_keystrokes(key);
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse_with_os(key, os).unwrap(),
        });
    }
}

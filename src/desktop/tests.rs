use super::*;
use eidoryn_psx::keybindings::{KeyBindings, PadButton};
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn video_updates_reuse_sidebar_but_ui_changes_invalidate_it(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    let sidebar = view.read_with(cx, |view, _| view.sidebar.clone().unwrap());
    let initial = sidebar.read_with(cx, |sidebar, _| sidebar.renders);
    assert!(initial > 0);
    for _ in 0..5 {
        view.update(cx, |view, cx| {
            view.snapshot.video_frames += 1;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
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
        cx.update(|window, cx| window.draw(cx).clear());
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
    // GPUI 0.2.2 retains historical debug_bounds entries after controls disappear.
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot.disassembly.capacity()),
        0
    );
    assert!(view.read_with(cx, |v, _| v.image.is_none()));
}

fn desktop(window: &mut gpui::Window, cx: &mut gpui::Context<Desktop>) -> Desktop {
    let machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "UI test").unwrap();
    let snapshot = Snapshot::capture_with_mode(&machine, false, None, 0., SnapshotMode::Display);
    let focus = cx.focus_handle();
    window.focus(&focus);
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
        graphics: graphics::GraphicsControls::new(snapshot.renderer, false),
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
                    (WorkspacePage::Console, true, false, SettingsTab::Graphics),
                    (WorkspacePage::Console, false, true, SettingsTab::Graphics),
                    (WorkspacePage::Console, false, true, SettingsTab::Appearance),
                    (WorkspacePage::Console, false, true, SettingsTab::Controller),
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
                            view.game_view = game;
                            view.graphics.open = settings;
                            view.settings_tab = tab;
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
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
            cx.update(|window, cx| window.draw(cx).clear());
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
            cx.update(|window, cx| window.draw(cx).clear());
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
    press_keys(cx, "tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Debug);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    press_keys(cx, "escape f8 tab tab tab enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.settings_tab),
        SettingsTab::Appearance
    );
    press_keys(cx, "tab tab enter");
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
    press_keys(cx, &"tab ".repeat(19));
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
fn f5_toggles_execution_without_space_pausing_the_game(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
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
    press_keys(cx, "f8 tab tab tab tab enter tab enter");
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

fn press_keys(cx: &mut VisualTestContext, keys: &str) {
    for key in keys.split_whitespace() {
        cx.simulate_keystrokes(key);
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
        });
    }
}

use super::*;
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
        appearance_open: false,
        pressed_buttons: 0,
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
                for (page, game, settings, appearance) in [
                    (WorkspacePage::Console, false, false, false),
                    (WorkspacePage::Debug, false, false, false),
                    (WorkspacePage::Console, true, false, false),
                    (WorkspacePage::Console, false, true, false),
                    (WorkspacePage::Console, false, true, true),
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
                            view.appearance_open = appearance;
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
    assert!(view.read_with(cx, |v, _| v.appearance_open));
    press_keys(cx, "tab enter");
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

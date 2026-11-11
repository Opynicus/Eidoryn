use super::*;
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn sidebar_clicks_navigate_between_workspaces(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
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

fn desktop(window: &mut gpui::Window, cx: &mut gpui::Context<Desktop>) -> Desktop {
    let machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "UI test").unwrap();
    let snapshot = Snapshot::capture(&machine, false, None, 0.);
    let image = frame_image(&snapshot);
    let focus = cx.focus_handle();
    window.focus(&focus);
    let mut themes = ThemeCatalog::default();
    themes.select("midnight").unwrap();
    Desktop {
        runner: Runner::new(machine, false).unwrap(),
        graphics: graphics::GraphicsControls::new(snapshot.renderer, false),
        snapshot,
        image,
        icon: Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../assets/eidoryn.png").to_vec(),
        )),
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
    let mut visual = VisualTestContext::from_window(*window, cx);
    for (width, height) in [(980., 680.), (1280., 900.), (1600., 1000.)] {
        visual.simulate_resize(size(px(width), px(height)));
        for theme in ["midnight", "lavender", "ocean", "forest"] {
            window
                .update(&mut visual, |view, _, cx| {
                    view.themes.select(theme).unwrap();
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
        }
    }
}

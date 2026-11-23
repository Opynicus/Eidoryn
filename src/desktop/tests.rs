use super::*;
use eidoryn_psx::i18n::Language;
use eidoryn_psx::keybindings::{KeyBindings, PadButton};
use gpui::{TestAppContext, VisualTestContext};

#[gpui::test]
fn default_workspace_paints_the_prototype_colors(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    let inactive_nav = cx.debug_bounds("nav-debug").unwrap().center();
    let active_nav = cx.debug_bounds("nav-console").unwrap().center();
    let open_disc = cx.debug_bounds("open-disc").unwrap().center();
    cx.update(|window, _| {
        let quads = window.painted_quads();
        for (position, expected) in [
            (inactive_nav, 0x0015_191e),
            (active_nav, 0x0029_332f),
            (open_disc, 0x00b7_e4c8),
        ] {
            let point = position.scale(window.scale_factor());
            let top_quad = quads
                .iter()
                .filter(|quad| {
                    !quad.background.is_transparent()
                        && quad
                            .bounds
                            .intersect(&quad.content_mask.bounds)
                            .contains(&point)
                })
                .max_by_key(|quad| quad.order)
                .expect("visible control has a painted background");
            assert_eq!(top_quad.background, gpui::rgb(expected).into());
        }
        for (role, expected) in [
            ("workspace", 0x0010_1317),
            ("secondary button", 0x0028_3138),
            ("paused indicator", 0x00e6_bf79),
        ] {
            assert!(
                quads
                    .iter()
                    .any(|quad| quad.background == gpui::rgb(expected).into()),
                "{role} should use #{expected:06x}"
            );
        }
    });
}

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
                0 => view.language = Language::En,
                1 => view.themes.select("ocean").unwrap(),
                2 => view.page = WorkspacePage::Debug,
                _ => view.page = WorkspacePage::Game,
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
fn sidebar_paints_the_project_icon_at_both_widths(cx: &mut TestAppContext) {
    let artwork = super::icons::app_icon().expect("bundled project icon decodes");
    for width in [1280., 980.] {
        let (_view, visual) = cx.add_window_view(desktop);
        visual.simulate_resize(size(px(width), px(900.)));
        visual.run_until_parked();
        let bounds = visual
            .debug_bounds("sidebar-app-icon")
            .expect("the project icon is visible in the sidebar");
        assert!(bounds.size.width > px(0.));
        assert_eq!(bounds.size.width, bounds.size.height);
        assert!(
            visual.update(|window, _| window.has_image_atlas_entry(&artwork)),
            "project artwork should be painted at window width {width}"
        );
    }
}

#[gpui::test]
fn fullscreen_controls_return_to_the_previous_workspace(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();

    // Enter from the console, then move keyboard focus off the root and into the overlay.
    click_control(cx, "enter-fullscreen");
    assert!(cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("fullscreen-controls").is_none());
    press_keys(cx, "f12");
    assert!(cx.debug_bounds("fullscreen-controls").is_some());
    assert!(cx.debug_bounds("nav-console").is_none());
    press_keys(cx, "tab");
    assert!(view.update_in(cx, |v, window, _| !v.focus.is_focused(window)));
    press_keys(cx, "escape");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("launch-card").is_some());

    click_control(cx, "nav-game");
    click_control(cx, "enter-fullscreen");
    assert!(cx.update(|window, _| window.is_fullscreen()));
    click_control(cx, "exit-fullscreen");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
    press_keys(cx, "f11");
    assert!(cx.update(|window, _| window.is_fullscreen()));
    press_keys(cx, "f11");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
}

#[gpui::test]
fn enlarge_opens_the_fullscreen_game_workspace_and_restores_the_source_page(
    cx: &mut TestAppContext,
) {
    let (_view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1568.), px(980.)));
    cx.run_until_parked();

    for (navigation, page) in [
        ("nav-console", "launch-card"),
        ("nav-game", "game-controls"),
    ] {
        click_control(cx, navigation);
        let original = cx.debug_bounds("display-stage").unwrap();
        click_control(cx, "enlarge-game");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        assert!(cx.debug_bounds("game-controls").is_some());
        assert!(cx.debug_bounds("launch-card").is_none());
        let stage = cx.debug_bounds("display-stage").unwrap();
        assert!(cx.debug_bounds("nav-game").unwrap().right() <= stage.left());
        assert!(cx.debug_bounds("transport-run").unwrap().top() >= stage.bottom());
        let expand = cx.debug_bounds("enter-fullscreen").unwrap();
        assert!(expand.top() >= stage.bottom());
        assert!(
            expand.bottom() <= px(980.),
            "fullscreen button outside viewport: {expand:?} below {stage:?}"
        );
        assert!(cx.debug_bounds("rewind-hold").unwrap().left() >= stage.right());
        assert!(cx.debug_bounds("memory-card").unwrap().left() >= stage.right());

        click_control(cx, "enter-fullscreen");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        assert!(
            cx.debug_bounds("fullscreen-display").is_some(),
            "fullscreen button at {expand:?} from {navigation}"
        );
        assert!(cx.debug_bounds("game-controls").is_none());
        assert!(cx.debug_bounds("nav-game").is_none());
        press_keys(cx, "escape");
        assert!(cx.update(|window, _| window.is_fullscreen()));
        assert_eq!(cx.debug_bounds("display-stage"), Some(stage));
        assert!(cx.debug_bounds("game-controls").is_some());
        press_keys(cx, "escape");
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        assert!(cx.debug_bounds(page).is_some());
        assert_eq!(cx.debug_bounds("display-stage"), Some(original));
    }

    press_keys(cx, "f6");
    click_control(cx, "manage-states");
    click_control(cx, "manage-all-snapshots");
    assert!(cx.debug_bounds("save-states-page").is_some());
    click_control(cx, "nav-debug");
    click_control(cx, "debug-back");
    assert!(cx.debug_bounds("game-controls").is_some());
    assert!(cx.update(|window, _| window.is_fullscreen()));
    press_keys(cx, "f11 f11");
    assert!(cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
    press_keys(cx, "f6");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
    click_control(cx, "enlarge-game");
    click_control(cx, "restore-game");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
    press_keys(cx, "f6 tab escape");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
}

#[gpui::test]
fn startup_views_return_through_the_requested_fullscreen_layers(cx: &mut TestAppContext) {
    for (game_view, fullscreen) in [(true, false), (false, true), (true, true)] {
        let (_view, cx) = cx.add_window_view(move |window, cx| {
            let mut view = desktop(window, cx);
            view.presentation = Presentation::at_startup(&Options {
                fullscreen,
                game_view,
                ..Options::default()
            });
            window.toggle_fullscreen();
            view
        });
        cx.run_until_parked();
        if fullscreen {
            assert!(cx.debug_bounds("fullscreen-display").is_some());
            assert!(cx.debug_bounds("game-controls").is_none());
            press_keys(cx, "escape");
        }
        if game_view {
            assert!(cx.update(|window, _| window.is_fullscreen()));
            assert!(cx.debug_bounds("game-controls").is_some());
            press_keys(cx, "escape");
        }
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        assert!(cx.debug_bounds("launch-card").is_some());
    }
}

#[gpui::test]
fn leaving_fullscreen_through_the_window_manager_restores_the_workspace(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(desktop);
    for keys in ["f6", "f11", "f6 f11"] {
        cx.simulate_resize(size(px(1280.), px(900.)));
        press_keys(cx, keys);
        assert!(cx.update(|window, _| window.is_fullscreen()));

        cx.update(|window, _| window.toggle_fullscreen());
        cx.simulate_resize(size(px(980.), px(680.)));
        cx.run_until_parked();
        assert!(!cx.update(|window, _| window.is_fullscreen()));
        assert!(cx.debug_bounds("launch-card").is_some(), "after {keys}");
        assert!(cx.debug_bounds("fullscreen-display").is_none());
    }
}

#[gpui::test]
fn fullscreen_video_fills_the_screen_even_when_a_notice_is_visible(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    let viewport = Bounds::new(gpui::point(px(0.), px(0.)), size(px(1280.), px(720.)));
    cx.simulate_resize(viewport.size);
    press_keys(cx, "f11");
    view.update(cx, |view, cx| {
        view.notice = Some("Disc read error".into());
        cx.notify();
    });
    cx.run_until_parked();
    let notice = cx.debug_bounds("error-notice").unwrap();
    assert_eq!(notice.top(), viewport.top());
    assert!(notice.size.height > px(0.));
    assert_eq!(cx.debug_bounds("fullscreen-display"), Some(viewport));
    assert!(cx.update(|window, _| window.is_fullscreen()));
}

#[gpui::test]
fn enlarged_workspace_and_fullscreen_share_the_frame_at_every_internal_resolution(
    cx: &mut TestAppContext,
) {
    use eidoryn_psx::gpu::RenderScale;
    use gpui::point;

    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    press_keys(cx, "f6");
    for (width, height, x, y, video_width, video_height) in [
        (960., 720., 0., 0., 960., 720.),
        (1280., 720., 160., 0., 960., 720.),
        (600., 900., 0., 225., 600., 450.),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
        let picture = Bounds::new(point(px(x), px(y)), size(px(video_width), px(video_height)));
        let workspace_stage = cx.debug_bounds("display-stage").unwrap();
        assert!(workspace_stage.size.width < viewport.size.width);
        assert!(workspace_stage.size.height < viewport.size.height);
        for scale in [RenderScale::Native, RenderScale::X2, RenderScale::X4] {
            let factor = scale.factor();
            let dimensions = (256 * factor, 240 * factor);
            view.update(cx, |view, cx| {
                // Adjacent samples within one guest pixel may differ at 2× and 4×.
                let mut rgb = vec![0; dimensions.0 * dimensions.1 * 3];
                rgb[..3].copy_from_slice(&[255, 0, 0]);
                rgb[3..6].copy_from_slice(&[0, 0, 255]);
                view.snapshot.native_dimensions = (256, 240);
                view.snapshot.dimensions = dimensions;
                view.snapshot.rgb = rgb.into();
                view.snapshot.render_scale = scale;
                if let Some(image) = &mut view.image {
                    image.dirty = true;
                }
                cx.notify();
            });
            cx.run_until_parked();
            assert_eq!(cx.debug_bounds("display-stage"), Some(workspace_stage));
            let image = view.update_in(cx, |view, window, _| {
                let image = &view.image.as_ref().unwrap().image;
                assert!(window.has_image_atlas_entry(image));
                assert_eq!(
                    image.size(0),
                    size(dimensions.0.into(), dimensions.1.into())
                );
                assert_eq!(
                    &image.as_bytes(0).unwrap()[..8],
                    &[0, 0, 255, 255, 255, 0, 0, 255]
                );
                Arc::clone(image)
            });
            press_keys(cx, "f11");
            for controls_visible in [false, true] {
                cx.run_until_parked();
                let stage = cx.debug_bounds("fullscreen-display").unwrap();
                assert_eq!(stage, viewport);
                assert_eq!(
                    super::video::fit_video(stage, 4.0 / 3.0),
                    picture,
                    "{scale:?} in {width} × {height}"
                );
                assert_eq!(
                    cx.debug_bounds("fullscreen-controls").is_some(),
                    controls_visible
                );
                view.update_in(cx, |view, window, _| {
                    assert!(Arc::ptr_eq(&image, &view.image.as_ref().unwrap().image));
                    assert!(window.has_image_atlas_entry(&image));
                });
                press_keys(cx, "f12");
            }
            press_keys(cx, "escape");
            assert!(cx.update(|window, _| window.is_fullscreen()));
            assert_eq!(cx.debug_bounds("display-stage"), Some(workspace_stage));
        }
    }
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
fn sidebar_drag_and_toggle_preserve_game_context(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    let modifiers = gpui::Modifiers::none();
    let game = cx.debug_bounds("nav-game").unwrap();
    cx.simulate_click(game.center(), modifiers);
    cx.run_until_parked();
    assert!(cx.debug_bounds("game-controls").is_some());
    let handle = cx.debug_bounds("sidebar-resize").unwrap().center();
    let compact = gpui::point(px(82.), handle.y);
    cx.simulate_mouse_down(handle, gpui::MouseButton::Left, modifiers);
    cx.run_until_parked();
    cx.simulate_mouse_move(compact, gpui::MouseButton::Left, modifiers);
    cx.simulate_mouse_up(compact, gpui::MouseButton::Left, modifiers);
    cx.run_until_parked();
    let width = cx.debug_bounds("nav-game").unwrap().size.width;
    assert!(width < game.size.width);
    for visible in [false, true] {
        let toggle = cx.debug_bounds("toggle-sidebar").unwrap();
        cx.simulate_click(toggle.center(), modifiers);
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("nav-game").is_some(), visible);
        assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Game);
    }
    assert_eq!(cx.debug_bounds("nav-game").unwrap().size.width, width);
}

#[gpui::test]
fn ctrl_l_does_not_leave_the_game_or_release_input(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.run_until_parked();
    press_keys(cx, "f6");
    cx.simulate_keystrokes("z");
    assert!(cx.debug_bounds("game-controls").is_some());
    let pressed = view.read_with(cx, |view, _| view.pressed_buttons);
    assert_ne!(pressed, 0);
    cx.simulate_keystrokes("ctrl-l");
    cx.run_until_parked();
    assert!(cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("game-controls").is_some());
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), pressed);
    assert!(cx.debug_bounds("save-states-page").is_none());
}

#[gpui::test]
fn snapshot_rows_load_the_chosen_state(cx: &mut TestAppContext) {
    use eidoryn_psx::systems::ps1::{input::PadInput, state_file::StateStore};
    let root = test_config_path().with_extension("states");
    let mut machine = Machine::from_exe(&eidoryn_psx::demo::executable(), "UI test").unwrap();
    let store = StateStore::new(&root, &machine);
    machine.cpu.registers[5] = 17;
    let older = store.save(&machine, &PadInput::default()).unwrap();
    machine.cpu.registers[5] = 29;
    store.save(&machine, &PadInput::default()).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1280.), px(900.)));
    view.update(cx, |view, cx| {
        view.page = WorkspacePage::SaveStates;
        view.snapshot.save_states = store.list().unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    let load = cx.debug_bounds("load-snapshot-1").unwrap();
    cx.simulate_click(load.center(), gpui::Modifiers::none());
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_save_state.clone()),
        Some(older)
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(snapshot) = view.read_with(cx, |v, _| v.runner.take_snapshot())
            && snapshot.success_message.is_some()
        {
            assert_eq!(snapshot.cpu.registers[5], 17);
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
        view.page = WorkspacePage::SaveStates;
        view.snapshot.success_message = Some("Saved state".into());
        view.snapshot.save_states = vec![SaveStateInfo {
            modified: std::time::UNIX_EPOCH,
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
        view.snapshot.title = "Game B".into();
        view.snapshot.save_states = vec![SaveStateInfo {
            modified: std::time::UNIX_EPOCH,
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
    view.update(cx, |view, cx| {
        view.snapshot.save_states.clear();
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.selected_save_state.is_none()));
    assert!(cx.debug_bounds("load-snapshot-0").is_none());
    assert!(cx.debug_bounds("rename-snapshot-0").is_none());
    assert!(cx.debug_bounds("delete-snapshot-0").is_none());
}

#[gpui::test]
fn selecting_a_bios_is_session_only_until_set_as_default(cx: &mut TestAppContext) {
    use eidoryn_psx::preferences::Preferences;
    let bios = TestConfig(test_config_path().with_extension("bin"));
    let second_dir = test_config_path().with_extension("dir");
    std::fs::create_dir(&second_dir).unwrap();
    let second = second_dir.join(bios.0.file_name().unwrap());
    for path in [&bios.0, &second] {
        std::fs::write(path, vec![0; eidoryn_psx::bus::BIOS_SIZE]).unwrap();
    }
    let first = bios.0.canonicalize().unwrap();
    let second = second.canonicalize().unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let config = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.simulate_resize(size(px(1400.), px(1000.)));
    view.update(cx, |view, cx| {
        view.save_preferences();
        view.command(Command::Toggle, cx);
    });
    wait_for_snapshot(&view, cx, |snapshot| snapshot.running);
    let title = view.read_with(cx, |v, _| v.snapshot.title.clone());
    cx.run_until_parked();
    let click = |selector, cx: &mut VisualTestContext| {
        let control = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(control.center(), gpui::Modifiers::none());
    };
    click("open-bios", cx);
    cx.simulate_path_prompt_response(|_| Some(vec![bios.0.clone()]));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.bios.clone()),
        Some(first.clone())
    );
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
    click("bios-details", cx);
    click("set-default-bios", cx);
    assert_eq!(
        Preferences::load(&config.0).unwrap().bios_path,
        Some(first.clone())
    );

    click("open-bios", cx);
    cx.simulate_path_prompt_response(|_| Some(vec![test_config_path()]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.notice.is_some()
        && v.bios.as_ref() == Some(&first)));

    // Supersede a pending picker. Its later response must not replace the current selection.
    click("open-bios", cx);
    click("open-bios", cx);
    cx.simulate_path_prompt_response(|_| Some(vec![second.clone()]));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.bios.clone()),
        Some(first.clone())
    );
    cx.simulate_path_prompt_response(|_| Some(vec![second.clone()]));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.notice.is_none()
        && v.bios.as_ref() == Some(&second)));
    assert_eq!(
        Preferences::load(&config.0).unwrap().bios_path,
        Some(first.clone())
    );
    click("set-default-bios", cx);
    assert_eq!(
        Preferences::load(&config.0).unwrap().bios_path,
        Some(second.clone())
    );
    click("clear-default-bios", cx);
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());
    assert_eq!(
        view.read_with(cx, |v, _| v.bios.clone()),
        Some(second.clone())
    );

    // Wait for a new worker snapshot after all picker responses.
    view.update(cx, |v, cx| v.command(Command::SetRewindSeconds(45), cx));
    wait_for_snapshot(&view, cx, |snapshot| snapshot.rewind_max_seconds == 45);
    view.read_with(cx, |v, _| {
        assert!(v.snapshot.running);
        assert_eq!(v.snapshot.media_revision, 0);
        assert_eq!(v.snapshot.title, title);
    });
    assert!(second.is_file());
    std::fs::remove_dir_all(second_dir).unwrap();
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
fn recent_games_open_the_game_workspace_and_only_reorder_after_success(cx: &mut TestAppContext) {
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
    cx.run_until_parked();
    let open_in_picker = |disc: &TestConfig, cx: &mut VisualTestContext| {
        click_control(cx, "open-disc");
        cx.simulate_path_prompt_response(|_| Some(vec![disc.0.clone()]));
        cx.run_until_parked();
    };
    for (index, disc) in discs.iter().enumerate() {
        let before = Preferences::load(&config.0).unwrap().recent_discs;
        open_in_picker(disc, cx);
        assert_eq!(Preferences::load(&config.0).unwrap().recent_discs, before);
        wait_for_snapshot(&view, cx, |snapshot| {
            snapshot.media_revision == index as u64 + 1
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("windowed-display").is_some());
        press_keys(cx, "escape");
        let saved = Preferences::load(&config.0).unwrap();
        assert_eq!(saved.recent_discs.len(), index + 1);
        assert_eq!(
            saved.recent_discs[0].path,
            std::fs::canonicalize(&disc.0).unwrap()
        );
        assert!(!saved.recent_discs[0].fast_boot);
    }
    for (selector, revision) in [("recent-game-1", 3), ("recent-game-0", 4)] {
        click_control(cx, selector);
        wait_for_snapshot(&view, cx, |snapshot| snapshot.media_revision == revision);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Game);
        assert!(cx.debug_bounds("game-controls").is_some());
        assert!(!cx.update(|window, _| window.is_fullscreen()));
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
        click_control(cx, "nav-console");
    }
    let before = Preferences::load(&config.0).unwrap().recent_discs;
    std::fs::write(&discs[1].0, b"invalid image").unwrap();
    click_control(cx, "recent-game-1");
    wait_for_snapshot(&view, cx, |snapshot| snapshot.message.is_some());
    cx.run_until_parked();
    assert!(cx.debug_bounds("launch-card").is_some());
    assert_eq!(view.read_with(cx, |v, _| v.snapshot.media_revision), 4);
    assert_eq!(Preferences::load(&config.0).unwrap().recent_discs, before);
    assert!(Preferences::load(&config.0).unwrap().bios_path.is_none());

    // A picker retry of the same image uses the picker's navigation behavior.
    std::fs::write(&discs[1].0, vec![0; 2048]).unwrap();
    open_in_picker(&discs[1], cx);
    wait_for_snapshot(&view, cx, |snapshot| snapshot.media_revision == 5);
    cx.run_until_parked();
    let windowed = cx.debug_bounds("windowed-display").unwrap();
    assert_eq!(windowed.size, cx.update(|window, _| window.viewport_size()));
    assert!(cx.debug_bounds("nav-game").is_none());
    assert!(cx.debug_bounds("transport-run").is_none());
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    press_keys(cx, "f11");
    assert!(cx.update(|window, _| window.is_fullscreen()));
    assert!(cx.debug_bounds("fullscreen-display").is_some());
    press_keys(cx, "escape");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    assert_eq!(cx.debug_bounds("windowed-display"), Some(windowed));
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

pub(super) fn wait_for_snapshot(
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
    let preferences = eidoryn_psx::preferences::Preferences::default();
    Desktop {
        gba: super::gba::Session::default(),
        library: library::Workspace::new(None),
        sidebar: None,
        navigation: sidebar::Navigation::default(),
        bios_details_open: false,
        image_picker: None,
        memory_details_open: false,
        fonts: fonts::Fonts::new(Arc::clone(cx.text_system())),
        language: Language::ZhCn,
        clean_fullscreen: preferences.clean_fullscreen,
        preferences,
        preferences_path: Some(test_config_path()),
        theme_paths: std::collections::HashMap::new(),
        settings_focus: cx.focus_handle(),
        library_dialog_focus: super::library_ui::DialogFocus::new(cx.focus_handle()),
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
        presentation: Presentation::default(),
        page: WorkspacePage::Console,
        themes: ThemeCatalog::default(),
        settings_tab: SettingsTab::Graphics,
        closed_settings_groups: std::collections::HashSet::new(),
        pressed_buttons: 0,
        selected_save_state: None,
        snapshot_menu: None,
        snapshot_confirmation: None,
        renaming_state: None,
        rewind_sources: 0,
        rewind_disable_origin: None,
        rebinding: None,
        keybindings_draft: None,
        shortcut_editor: shortcuts::ShortcutEditor::default(),
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
    press_keys(cx, "f6");
    assert!(cx.debug_bounds("game-controls").is_some());
    assert!(cx.update(|window, _| window.is_fullscreen()));
    cx.simulate_keystrokes("f8");
    assert!(view.read_with(cx, |view, _| view.graphics.open));
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), 0);
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(cx, |view, _| view.graphics.open));
    assert!(cx.debug_bounds("game-controls").is_some());
    cx.simulate_keystrokes("f6");
    assert!(!cx.update(|window, _| window.is_fullscreen()));
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
        for theme in ["midnight", "lavender", "ocean", "forest", "lemon-green"] {
            for &language in Language::ALL {
                for (page, settings, tab) in [
                    (WorkspacePage::Console, false, SettingsTab::Graphics),
                    (WorkspacePage::Game, false, SettingsTab::Graphics),
                    (WorkspacePage::Debug, false, SettingsTab::Graphics),
                    (WorkspacePage::SaveStates, false, SettingsTab::Graphics),
                    (WorkspacePage::Console, true, SettingsTab::Graphics),
                    (WorkspacePage::Console, true, SettingsTab::Appearance),
                    (WorkspacePage::Console, true, SettingsTab::Controller),
                    (WorkspacePage::Console, true, SettingsTab::Rewind),
                    (WorkspacePage::Console, true, SettingsTab::Shortcuts),
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
                                                modified: std::time::UNIX_EPOCH,
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
                            view.graphics.open = settings;
                            view.settings_tab = tab;
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    if page == WorkspacePage::SaveStates {
                        for selector in [
                            "save-state",
                            "load-snapshot-0",
                            "rename-snapshot-0",
                            "delete-snapshot-0",
                        ] {
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
    view.update(cx, |view, _| {
        view.bios = Some(format!("firmware/{}.bin", "long-name-".repeat(20)).into());
    });
    for (width, height) in [(980., 680.), (1040., 760.), (1280., 900.), (1600., 1000.)] {
        cx.simulate_resize(size(px(width), px(height)));
        for &language in Language::ALL {
            view.update(cx, |view, cx| {
                view.language = language;
                view.themes.select("midnight").unwrap();
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let [launch, bios, disc, session, stage, play] = [
                "launch-card",
                "selected-bios",
                "open-disc",
                "session-card",
                "display-stage",
                "transport-run",
            ]
            .map(|selector| {
                let bounds = cx
                    .debug_bounds(selector)
                    .expect("startup control is rendered");
                assert!(
                    bounds.origin.x >= px(0.)
                        && bounds.right() <= px(width)
                        && bounds.origin.y >= px(0.)
                        && bounds.bottom() <= px(height),
                    "{selector} overflows {width} × {height} in {language:?}: {bounds:?}"
                );
                bounds
            });
            assert!(launch.right() <= session.left());
            for control in [bios, disc] {
                assert!(
                    launch.contains(&control.origin) && launch.contains(&control.bottom_right())
                );
            }
            for control in [stage, play] {
                assert!(
                    session.contains(&control.origin) && session.contains(&control.bottom_right())
                );
            }
        }
    }
}

#[gpui::test]
fn standby_marks_only_the_unstarted_builtin_demo(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    let mut demo = Machine::demo();
    let same_title = Machine::from_exe(&eidoryn_psx::demo::executable(), &demo.title).unwrap();
    for (machine, running, standby) in [
        (&demo, false, true),
        (&demo, true, false),
        (&same_title, false, false),
    ] {
        view.update(cx, |view, cx| {
            view.snapshot = Snapshot::capture(machine, running, None, 0.);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("demo-standby").is_some(), standby);
    }
    demo.step().unwrap();
    view.update(cx, |view, cx| {
        view.snapshot = Snapshot::capture(&demo, false, None, 0.);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("demo-standby").is_none());
}

#[gpui::test]
fn game_workspace_keeps_playback_and_quick_actions_below_the_frame(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    cx.run_until_parked();
    click_control(cx, "nav-game");
    for (width, height) in [(980., 680.), (1440., 1000.)] {
        cx.simulate_resize(size(px(width), px(height)));
        for &language in Language::ALL {
            view.update(cx, |view, cx| {
                view.language = language;
                cx.notify();
            });
            cx.run_until_parked();
            let stage = cx.debug_bounds("display-stage").unwrap();
            for selector in [
                "transport-run",
                "quick-save",
                "manage-states",
                "volume-slider",
            ] {
                let button = cx.debug_bounds(selector).expect("game control is rendered");
                assert!(
                    button.top() >= stage.bottom()
                        && button.left() >= stage.left()
                        && button.right() <= stage.right()
                        && button.bottom() <= px(height),
                    "{selector} outside the visible toolbar at {width} × {height}: {button:?}"
                );
            }
            let rewind = cx.debug_bounds("rewind-hold").unwrap();
            assert!(rewind.left() >= stage.right() || rewind.top() >= stage.bottom());
        }
    }
}

#[gpui::test]
fn debugger_columns_keep_details_readable(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(desktop);
    let _cleanup = TestConfig(view.read_with(cx, |v, _| v.preferences_path.clone().unwrap()));
    for width in [980., 1280., 1600.] {
        cx.simulate_resize(size(px(width), px(1400.)));
        for &language in Language::ALL {
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
            let stacked = cx.update(|window, cx| view.read(cx).content_width(window) < px(760.));
            if stacked {
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
    press_keys(cx, "tab tab tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Debug);
    assert_eq!(view.read_with(cx, |v, _| v.pressed_buttons), 0);
    press_keys(cx, "escape f8 tab tab tab enter");
    assert_eq!(
        view.read_with(cx, |v, _| v.settings_tab),
        SettingsTab::Appearance
    );
    press_keys(cx, "tab tab tab tab tab enter");
    let first = Language::ALL[0];
    assert_eq!(view.read_with(cx, |v, _| v.language), first);
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .language
            .as_deref(),
        Some(first.id())
    );
    for &language in Language::ALL {
        let button = cx.debug_bounds(language.id()).unwrap();
        cx.simulate_click(button.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.language), language);
        assert_eq!(
            eidoryn_psx::preferences::Preferences::load(&path)
                .unwrap()
                .language(),
            language
        );
    }
    for _ in 0..24 {
        press_keys(cx, "tab");
        view.update_in(cx, |v, window, cx| {
            assert!(v.settings_focus.contains_focused(window, cx));
        });
    }
    click_control(cx, "theme-lemon-green");
    assert!(view.read_with(cx, |v, _| v.themes.active().id == "lemon-green"));
    assert_eq!(
        eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .theme,
        "lemon-green"
    );
    press_keys(cx, "escape f12");
    assert!(!view.read_with(cx, |v, _| v.clean_fullscreen));
    assert!(
        !eidoryn_psx::preferences::Preferences::load(&path)
            .unwrap()
            .clean_fullscreen
    );
    press_keys(cx, "f11");
    assert!(cx.debug_bounds("fullscreen-display").is_some());
    assert!(cx.debug_bounds("fullscreen-controls").is_some());
    press_keys(cx, "f12");
    assert!(cx.debug_bounds("fullscreen-controls").is_none());
    assert!(cx.debug_bounds("game-controls").is_none());
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
    // Pass the remaining tabs, group headings and bindings to reach Square.
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
fn shortcuts_follow_the_applied_bindings_and_respect_settings_focus(cx: &mut TestAppContext) {
    use eidoryn_psx::shortcuts::{ShortcutAction, ShortcutChord};
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.preferences_path = None;
        for (action, keys) in [
            (ShortcutAction::Settings, "ctrl-j"),
            (ShortcutAction::Crt, "shift-s"),
        ] {
            let stroke = gpui::Keystroke::parse(keys).unwrap();
            view.preferences.shortcuts.set(
                action,
                Some(ShortcutChord::from_keystroke(&stroke).unwrap()),
            );
        }
        view
    });
    let initial = view.read_with(cx, |view, _| view.crt_filter());
    press_keys(cx, "f8 f7 ctrl-alt-j");
    assert!(!view.read_with(cx, |view, _| view.graphics.open));
    assert_eq!(view.read_with(cx, |view, _| view.crt_filter()), initial);

    press_keys(cx, "ctrl-j");
    assert!(view.read_with(cx, |view, _| view.graphics.open));
    press_keys(cx, "shift-s f8");
    assert!(view.read_with(cx, |view, _| view.graphics.open));
    assert_eq!(view.read_with(cx, |view, _| view.crt_filter()), initial);

    press_keys(cx, "ctrl-j");
    assert!(!view.read_with(cx, |view, _| view.graphics.open));
    cx.simulate_keystrokes("shift-s");
    assert_eq!(
        view.read_with(cx, |view, _| view.crt_filter()),
        initial.next()
    );
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), 0);
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
    press_keys(cx, "f8 tab tab tab tab enter tab tab tab tab enter");
    assert_eq!(view.read_with(cx, |v, _| v.rebinding), Some(PadButton::Up));
    for key in ["tab", "ctrl-r", "shift-j"] {
        press_keys(cx, key);
        assert_eq!(view.read_with(cx, |v, _| v.rebinding), Some(PadButton::Up));
        assert!(view.read_with(cx, |v, _| v.graphics.open));
        assert!(view.read_with(cx, |v, _| v.clean_fullscreen));
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

pub(super) fn click_control(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
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

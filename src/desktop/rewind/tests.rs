use crate::desktop::{
    Desktop, SettingsTab, WorkspacePage,
    tests::{desktop, press_keys},
};
use eidoryn_psx::{
    preferences::Preferences,
    systems::ps1::{runner::Command, state_file::SaveStateInfo},
};
use gpui::{
    Entity, KeyUpEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px, size,
};
use std::time::{Duration, Instant};

fn receive_until(
    view: &Entity<Desktop>,
    cx: &mut VisualTestContext,
    ready: impl Fn(&Desktop) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        view.update(cx, Desktop::receive_snapshot);
        cx.run_until_parked();
        if view.read_with(cx, |view, _| ready(view)) {
            return;
        }
        assert!(Instant::now() < deadline, "rewind action did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn enable_recording(view: &Entity<Desktop>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| {
        view.preferences.rewind_enabled = true;
        view.command(Command::SetRewindEnabled(true), cx);
    });
    receive_until(view, cx, |view| view.snapshot.rewind_enabled);
}

fn open_rewind_settings(view: &Entity<Desktop>, cx: &mut VisualTestContext) {
    press_keys(cx, "f8");
    let tab = cx.debug_bounds("tab-rewind").unwrap();
    cx.simulate_click(tab.center(), Modifiers::none());
    assert_eq!(
        view.read_with(cx, |view, _| view.settings_tab),
        SettingsTab::Rewind
    );
}

#[gpui::test]
fn keyboard_rewind_stops_on_release_and_window_deactivation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.preferences_path = None;
        view
    });
    let _subscription = view.update_in(cx, |_, window, cx| {
        window.activate_window();
        cx.observe_window_activation(window, |view, window, cx| {
            if !window.is_window_active() {
                view.stop_input(cx);
            }
        })
    });
    enable_recording(&view, cx);
    for release_control in [false, true] {
        press_keys(cx, "f9");
        receive_until(&view, cx, |view| view.snapshot.rewind_states > 0);
        let before = view.read_with(cx, |view, _| view.snapshot.slices);
        cx.simulate_keystrokes("ctrl-z");
        receive_until(&view, cx, |view| view.snapshot.rewinding);
        view.read_with(cx, |view, _| {
            assert!(!view.snapshot.running);
            assert!(view.snapshot.slices < before);
            assert_eq!(view.pressed_buttons, 0);
        });
        cx.simulate_event(KeyUpEvent {
            keystroke: if release_control {
                Keystroke {
                    modifiers: Modifiers::none(),
                    key: "control".into(),
                    key_char: None,
                }
            } else {
                Keystroke::parse("ctrl-z").unwrap()
            },
        });
        receive_until(&view, cx, |view| !view.snapshot.rewinding);
        assert!(!view.read_with(cx, |view, _| view.snapshot.running));
    }

    press_keys(cx, "f5");
    receive_until(&view, cx, |view| {
        view.snapshot.running && view.snapshot.rewind_states > 0
    });
    cx.simulate_keystrokes("ctrl-z");
    receive_until(&view, cx, |view| view.snapshot.rewinding);
    cx.deactivate_window();
    receive_until(&view, cx, |view| {
        !view.snapshot.rewinding && view.snapshot.running
    });
    assert_eq!(view.read_with(cx, |view, _| view.rewind_sources), 0);
}

#[gpui::test]
fn run_control_finishes_rewind_without_toggling_prior_execution(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.preferences_path = None;
        view
    });
    enable_recording(&view, cx);
    for running in [false, true] {
        press_keys(cx, if running { "f5" } else { "f9" });
        receive_until(&view, cx, |view| {
            view.snapshot.running == running && view.snapshot.rewind_states > 0
        });
        cx.simulate_keystrokes("ctrl-z");
        receive_until(&view, cx, |view| view.snapshot.rewinding);
        press_keys(cx, "f5");
        receive_until(&view, cx, |view| !view.snapshot.rewinding);
        assert_eq!(view.read_with(cx, |view, _| view.snapshot.running), running);
        assert_eq!(view.read_with(cx, |view, _| view.rewind_sources), 0);
    }
}

#[gpui::test]
fn mouse_release_and_disabling_rewind_end_playback(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.page = WorkspacePage::Game;
        view
    });
    let path = view.read_with(cx, |view, _| view.preferences_path.clone().unwrap());
    cx.simulate_resize(size(px(1280.), px(1000.)));
    enable_recording(&view, cx);
    for release_outside in [false, true] {
        press_keys(cx, "f9");
        receive_until(&view, cx, |view| view.snapshot.rewind_states > 0);
        let button = cx.debug_bounds("rewind-hold").unwrap();
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        receive_until(&view, cx, |view| view.snapshot.rewinding);
        let release = if release_outside {
            point(px(1.), px(1.))
        } else {
            button.center()
        };
        cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
        receive_until(&view, cx, |view| !view.snapshot.rewinding);
        assert!(!view.read_with(cx, |view, _| view.snapshot.running));
    }
    press_keys(cx, "f9");
    receive_until(&view, cx, |view| view.snapshot.rewind_states > 0);
    cx.simulate_keystrokes("ctrl-z");
    receive_until(&view, cx, |view| view.snapshot.rewinding);
    open_rewind_settings(&view, cx);
    let toggle = cx.debug_bounds("rewind-enabled").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::none());
    let confirm = cx.debug_bounds("rewind-confirm").unwrap();
    cx.simulate_click(confirm.center(), Modifiers::none());
    receive_until(&view, cx, |view| {
        !view.snapshot.rewind_enabled && !view.snapshot.rewinding
    });
    assert!(!Preferences::load(&path).unwrap().rewind_enabled);
    assert_eq!(view.read_with(cx, |view, _| view.snapshot.rewind_states), 0);
    press_keys(cx, "ctrl-z");
    assert_eq!(view.read_with(cx, |view, _| view.rewind_sources), 0);
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn rewind_shortcut_is_blocked_by_settings_and_rename(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.preferences_path = None;
        view
    });
    cx.simulate_resize(size(px(1280.), px(1000.)));
    press_keys(cx, "f6");
    enable_recording(&view, cx);
    press_keys(cx, "f8");
    cx.simulate_keystrokes("ctrl-z");
    assert!(view.read_with(cx, |view, _| view.graphics.open));
    assert_eq!(view.read_with(cx, |view, _| view.rewind_sources), 0);
    press_keys(cx, "ctrl-z escape f6");

    view.update(cx, |view, cx| {
        view.page = crate::desktop::WorkspacePage::SaveStates;
        let path = std::path::PathBuf::from("rename-test.ps1state");
        view.snapshot.save_states = vec![SaveStateInfo {
            modified: std::time::UNIX_EPOCH,
            path: path.clone(),
            name: "rename-test".into(),
        }];
        view.selected_save_state = Some(path);
        cx.notify();
    });
    cx.run_until_parked();
    let rename = cx.debug_bounds("rename-snapshot-0").unwrap();
    cx.simulate_click(rename.center(), Modifiers::none());
    assert!(cx.debug_bounds("rename-dialog").is_some());
    cx.simulate_keystrokes("ctrl-z");
    assert!(view.read_with(cx, |view, _| view.renaming_state.is_some()));
    assert_eq!(view.read_with(cx, |view, _| view.rewind_sources), 0);
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), 0);
}

#[gpui::test]
fn recording_is_opt_in_and_settings_persist_duration_and_enabled_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.page = WorkspacePage::Game;
        view
    });
    let path = view.read_with(cx, |view, _| view.preferences_path.clone().unwrap());
    cx.simulate_resize(size(px(1280.), px(1000.)));
    press_keys(cx, "f9");
    receive_until(&view, cx, |view| view.snapshot.slices > 0);
    view.read_with(cx, |view, _| {
        assert!(!view.preferences.rewind_enabled);
        assert_eq!(view.preferences.rewind_seconds, 30);
        assert!(!view.snapshot.rewind_enabled);
        assert_eq!(view.snapshot.rewind_max_seconds, 30);
        assert_eq!(view.snapshot.rewind_states, 0);
    });
    press_keys(cx, "ctrl-z");
    let hold = cx.debug_bounds("rewind-hold").unwrap();
    cx.simulate_click(hold.center(), Modifiers::none());
    view.read_with(cx, |view, _| {
        assert!(!view.snapshot.rewinding);
        assert_eq!(view.rewind_sources, 0);
    });
    assert!(cx.debug_bounds("rewind-enabled").is_none());

    open_rewind_settings(&view, cx);
    assert!(cx.debug_bounds("rewind-duration").is_some());
    for (selector, seconds) in [
        ("rewind-duration-increase", 35),
        ("rewind-duration-decrease", 30),
        ("rewind-duration-increase", 35),
    ] {
        let button = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(button.center(), Modifiers::none());
        receive_until(&view, cx, |view| {
            view.snapshot.rewind_max_seconds == seconds
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.preferences.rewind_seconds, seconds);
            assert!(!view.preferences.rewind_enabled);
            assert!(!view.snapshot.rewind_enabled);
            assert_eq!(view.snapshot.rewind_states, 0);
        });
    }
    let stored = Preferences::load(&path).unwrap();
    assert!(!stored.rewind_enabled);
    assert_eq!(stored.rewind_seconds, 35);

    let toggle = cx.debug_bounds("rewind-enabled").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::none());
    receive_until(&view, cx, |view| view.snapshot.rewind_enabled);
    let stored = Preferences::load(&path).unwrap();
    assert!(stored.rewind_enabled);
    assert_eq!(stored.rewind_seconds, 35);
    press_keys(cx, "escape f9");
    receive_until(&view, cx, |view| view.snapshot.rewind_states > 0);
    assert_eq!(
        view.read_with(cx, |view, _| view.snapshot.rewind_max_seconds),
        35
    );

    // From the drawer focus, traverse its tabs and reach the recording switch.
    press_keys(cx, "f8 tab tab tab tab tab enter tab tab enter");
    assert!(cx.debug_bounds("rewind-confirm").is_some());
    press_keys(cx, "tab enter");
    assert!(cx.debug_bounds("rewind-confirm").is_none());
    assert!(view.read_with(cx, |v, _| v.snapshot.rewind_states > 0
        && v.preferences.rewind_enabled));
    // Cancel returns focus to the switch. Open again and activate the destructive action.
    press_keys(cx, "enter tab tab enter");
    receive_until(&view, cx, |view| {
        !view.snapshot.rewind_enabled && view.snapshot.rewind_states == 0
    });
    let stored = Preferences::load(&path).unwrap();
    assert!(!stored.rewind_enabled);
    assert_eq!(stored.rewind_seconds, 35);
    std::fs::remove_file(path).unwrap();
}

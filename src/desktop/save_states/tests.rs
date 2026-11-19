use crate::desktop::{
    Desktop, WorkspacePage,
    tests::{click_control, desktop, press_keys},
};
use eidoryn_psx::systems::ps1::{
    card_file::CardStore,
    input::PadInput,
    machine::Machine,
    runner::{Command, QuickSlot, Runner, Snapshot, SnapshotMode},
    state_file::StateStore,
};
use gpui::{
    Entity, EntityInputHandler, Focusable, Modifiers, TestAppContext, VisualTestContext, px, size,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct SnapshotFiles {
    original: PathBuf,
    other: PathBuf,
}

impl Drop for SnapshotFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.original.parent().unwrap());
    }
}

fn history_window(
    cx: &mut TestAppContext,
    additional_registers: impl IntoIterator<Item = u32>,
) -> (Entity<Desktop>, &mut VisualTestContext, SnapshotFiles) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let title = format!(
        "snapshot-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let machine = Machine::from_exe(&eidoryn_psx::demo::executable(), &title).unwrap();
    // Runner uses saves as its root, so isolate and remove this test's game directory.
    let store = StateStore::new(Path::new("saves"), &machine);
    let mut saved = machine.clone();
    saved.cpu.registers[5] = 17;
    let original = store.save(&saved, &PadInput::default()).unwrap();
    saved.cpu.registers[5] = 29;
    let other = store.save(&saved, &PadInput::default()).unwrap();
    for register in additional_registers {
        saved.cpu.registers[5] = register;
        store.save(&saved, &PadInput::default()).unwrap();
    }
    let selected = original.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let mut view = desktop(window, cx);
        view.preferences_path = None;
        view.snapshot =
            Snapshot::capture_with_mode(&machine, false, None, 0., SnapshotMode::Display);
        view.snapshot.save_states = store.list().unwrap();
        view.selected_save_state = Some(selected);
        view.runner = Runner::with_snapshot_mode(
            machine,
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        )
        .unwrap();
        view.page = WorkspacePage::SaveStates;
        view
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    (view, cx, SnapshotFiles { original, other })
}

fn open_rename(cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds("rename-snapshot-1").unwrap();
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("rename-dialog").is_some());
}

fn receive_feedback(view: &Entity<Desktop>, cx: &mut VisualTestContext) {
    receive_until(view, cx, |view| {
        view.snapshot.message.is_some() || view.snapshot.success_message.is_some()
    });
}

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
        assert!(Instant::now() < deadline, "snapshot action did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui::test]
fn game_snapshot_menu_loads_the_three_newest_checkpoints_in_place(cx: &mut TestAppContext) {
    let (view, cx, _files) = history_window(cx, [41, 53]);
    press_keys(cx, "f6");
    for (selector, register, width, height, opens_above) in [
        ("recent-snapshot-0", 53, 1568., 980., true),
        ("recent-snapshot-1", 41, 980., 1400., false),
        ("recent-snapshot-2", 29, 1568., 980., true),
    ] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        let stage = cx.debug_bounds("display-stage").unwrap();
        click_control(cx, "manage-states");
        assert_eq!(cx.debug_bounds("display-stage"), Some(stage));
        let menu = cx.debug_bounds("snapshot-menu").unwrap();
        let trigger = cx.debug_bounds("manage-states").unwrap();
        let gap = if opens_above {
            trigger.top() - menu.bottom()
        } else {
            menu.top() - trigger.bottom()
        };
        assert!(
            (px(3.)..=px(5.)).contains(&gap),
            "menu {menu:?} should sit just above or below trigger {trigger:?}"
        );
        assert!(menu.left() >= px(0.) && menu.right() <= px(width));
        assert!(menu.top() >= px(0.) && menu.bottom() <= px(height));
        let mut bottom = menu.top();
        for row in [
            "recent-snapshot-0",
            "recent-snapshot-1",
            "recent-snapshot-2",
            "manage-all-snapshots",
        ] {
            let bounds = cx.debug_bounds(row).expect("visible snapshot menu item");
            assert!(bounds.top() >= bottom && bounds.bottom() <= menu.bottom());
            bottom = bounds.bottom();
        }
        assert!(cx.debug_bounds("recent-snapshot-3").is_none());
        if selector == "recent-snapshot-1" {
            press_keys(cx, "down enter");
        } else {
            click_control(cx, selector);
        }
        receive_until(&view, cx, |view| view.snapshot.cpu.registers[5] == register);
        assert!(cx.debug_bounds("snapshot-menu").is_none());
        assert!(cx.debug_bounds("game-controls").is_some());
        assert!(cx.update(|window, _| window.is_fullscreen()));
        assert!(!view.read_with(cx, |view, _| view.snapshot.running));
    }
}

#[gpui::test]
fn game_snapshot_menu_can_be_dismissed_and_manages_an_empty_history(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    click_control(cx, "nav-game");
    assert_eq!(view.read_with(cx, |view, _| view.page), WorkspacePage::Game);
    click_control(cx, "manage-states");
    assert!(cx.debug_bounds("recent-snapshot-1").is_some());
    assert!(cx.debug_bounds("recent-snapshot-2").is_none());
    click_control(cx, "manage-states");
    assert!(cx.debug_bounds("snapshot-menu").is_none());
    assert!(cx.debug_bounds("game-controls").is_some(), "after toggle");
    click_control(cx, "manage-states");
    cx.simulate_keystrokes("z");
    assert_eq!(view.read_with(cx, |view, _| view.pressed_buttons), 0);
    press_keys(cx, "escape");
    assert!(cx.debug_bounds("snapshot-menu").is_none());
    assert!(cx.debug_bounds("game-controls").is_some());
    press_keys(cx, "enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("snapshot-menu").is_some());
    let outside = cx.debug_bounds("display-stage").unwrap().center();
    cx.simulate_click(outside, Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("snapshot-menu").is_none());
    assert!(
        cx.debug_bounds("game-controls").is_some(),
        "after outside click"
    );

    fs::remove_file(&files.original).unwrap();
    fs::remove_file(&files.other).unwrap();
    click_control(cx, "manage-states");
    receive_until(&view, cx, |view| view.snapshot.save_states.is_empty());
    assert!(cx.debug_bounds("snapshot-menu-empty").is_some());
    assert!(cx.debug_bounds("recent-snapshot-0").is_none());
    press_keys(cx, "enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("snapshot-menu").is_none());
    assert!(cx.debug_bounds("save-states-page").is_some());
}

#[gpui::test]
fn delete_removes_only_the_confirmed_history_row(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    let quick = files.original.parent().unwrap().join("quick/slot.ps1state");
    press_keys(cx, "ctrl-shift-s");
    receive_until(&view, cx, |view| {
        matches!(view.snapshot.quick_slot, QuickSlot::Saved(_))
    });
    let quick_bytes = fs::read(&quick).unwrap();
    let other_bytes = fs::read(&files.other).unwrap();

    let delete = cx.debug_bounds("delete-snapshot-1").unwrap().center();
    for cancel in ["escape", "enter"] {
        cx.simulate_click(delete, Modifiers::none());
        assert!(cx.debug_bounds("confirm-delete-snapshot").is_some());
        press_keys(cx, cancel);
        assert!(cx.debug_bounds("confirm-delete-snapshot").is_none());
        assert!(files.original.is_file());
    }

    cx.simulate_click(delete, Modifiers::none());
    press_keys(cx, "tab enter");
    receive_until(&view, cx, |view| view.snapshot.save_states.len() == 1);
    assert!(!files.original.exists());
    assert_eq!(fs::read(&files.other).unwrap(), other_bytes);
    view.read_with(cx, |view, _| {
        assert_eq!(view.snapshot.save_states[0].path, files.other);
        assert_eq!(view.selected_save_state.as_ref(), Some(&files.other));
        assert!(!view.snapshot.running);
        assert!(view.snapshot.message.is_none());
    });

    let last = cx.debug_bounds("delete-snapshot-0").unwrap().center();
    cx.simulate_click(last, Modifiers::none());
    let confirm = cx.debug_bounds("confirm-delete-snapshot").unwrap().center();
    cx.simulate_click(confirm, Modifiers::none());
    receive_until(&view, cx, |view| view.snapshot.save_states.is_empty());
    assert!(!files.other.exists());
    assert!(cx.debug_bounds("delete-snapshot-0").is_none());
    assert!(view.read_with(cx, |view, _| view.selected_save_state.is_none()));
    assert_eq!(fs::read(quick).unwrap(), quick_bytes);
}

#[gpui::test]
fn delete_reports_a_snapshot_changed_during_confirmation(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    let delete = cx.debug_bounds("delete-snapshot-1").unwrap().center();
    cx.simulate_click(delete, Modifiers::none());
    assert!(cx.debug_bounds("confirm-delete-snapshot").is_some());

    fs::OpenOptions::new()
        .write(true)
        .open(&files.original)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(60))
        .unwrap();
    press_keys(cx, "tab enter");
    receive_until(&view, cx, |view| view.snapshot.message.is_some());
    assert!(cx.debug_bounds("error-notice").is_some());
    assert!(files.original.is_file());
    assert!(files.other.is_file());
    assert_eq!(
        view.read_with(cx, |view, _| view.snapshot.save_states.len()),
        2
    );
}

#[gpui::test]
fn quick_controls_reuse_one_slot_without_using_the_snapshot_selection(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    let quick = files.original.parent().unwrap().join("quick/slot.ps1state");
    for (index, path) in [&files.original, &files.other].into_iter().enumerate() {
        let register = if index == 0 { 17 } else { 29 };
        view.update(cx, |view, cx| {
            view.command(Command::LoadState { path: path.clone() }, cx);
        });
        receive_until(&view, cx, |view| view.snapshot.cpu.registers[5] == register);
        let before = fs::read(&quick).ok();
        if index == 0 {
            press_keys(cx, "ctrl-shift-s");
        } else {
            let button = cx.debug_bounds("quick-save").unwrap();
            cx.simulate_click(button.center(), Modifiers::none());
            assert!(cx.debug_bounds("confirm-quick-save").is_some());
            press_keys(cx, "escape");
            assert!(cx.debug_bounds("confirm-quick-save").is_none());
            assert_eq!(fs::read(&quick).ok(), before);
            cx.simulate_click(button.center(), Modifiers::none());
            press_keys(cx, "tab enter");
        }
        receive_until(&view, cx, |_| {
            fs::read(&quick).is_ok_and(|bytes| before.as_ref().is_none_or(|old| old != &bytes))
        });
        assert_eq!(fs::read_dir(quick.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(
            view.read_with(cx, |view, _| view.snapshot.save_states.len()),
            2
        );
    }
    let quick_bytes = fs::read(&quick).unwrap();
    press_keys(cx, "ctrl-s");
    receive_until(&view, cx, |view| view.snapshot.save_states.len() == 3);
    assert_eq!(fs::read(&quick).unwrap(), quick_bytes);
    assert!(view.read_with(cx, |view, _| {
        view.snapshot
            .save_states
            .iter()
            .all(|state| state.path != quick)
    }));

    for use_button in [false, true] {
        view.update(cx, |view, cx| {
            view.selected_save_state = Some(files.original.clone());
            view.command(
                Command::LoadState {
                    path: files.original.clone(),
                },
                cx,
            );
        });
        receive_until(&view, cx, |view| view.snapshot.cpu.registers[5] == 17);
        if use_button {
            let button = cx.debug_bounds("quick-load").unwrap();
            cx.simulate_click(button.center(), Modifiers::none());
        } else {
            press_keys(cx, "ctrl-shift-l");
        }
        receive_until(&view, cx, |view| view.snapshot.cpu.registers[5] == 29);
        view.read_with(cx, |view, _| {
            assert!(!view.snapshot.running);
            assert!(view.snapshot.message.is_none());
            assert_eq!(view.selected_save_state.as_ref(), Some(&files.original));
        });
    }
    assert!(cx.debug_bounds("success-notice").is_some());
}

#[gpui::test]
fn quick_shortcuts_are_blocked_by_rename_and_settings(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    let quick = files.original.parent().unwrap().join("quick/slot.ps1state");
    for mode in [SnapshotMode::Debug, SnapshotMode::Display] {
        if mode == SnapshotMode::Debug {
            open_rename(cx);
        } else {
            press_keys(cx, "f8");
            assert!(view.read_with(cx, |view, _| view.graphics.open));
        }
        press_keys(cx, "ctrl-shift-s ctrl-shift-l");
        // A worker acknowledgement after both keys makes unintended commands observable.
        view.read_with(cx, |view, _| {
            view.runner.send(Command::SetSnapshotMode(mode)).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(snapshot) = view.read_with(cx, |view, _| view.runner.take_snapshot())
                && snapshot.mode == mode
            {
                assert!(snapshot.message.is_none());
                assert!(!quick.exists());
                break;
            }
            assert!(Instant::now() < deadline, "worker did not acknowledge keys");
            std::thread::sleep(Duration::from_millis(1));
        }
        press_keys(cx, "escape");
    }
}

#[gpui::test]
fn rename_accepts_unicode_and_keeps_the_renamed_snapshot_selected(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    open_rename(cx);
    let os = cx.read(|cx| cx.platform_info().os);
    for key in ["tab", "shift-tab"] {
        let mut returned_to_input = false;
        for _ in 0..4 {
            cx.simulate_keystrokes(key);
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse_with_os(key, os).unwrap(),
            });
            returned_to_input = cx.update(|window, cx| {
                let rename = view.read(cx).renaming_state.as_ref().unwrap();
                assert!(rename.focus.contains_focused(window, cx));
                rename.input.focus_handle(cx).is_focused(window)
            });
            if returned_to_input {
                break;
            }
        }
        assert!(returned_to_input, "{key} did not return to the name field");
    }
    cx.simulate_input("城门🙂");
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.renaming_state.as_ref().unwrap().input.read(cx).value(),
            "城门🙂"
        );
        assert_eq!(view.pressed_buttons, 0);
    });
    let input = view.read_with(cx, |view, _| {
        view.renaming_state.as_ref().unwrap().input.clone()
    });
    input.update_in(cx, |input, window, cx| {
        input.replace_and_mark_text_in_range(Some(0..4), "cheng", Some(5..5), window, cx);
    });
    for key in ["enter", "escape"] {
        cx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse_with_os(key, os).unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
    }
    assert!(view.read_with(cx, |view, _| view.renaming_state.is_some()));
    assert!(input.read_with(cx, |input, _| input.is_composing()));
    input.update_in(cx, |input, window, cx| {
        input.replace_text_in_range(None, "城门🙂", window, cx);
    });
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |view, _| view.renaming_state.is_none()));
    receive_feedback(&view, cx);
    let renamed = files.original.with_file_name("城门🙂.ps1state");
    assert!(!files.original.exists());
    assert!(renamed.is_file());
    view.read_with(cx, |view, _| {
        assert_eq!(view.selected_save_state.as_ref(), Some(&renamed));
        assert_eq!(
            view.snapshot.renamed_state,
            Some((files.original.clone(), renamed))
        );
        assert_eq!(view.snapshot.save_states[0].path, files.other);
        assert!(view.snapshot.message.is_none());
    });
    assert!(cx.debug_bounds("success-notice").is_some());
}

#[gpui::test]
fn cancel_and_conflicting_names_preserve_files_and_game_input(cx: &mut TestAppContext) {
    let (view, cx, files) = history_window(cx, []);
    let original_bytes = fs::read(&files.original).unwrap();
    let other_bytes = fs::read(&files.other).unwrap();
    open_rename(cx);
    cx.simulate_input("丢弃🙂");
    cx.simulate_keystrokes("z up f6 f8");
    view.read_with(cx, |view, _| {
        assert_eq!(view.pressed_buttons, 0);
        assert_eq!(view.page, WorkspacePage::SaveStates);
        assert!(!view.graphics.open);
    });
    assert!(!cx.update(|window, _| window.is_fullscreen()));
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |view, _| view.renaming_state.is_none()));
    assert!(files.original.is_file());
    open_rename(cx);
    cx.simulate_input(files.other.file_stem().unwrap().to_str().unwrap());
    cx.simulate_keystrokes("enter");
    receive_feedback(&view, cx);
    assert_eq!(fs::read(&files.original).unwrap(), original_bytes);
    assert_eq!(fs::read(&files.other).unwrap(), other_bytes);
    view.read_with(cx, |view, _| {
        assert_eq!(view.selected_save_state.as_ref(), Some(&files.original));
        assert!(
            view.snapshot
                .message
                .as_ref()
                .unwrap()
                .contains("already exists")
        );
        assert!(view.snapshot.renamed_state.is_none());
    });
    assert!(cx.debug_bounds("error-notice").is_some());
}

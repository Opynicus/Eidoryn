use super::{
    Desktop,
    components::{button, ghost_icon_button, label},
};
use eidoryn_psx::{
    i18n::Text,
    keybindings::ControllerButton,
    shortcuts::{ShortcutAction, ShortcutBindings, ShortcutChord},
};
use gpui::{App, Context, Div, KeyDownEvent, Keystroke, Role, div, prelude::*, px, rgb};
use std::borrow::Cow;

/// GPUI on Windows can report a shifted symbol such as "!" without the Shift
/// flag. Use the current layout's base key for capture, dispatch and release.
pub(super) fn shortcut_keystroke<'a>(stroke: &'a Keystroke, cx: &App) -> Cow<'a, Keystroke> {
    if cfg!(windows)
        && stroke.key.chars().count() == 1
        && !stroke.key.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        let mapped = cx
            .keyboard_mapper()
            .map_key_equivalent(stroke.clone(), false);
        if mapped.key() != stroke.key || mapped.modifiers() != &stroke.modifiers {
            return Cow::Owned(Keystroke {
                key: mapped.key().into(),
                modifiers: *mapped.modifiers(),
                key_char: None,
            });
        }
    }
    Cow::Borrowed(stroke)
}

#[derive(Default)]
pub(super) struct ShortcutEditor {
    pub(super) draft: Option<ShortcutBindings>,
    pub(super) recording: Option<ShortcutAction>,
}

pub(super) fn action_title(action: ShortcutAction) -> Text {
    match action {
        ShortcutAction::Toggle => Text::RunPause,
        ShortcutAction::Enlarge => Text::EnlargeGame,
        ShortcutAction::Crt => Text::Crt,
        ShortcutAction::Settings => Text::Settings,
        ShortcutAction::Step => Text::Step,
        ShortcutAction::Slice => Text::Slice,
        ShortcutAction::Fullscreen => Text::Fullscreen,
        ShortcutAction::CleanFullscreen => Text::CleanFullscreen,
        ShortcutAction::Reset => Text::Reset,
        ShortcutAction::Snapshot => Text::SaveState,
        ShortcutAction::QuickSave => Text::QuickSave,
        ShortcutAction::QuickLoad => Text::QuickLoad,
        ShortcutAction::Rewind => Text::Rewind,
    }
}

impl Desktop {
    pub(super) fn capture_shortcut(
        &mut self,
        action: ShortcutAction,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if event.is_held {
            return;
        }
        if event.keystroke.key == "escape" {
            self.shortcut_editor.recording = None;
            self.notice = None;
        } else if let Some(chord) =
            ShortcutChord::from_keystroke(&shortcut_keystroke(&event.keystroke, cx))
        {
            self.edit_shortcut(action, Some(chord));
        } else {
            self.notice = Some(self.tr(Text::ShortcutUnsupported).into());
        }
        cx.notify();
    }

    fn edit_shortcut(&mut self, action: ShortcutAction, chord: Option<ShortcutChord>) {
        self.shortcut_editor
            .draft
            .get_or_insert_with(|| self.preferences.shortcuts.clone())
            .set(action, chord);
        self.shortcut_editor.recording = None;
        self.notice = None;
    }

    pub(super) fn apply_shortcuts(&mut self, cx: &mut Context<Self>) {
        let Some(bindings) = &self.shortcut_editor.draft else {
            return;
        };
        if bindings.validate(&self.preferences.keybindings).is_err()
            || bindings
                .validate(&self.preferences.nds_keybindings)
                .is_err()
            || bindings.gba_controller_conflict(&self.preferences.gba_keybindings)
        {
            self.notice = Some(self.tr(Text::BindingInvalid).into());
        } else if let Some(bindings) = self.shortcut_editor.draft.take() {
            self.stop_rewind(cx);
            self.release_buttons(cx);
            self.preferences.shortcuts = bindings;
            self.save_preferences();
        }
    }

    pub(super) fn shortcuts_panel(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.themes.active().palette;
        let bindings = self
            .shortcut_editor
            .draft
            .as_ref()
            .unwrap_or(&self.preferences.shortcuts);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label(p, self.tr(Text::ShortcutEditHint)))
            .when(bindings != &self.preferences.shortcuts, |panel| {
                panel.child(label(p, self.tr(Text::BindingDraft)).text_color(rgb(p.primary)))
            })
            .children(
                ShortcutAction::ALL
                    .into_iter()
                    .filter(|action| {
                        !self.is_nds()
                            || !matches!(action, ShortcutAction::Snapshot | ShortcutAction::Rewind)
                    })
                    .map(|action| self.shortcut_row(action, bindings, cx)),
            )
    }

    fn shortcut_row(
        &self,
        action: ShortcutAction,
        bindings: &ShortcutBindings,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = &self.themes.active().palette;
        let field = if self.is_nds() && action == ShortcutAction::Slice {
            Text::StepFrame
        } else {
            action_title(action)
        };
        let title = self.tr(field);
        let binding = bindings.binding(action);
        let capturing = self.shortcut_editor.recording == Some(action);
        let caption = if capturing {
            self.tr(Text::BindingPrompt).into()
        } else {
            binding.map_or_else(
                || self.tr(Text::ShortcutUnassigned).into(),
                ShortcutChord::label,
            )
        };
        let conflict = bindings
            .conflict(action)
            .map(|other| self.tr(action_title(other)).to_owned())
            .or_else(|| {
                let chord = binding?;
                if let Some(button) = chord.pad_conflict(&self.preferences.keybindings) {
                    return Some(format!("PS1 · {}", button.name()));
                }
                chord
                    .pad_conflict(&self.preferences.gba_keybindings)
                    .map(|button| format!("GBA · {}", button.name()))
                    .or_else(|| {
                        chord
                            .pad_conflict(&self.preferences.nds_keybindings)
                            .map(|button| format!("NDS · {}", button.name()))
                    })
            });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_sm().child(title))
                    .child(
                        button(
                            &self.fonts,
                            p,
                            ("shortcut", action as usize),
                            caption.clone(),
                            false,
                        )
                        .min_w(px(115.))
                        .role(Role::Button)
                        .aria_label(format!("{title} · {caption}"))
                        .debug_selector(move || format!("shortcut-{}", field.id()))
                        .when(capturing, |b| b.border_color(rgb(p.primary)))
                        .when(conflict.is_some(), |b| b.border_color(rgb(p.danger_text())))
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.release_buttons(cx);
                            v.rebinding = None;
                            v.shortcut_editor.recording = Some(action);
                            v.notice = None;
                            window.focus(&v.settings_focus, cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        ghost_icon_button(
                            p,
                            ("clear-shortcut", action as usize),
                            "close",
                            self.tr(Text::ShortcutClear),
                        )
                        .role(Role::Button)
                        .aria_label(format!("{} · {title}", self.tr(Text::ShortcutClear)))
                        .debug_selector(move || format!("clear-shortcut-{}", field.id()))
                        .when(binding.is_none(), |b| {
                            b.opacity(0.45).tab_index(-1).cursor_default()
                        })
                        .when(binding.is_some(), |b| {
                            b.on_click(cx.listener(move |v, _, _, cx| {
                                v.edit_shortcut(action, None);
                                cx.notify();
                            }))
                        }),
                    ),
            )
            .when_some(conflict, |row, other| {
                row.child(
                    label(p, format!("{} · {other}", self.tr(Text::ShortcutConflict)))
                        .debug_selector(move || format!("conflict-shortcut-{}", field.id()))
                        .text_color(rgb(p.danger_text())),
                )
            })
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::*;
    use crate::desktop::tests::{click_control, desktop, press_keys};
    use eidoryn_psx::{keybindings::PadButton, preferences::Preferences};
    use gpui::{TestAppContext, size};

    #[gpui::test]
    fn shortcut_drafts_can_be_recorded_cleared_restored_and_discarded(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let (view, cx) = cx.add_window_view(desktop);
        view.update(cx, |v, _| v.preferences_path = Some(path.clone()));
        cx.simulate_resize(size(px(1280.), px(1200.)));
        press_keys(cx, "f8");
        click_control(cx, "tab-shortcuts");

        click_control(cx, "shortcut-RunPause");
        press_keys(cx, "f8");
        assert!(view.read_with(cx, |v, _| v.graphics.open));
        assert!(cx.debug_bounds("conflict-shortcut-RunPause").is_some());
        click_control(cx, "shortcuts-apply");
        assert!(view.read_with(cx, |v, _| v.notice.is_some()));
        assert!(!path.exists());
        click_control(cx, "shortcut-RunPause");
        press_keys(cx, "x");
        assert!(cx.debug_bounds("conflict-shortcut-RunPause").is_some());
        click_control(cx, "shortcut-RunPause");
        press_keys(cx, "ctrl-shift-p");
        assert!(cx.debug_bounds("conflict-shortcut-RunPause").is_none());
        click_control(cx, "clear-shortcut-EnlargeGame");
        click_control(cx, "shortcuts-apply");
        let saved = Preferences::load(&path).unwrap().shortcuts;
        assert_eq!(
            saved.binding(ShortcutAction::Toggle).unwrap().label(),
            "Ctrl+Shift+P"
        );
        assert!(saved.binding(ShortcutAction::Enlarge).is_none());

        click_control(cx, "shortcut-RunPause");
        press_keys(cx, "escape");
        assert!(view.read_with(cx, |v, _| v.graphics.open
            && v.shortcut_editor.recording.is_none()));
        click_control(cx, "shortcuts-reset");
        click_control(cx, "shortcuts-cancel");
        assert_eq!(Preferences::load(&path).unwrap().shortcuts, saved);
        click_control(cx, "shortcuts-reset");
        click_control(cx, "shortcuts-apply");
        assert_eq!(
            Preferences::load(&path).unwrap().shortcuts,
            ShortcutBindings::default()
        );
    }

    #[gpui::test]
    fn controller_changes_respect_confirmed_application_shortcuts(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let (view, cx) = cx.add_window_view(desktop);
        view.update(cx, |v, _| v.preferences_path = Some(path.clone()));
        cx.simulate_resize(size(px(1280.), px(1200.)));
        press_keys(cx, "f8");
        click_control(cx, "tab-shortcuts");
        click_control(cx, "shortcut-RunPause");
        press_keys(cx, "j");
        click_control(cx, "shortcuts-apply");
        let saved = Preferences::load(&path).unwrap();
        click_control(cx, "tab-controller");
        click_control(cx, "binding-4");
        press_keys(cx, "j");
        assert!(cx.debug_bounds("bindings-conflict").is_some());
        click_control(cx, "bindings-apply");
        assert!(view.read_with(cx, |v, _| v.notice.is_some()));
        assert_eq!(Preferences::load(&path).unwrap(), saved);
        click_control(cx, "binding-4");
        press_keys(cx, "f5");
        click_control(cx, "bindings-apply");
        assert_eq!(
            Preferences::load(&path)
                .unwrap()
                .keybindings
                .key(PadButton::Up),
            "f5"
        );
    }
}

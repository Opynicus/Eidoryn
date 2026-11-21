use super::{
    Desktop, SettingsTab,
    components::{button, label},
    shortcuts::action_title,
};
use eidoryn_psx::{
    i18n::Text,
    keybindings::{KeyBindings, PadButton},
    shortcuts::ShortcutAction,
    systems::ps1::{
        input::{TURBO_RATES, TurboConfig},
        runner::Command,
    },
};
use gpui::{Context, KeyDownEvent, Window, div, prelude::*, px, rgb};

const BINDINGS: [(PadButton, &str); PadButton::ALL.len()] = [
    (PadButton::Up, "binding-4"),
    (PadButton::Down, "binding-6"),
    (PadButton::Left, "binding-7"),
    (PadButton::Right, "binding-5"),
    (PadButton::Cross, "binding-14"),
    (PadButton::Square, "binding-15"),
    (PadButton::Triangle, "binding-12"),
    (PadButton::Circle, "binding-13"),
    (PadButton::Start, "binding-3"),
    (PadButton::Select, "binding-0"),
    (PadButton::L1, "binding-10"),
    (PadButton::R1, "binding-11"),
    (PadButton::L2, "binding-8"),
    (PadButton::R2, "binding-9"),
];

const TURBO_CHOICES: [(u8, &str, &str); TURBO_RATES.len()] = [
    (5, "turbo-rate-5", "5 Hz"),
    (10, "turbo-rate-10", "10 Hz"),
    (15, "turbo-rate-15", "15 Hz"),
    (20, "turbo-rate-20", "20 Hz"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingAction {
    Apply,
    Reset,
    Cancel,
}

impl Desktop {
    pub(super) fn capture_binding(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(action) = self.shortcut_editor.recording {
            self.capture_shortcut(action, event, cx);
            return;
        }
        let Some(button) = self.rebinding else {
            return;
        };
        cx.stop_propagation();
        if event.is_held {
            return;
        }
        let key = event.keystroke.key.to_lowercase();
        if key == "escape" {
            self.rebinding = None;
            self.notice = None;
        } else if event.keystroke.modifiers.modified()
            || self
                .keybindings_draft
                .get_or_insert_with(|| self.preferences.keybindings.clone())
                .rebind(button, &key)
                .is_err()
        {
            self.notice = Some(self.tr(Text::BindingUnsupported).into());
        } else {
            self.rebinding = None;
            self.notice = None;
        }
        cx.notify();
    }

    fn apply_bindings(&mut self, cx: &mut Context<Self>) {
        let Some(bindings) = &self.keybindings_draft else {
            return;
        };
        self.rebinding = None;
        if bindings.validate().is_err() || self.preferences.shortcuts.validate(bindings).is_err() {
            self.notice = Some(self.tr(Text::BindingInvalid).into());
        } else if let Some(bindings) = self.keybindings_draft.take() {
            self.release_buttons(cx);
            self.preferences.keybindings = bindings;
            self.notice = None;
            self.save_preferences();
        }
        cx.notify();
    }

    fn set_turbo(&mut self, buttons: u16, hz: u8, cx: &mut Context<Self>) {
        let result = TurboConfig::new(buttons, hz)
            .and_then(|config| self.runner.send(Command::SetTurbo(config)));
        match result {
            Ok(()) => {
                self.preferences.turbo_buttons = buttons;
                self.preferences.turbo_hz = hz;
                self.notice = None;
                self.save_preferences();
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    fn keybindings_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let bindings = self
            .keybindings_draft
            .as_ref()
            .unwrap_or(&self.preferences.keybindings);
        let occupied: Vec<_> = ShortcutAction::ALL
            .into_iter()
            .filter_map(|action| {
                let chord = self.preferences.shortcuts.binding(action)?;
                Some((chord.pad_conflict(bindings)?, self.tr(action_title(action))))
            })
            .collect();
        let conflicts = PadButton::ALL
            .into_iter()
            .filter_map(|pad| {
                let other = bindings.conflict(pad).map(PadButton::name).or_else(|| {
                    occupied
                        .iter()
                        .find_map(|&(button, title)| (button == pad).then_some(title))
                })?;
                Some((
                    pad,
                    format!(
                        "{} · {} ({other})",
                        pad.name(),
                        bindings.key(pad).to_uppercase()
                    ),
                ))
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .when(bindings != &self.preferences.keybindings, |panel| {
                panel.child(label(p, self.tr(Text::BindingDraft)).text_color(rgb(p.primary)))
            })
            .child(label(p, self.tr(Text::KeyBindingsHint)))
            .child(label(p, self.tr(Text::BindingCancelHint)))
            .when(!conflicts.is_empty(), |panel| {
                let details = conflicts
                    .iter()
                    .map(|(_, text)| text.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                panel.child(
                    label(p, format!("{}: {details}", self.tr(Text::BindingConflict)))
                        .debug_selector(|| "bindings-conflict".into())
                        .p_3()
                        .rounded_md()
                        .bg(rgb(p.error))
                        .text_color(rgb(p.on_error)),
                )
            })
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .children(BINDINGS.into_iter().map(|(pad, id)| {
                        let waiting = self.rebinding == Some(pad);
                        let key = if waiting {
                            self.tr(Text::BindingPrompt).into()
                        } else {
                            bindings.key(pad).to_uppercase()
                        };
                        button(&self.fonts, p, id, pad.name(), false)
                            .min_w_0()
                            .px_3()
                            .h(px(40.))
                            .justify_between()
                            .text_xs()
                            .when(waiting, |button| {
                                button
                                    .bg(rgb(p.tonal))
                                    .border_color(rgb(p.primary))
                                    .text_color(rgb(p.on_tonal))
                            })
                            .when(
                                conflicts.iter().any(|(button, _)| *button == pad),
                                |button| {
                                    button
                                        .border_color(rgb(p.on_error))
                                        .bg(rgb(p.error))
                                        .text_color(rgb(p.on_error))
                                },
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font(self.fonts.mono())
                                    .child(key),
                            )
                            .debug_selector(move || id.into())
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.release_buttons(cx);
                                view.rebinding = Some(pad);
                                view.notice = None;
                                // Capture Enter/Space without activating the focused button again.
                                window.focus(&view.settings_focus, cx);
                                cx.notify();
                            }))
                    })),
            )
    }

    pub(super) fn binding_actions(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let shortcuts = self.settings_tab == SettingsTab::Shortcuts;
        let [apply, reset, cancel] = if shortcuts {
            ["shortcuts-apply", "shortcuts-reset", "shortcuts-cancel"]
        } else {
            ["bindings-apply", "bindings-reset", "bindings-cancel"]
        };
        div().flex().flex_wrap().gap_2().children(
            [
                (BindingAction::Apply, apply, Text::ApplyBindings),
                (BindingAction::Reset, reset, Text::ResetBindings),
                (BindingAction::Cancel, cancel, Text::CancelBindings),
            ]
            .into_iter()
            .map(|(action, id, text)| {
                button(
                    &self.fonts,
                    p,
                    id,
                    self.tr(text),
                    action == BindingAction::Apply,
                )
                .debug_selector(move || id.into())
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.rebinding = None;
                    view.shortcut_editor.recording = None;
                    view.notice = None;
                    match (shortcuts, action) {
                        (true, BindingAction::Apply) => view.apply_shortcuts(cx),
                        (true, BindingAction::Reset) => {
                            view.shortcut_editor.draft = Some(Default::default())
                        }
                        (true, BindingAction::Cancel) => view.shortcut_editor.draft = None,
                        (false, BindingAction::Apply) => view.apply_bindings(cx),
                        (false, BindingAction::Reset) => {
                            view.keybindings_draft = Some(KeyBindings::default())
                        }
                        (false, BindingAction::Cancel) => view.keybindings_draft = None,
                    }
                    cx.notify();
                }))
            }),
        )
    }

    pub(super) fn controller_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.setting_group(Text::KeyBindings, cx, |cx| self.keybindings_panel(cx)))
            .child(self.setting_group(Text::Turbo, cx, |cx| self.turbo_settings(cx)))
    }

    fn turbo_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(label(p, self.tr(Text::TurboHint)))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [
                        ("turbo-cross", PadButton::Cross),
                        ("turbo-square", PadButton::Square),
                        ("turbo-triangle", PadButton::Triangle),
                        ("turbo-circle", PadButton::Circle),
                        ("turbo-l1", PadButton::L1),
                        ("turbo-r1", PadButton::R1),
                        ("turbo-l2", PadButton::L2),
                        ("turbo-r2", PadButton::R2),
                    ]
                    .into_iter()
                    .map(|(id, pad)| {
                        button(
                            &self.fonts,
                            p,
                            id,
                            format!(
                                "{} · {}",
                                pad.name(),
                                self.preferences.keybindings.key(pad).to_uppercase()
                            ),
                            false,
                        )
                        .when(self.preferences.turbo_buttons & pad.mask() != 0, |button| {
                            button
                                .bg(rgb(p.tonal))
                                .border_color(rgb(p.primary))
                                .text_color(rgb(p.on_tonal))
                        })
                        .debug_selector(move || id.into())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.set_turbo(
                                view.preferences.turbo_buttons ^ pad.mask(),
                                view.preferences.turbo_hz,
                                cx,
                            );
                        }))
                    }),
                ),
            )
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::TurboRate)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(TURBO_CHOICES.into_iter().map(|(hz, id, text)| {
                        button(&self.fonts, p, id, text, false)
                            .when(self.preferences.turbo_hz == hz, |button| {
                                button
                                    .bg(rgb(p.tonal))
                                    .border_color(rgb(p.primary))
                                    .text_color(rgb(p.on_tonal))
                            })
                            .debug_selector(move || id.into())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.set_turbo(view.preferences.turbo_buttons, hz, cx);
                            }))
                    })),
            )
            .child(label(p, self.tr(Text::TurboRateHint)))
            .child(
                button(
                    &self.fonts,
                    p,
                    "turbo-clear",
                    self.tr(Text::TurboClear),
                    false,
                )
                .on_click(
                    cx.listener(|view, _, _, cx| view.set_turbo(0, view.preferences.turbo_hz, cx)),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_descriptors_keep_existing_order_ids_and_rates() {
        assert_eq!(BINDINGS.map(|(pad, _)| pad), PadButton::ALL);
        for (pad, id) in BINDINGS {
            assert_eq!(id, format!("binding-{}", pad as u8));
        }
        assert_eq!(TURBO_CHOICES.map(|(hz, _, _)| hz), TURBO_RATES);
        for (hz, id, text) in TURBO_CHOICES {
            assert_eq!(id, format!("turbo-rate-{hz}"));
            assert_eq!(text, format!("{hz} Hz"));
        }
    }
}

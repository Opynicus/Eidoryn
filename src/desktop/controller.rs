use super::{
    Desktop,
    components::{button, label},
};
use eidoryn_psx::{
    i18n::Text,
    keybindings::{KeyBindings, PadButton},
    systems::ps1::{
        input::{TURBO_RATES, TurboConfig},
        runner::Command,
    },
};
use gpui::{Context, KeyDownEvent, Window, div, prelude::*, rgb};

impl Desktop {
    pub(super) fn capture_binding(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if bindings.validate().is_err() {
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
        let conflicts = PadButton::ALL
            .into_iter()
            .filter(|&pad| bindings.conflict(pad).is_some())
            .map(|pad| format!("{} · {}", pad.name(), bindings.key(pad).to_uppercase()))
            .collect::<Vec<_>>()
            .join(" / ");
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::KeyBindings)),
            )
            .child(label(p, self.tr(Text::KeyBindingsHint)))
            .child(label(p, self.tr(Text::BindingCancelHint)))
            .when(!conflicts.is_empty(), |panel| {
                panel.child(
                    label(
                        p,
                        format!("{}: {conflicts}", self.tr(Text::BindingConflict)),
                    )
                    .debug_selector(|| "bindings-conflict".into())
                    .text_color(rgb(p.error)),
                )
            })
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .children(PadButton::ALL.into_iter().map(|pad| {
                        let id = format!("binding-{}", pad as u8);
                        let waiting = self.rebinding == Some(pad);
                        let key = if waiting {
                            self.tr(Text::BindingPrompt).into()
                        } else {
                            bindings.key(pad).to_uppercase()
                        };
                        button(
                            &self.fonts,
                            p,
                            gpui::ElementId::Name(id.clone().into()),
                            format!("{} · {key}", pad.name()),
                            waiting,
                        )
                        .px_2()
                        .text_xs()
                        .when(bindings.conflict(pad).is_some(), |button| {
                            button.border_color(rgb(p.error))
                        })
                        .debug_selector(move || id.clone())
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.release_buttons(cx);
                                view.rebinding = Some(pad);
                                view.notice = None;
                                // Capture Enter/Space without activating the focused button again.
                                window.focus(&view.focus);
                                cx.notify();
                            },
                        ))
                    })),
            )
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [
                        ("bindings-apply", Text::ApplyBindings),
                        ("bindings-reset", Text::ResetBindings),
                        ("bindings-cancel", Text::CancelBindings),
                    ]
                    .into_iter()
                    .map(|(id, text)| {
                        button(&self.fonts, p, id, self.tr(text), id == "bindings-apply")
                            .debug_selector(move || id.into())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.rebinding = None;
                                view.notice = None;
                                match id {
                                    "bindings-apply" => view.apply_bindings(cx),
                                    "bindings-reset" => {
                                        view.keybindings_draft = Some(KeyBindings::default());
                                    }
                                    _ => view.keybindings_draft = None,
                                }
                                cx.notify();
                            }))
                    }),
                ),
            )
    }

    pub(super) fn controller_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.keybindings_panel(cx))
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Turbo)),
            )
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
                            self.preferences.turbo_buttons & pad.mask() != 0,
                        )
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
                    .children(TURBO_RATES.into_iter().map(|hz| {
                        button(
                            &self.fonts,
                            p,
                            gpui::ElementId::Name(format!("turbo-rate-{hz}").into()),
                            format!("{hz} Hz"),
                            self.preferences.turbo_hz == hz,
                        )
                        .debug_selector(move || format!("turbo-rate-{hz}"))
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

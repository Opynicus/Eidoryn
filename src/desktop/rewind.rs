use super::{
    Desktop,
    components::{button, label},
    graphics::switch,
};
use eidoryn_psx::{
    i18n::Text,
    systems::ps1::runner::{Command, MAX_REWIND_SECONDS, MIN_REWIND_SECONDS},
};
use gpui::{Context, MouseButton, div, prelude::*};

pub(super) enum RewindSource {
    Keyboard = 1,
    Mouse = 2,
}

impl Desktop {
    pub(super) fn hold_rewind(&mut self, source: RewindSource, held: bool, cx: &mut Context<Self>) {
        if held && !self.snapshot.rewind_enabled {
            return;
        }
        let was_held = self.rewind_sources != 0;
        if held {
            self.rewind_sources |= source as u8;
        } else {
            self.rewind_sources &= !(source as u8);
        }
        let active = self.rewind_sources != 0;
        if active != was_held {
            self.command(Command::SetRewinding(active), cx);
        }
    }

    pub(super) fn stop_rewind(&mut self, cx: &mut Context<Self>) {
        if self.rewind_sources != 0 || self.snapshot.rewinding {
            self.rewind_sources = 0;
            self.command(Command::SetRewinding(false), cx);
        }
    }

    pub(super) fn rewind_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let enabled = self.snapshot.rewind_enabled;
        let available = enabled && (self.snapshot.rewind_states > 0 || self.snapshot.rewinding);
        let status = if self.snapshot.rewinding {
            self.tr(Text::Rewinding).to_owned()
        } else if !enabled {
            self.tr(Text::RewindDisabled).to_owned()
        } else if self.snapshot.rewind_states == 0 {
            self.tr(Text::RewindBuffering).to_owned()
        } else {
            format!(
                "{} {:.1} s",
                self.tr(Text::RewindAvailable),
                self.snapshot.rewind_seconds
            )
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                button(
                    &self.fonts,
                    p,
                    "rewind-hold",
                    self.tr(Text::RewindHold),
                    self.snapshot.rewinding,
                )
                .debug_selector(|| "rewind-hold".into())
                .tab_index(-1)
                .when(!available, |button| button.opacity(0.45).cursor_default())
                .when(available, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            window.focus(&view.focus, cx);
                            view.hold_rewind(RewindSource::Mouse, true, cx);
                            cx.stop_propagation();
                        }),
                    )
                })
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|view, _, _, cx| {
                        view.hold_rewind(RewindSource::Mouse, false, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|view, _, _, cx| {
                        view.hold_rewind(RewindSource::Mouse, false, cx);
                    }),
                ),
            )
            .child(label(p, status).debug_selector(|| "rewind-status".into()))
    }

    pub(super) fn rewind_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let seconds = self.preferences.rewind_seconds;
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                switch(
                    &self.fonts,
                    p,
                    "rewind-enabled",
                    self.tr(Text::EnableRewind),
                    self.tr(Text::RewindSettingsHint),
                    self.preferences.rewind_enabled,
                    true,
                )
                .debug_selector(|| "rewind-enabled".into())
                .on_click(cx.listener(|view, _, _, cx| {
                    view.stop_rewind(cx);
                    let enabled = !view.preferences.rewind_enabled;
                    match view.runner.send(Command::SetRewindEnabled(enabled)) {
                        Ok(()) => {
                            view.preferences.rewind_enabled = enabled;
                            view.notice = None;
                            view.save_preferences();
                        }
                        Err(error) => view.notice = Some(error),
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::RewindDuration)),
            )
            .child(label(p, self.tr(Text::RewindDurationHint)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        label(p, format!("{seconds} s"))
                            .debug_selector(|| "rewind-duration".into()),
                    )
                    .children(
                        [
                            ("rewind-duration-decrease", Text::RewindShorter, -5),
                            ("rewind-duration-increase", Text::RewindLonger, 5),
                        ]
                        .into_iter()
                        .map(|(id, title, change)| {
                            let next = seconds
                                .saturating_add_signed(change)
                                .clamp(MIN_REWIND_SECONDS, MAX_REWIND_SECONDS);
                            button(&self.fonts, p, id, self.tr(title), false)
                                .debug_selector(move || id.into())
                                .when(next == seconds, |button| {
                                    button.opacity(0.45).cursor_default().tab_index(-1)
                                })
                                .when(next != seconds, |button| {
                                    button.on_click(cx.listener(move |view, _, _, cx| {
                                        view.stop_rewind(cx);
                                        match view.runner.send(Command::SetRewindSeconds(next)) {
                                            Ok(()) => {
                                                view.preferences.rewind_seconds = next;
                                                view.notice = None;
                                                view.save_preferences();
                                            }
                                            Err(error) => view.notice = Some(error),
                                        }
                                        cx.notify();
                                    }))
                                })
                        }),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

use super::{
    Desktop,
    components::{button, label},
};
use eidoryn_psx::{
    i18n::Text,
    systems::ps1::{
        input::{TURBO_RATES, TurboConfig},
        runner::Command,
    },
};
use gpui::{Context, div, prelude::*};

impl Desktop {
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

    pub(super) fn controller_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Turbo)),
            )
            .child(label(p, self.tr(Text::TurboHint)))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [
                        ("turbo-cross", "Cross · X", 14),
                        ("turbo-square", "Square · Z", 15),
                        ("turbo-triangle", "Triangle · S", 12),
                        ("turbo-circle", "Circle · D", 13),
                        ("turbo-l1", "L1 · Q", 10),
                        ("turbo-r1", "R1 · E", 11),
                        ("turbo-l2", "L2 · A", 8),
                        ("turbo-r2", "R2 · F", 9),
                    ]
                    .into_iter()
                    .map(|(id, name, bit)| {
                        button(
                            &self.fonts,
                            p,
                            id,
                            name,
                            self.preferences.turbo_buttons & (1 << bit) != 0,
                        )
                        .debug_selector(move || id.into())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.set_turbo(
                                view.preferences.turbo_buttons ^ (1 << bit),
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

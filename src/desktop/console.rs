use super::{
    Desktop,
    components::{action, button, card, heading, icon_button, label, pair},
    icons::icon,
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::systems::ps1::runner::{Command, ExecutionState};
use gpui::{Context, Window, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn console_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let width = f32::from(window.viewport_size().width);
        let stacked = width <= 1250.;
        let content_width = (width - if stacked { 72. } else { 224. } - 72.).min(1608.);
        let display_width = content_width - if stacked { 0. } else { 320. };
        let height = (display_width - 34.).clamp(0., 560.) * 0.75 + 32.;
        let display = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(self.display_card(height, cx))
            .child(self.shortcuts());
        let details = if stacked {
            div()
                .flex()
                .gap_5()
                .child(self.session_card(cx).flex_1().min_w_0())
                .child(self.memory_card(cx).flex_1().min_w_0())
        } else {
            div()
                .w(px(300.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_4()
                .child(self.session_card(cx))
                .child(self.memory_card(cx))
        };
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(self.console_header(cx))
            .child(if stacked {
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(display)
                    .child(details)
            } else {
                div()
                    .flex()
                    .items_start()
                    .gap_5()
                    .child(display)
                    .child(details)
            })
    }

    fn console_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let fonts = &self.fonts;
        div()
            .flex()
            .items_end()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font(self.fonts.semibold())
                            .text_color(rgb(p.muted))
                            .child(self.tr(Text::ConsoleEyebrow)),
                    )
                    .child(
                        div()
                            .text_size(px(26.))
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::Welcome)),
                    )
                    .child(label(p, self.tr(Text::WelcomeHint)).text_size(px(13.))),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap_2()
                    .child(
                        action(
                            fonts,
                            p,
                            "open-bios",
                            self.tr(Text::ChooseBios),
                            Some("plus"),
                            false,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.open(true, cx))),
                    )
                    .child(
                        action(
                            fonts,
                            p,
                            "open-disc",
                            self.tr(Text::OpenDisc),
                            Some("disc"),
                            true,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.open_disc(false, cx))),
                    ),
            )
    }

    fn display_card(&self, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let s = &self.snapshot;
        let speed = self.tr(Text::Speed);
        let (status, dot) = match s.execution_state() {
            ExecutionState::Stopped => (self.tr(Text::Stopped), p.on_error),
            ExecutionState::Running => (self.tr(Text::Running), p.on_success),
            ExecutionState::Paused => (self.tr(Text::Paused), p.muted),
        };
        card(p)
            .debug_selector(|| "display-card".into())
            .overflow_hidden()
            .child(
                div()
                    .h(px(50.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::Display)),
                    )
                    .child(div().flex_1())
                    .child(label(
                        p,
                        format!("{} × {}   ·   4:3", s.dimensions.0, s.dimensions.1),
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "display-stage".into())
                    .p_4()
                    .h(px(height))
                    .bg(rgb(p.video_background))
                    .child(self.video()),
            )
            .child(
                div()
                    .p_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font(self.fonts.medium())
                                    .truncate()
                                    .child(self.display_title().to_owned()),
                            )
                            .child(label(
                                p,
                                format!(
                                    "PlayStation  ·  {:.1} FPS  ·  {:.0}% {speed}",
                                    s.video_fps,
                                    100. * s.video_fps / s.target_fps
                                ),
                            )),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(p.border))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(rgb(dot)))
                            .child(label(p, status)),
                    ),
            )
            .child(self.transport(false, cx))
    }

    pub(super) fn transport(&self, game: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let running = self.snapshot.running;
        div()
            .debug_selector(|| "transport".into())
            .p_3()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                action(
                    &self.fonts,
                    p,
                    "transport-run",
                    if running {
                        self.tr(Text::Pause)
                    } else {
                        self.tr(Text::Run)
                    },
                    Some(if running { "pause" } else { "play" }),
                    true,
                )
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::Toggle, cx))),
            )
            .child(
                icon_button(p, "transport-reset", "reset", self.tr(Text::Reset))
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Reset, cx))),
            )
            .child(div().w(px(1.)).h(px(24.)).mx_1().bg(rgb(p.border)))
            .child(self.crt_control(cx))
            .children(game.then(|| {
                action(
                    &self.fonts,
                    p,
                    "transport-graphics",
                    self.tr(Text::Graphics),
                    Some("settings"),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.open_settings(false, cx)))
            }))
            .child(div().flex_1().min_w_0())
            .child(self.audio_controls(cx))
            .child(if game {
                action(
                    &self.fonts,
                    p,
                    "transport-fullscreen",
                    self.tr(Text::Fullscreen),
                    Some("expand"),
                    false,
                )
                .on_click(cx.listener(|view, _, window, cx| {
                    window.toggle_fullscreen();
                    window.focus(&view.focus, cx);
                    cx.notify();
                }))
            } else {
                action(
                    &self.fonts,
                    p,
                    "transport-game",
                    self.tr(Text::GameView),
                    Some("expand"),
                    false,
                )
                .on_click(cx.listener(|view, _, window, cx| view.toggle_view(window, cx)))
            })
    }

    fn shortcuts(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .text_xs()
            .text_color(rgb(p.muted))
            .children(
                [
                    ("F5", self.tr(Text::RunPause)),
                    ("F6", self.tr(Text::GameView)),
                    ("F7", "CRT"),
                    ("F8", self.tr(Text::Graphics)),
                ]
                .into_iter()
                .map(|(key, description)| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.surface_variant))
                                .child(key),
                        )
                        .child(description)
                }),
            )
    }

    fn session_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let bios = self
            .bios
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || self.tr(Text::NotSelected).into(),
                |name| name.to_string_lossy().into_owned(),
            );
        card(p)
            .debug_selector(|| "session-card".into())
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::Session),
                "PLAYSTATION",
            ))
            .child(
                div()
                    .p(px(10.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.surface_variant))
                    .child(
                        div()
                            .size(px(34.))
                            .rounded_md()
                            .bg(rgb(p.tonal_hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("disc", p.text)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font(self.fonts.medium())
                                    .truncate()
                                    .child(self.display_title().to_owned()),
                            )
                            .child(label(
                                p,
                                if self.snapshot.disc_mounted {
                                    self.tr(Text::DiscSession)
                                } else {
                                    self.tr(Text::LocalSession)
                                },
                            )),
                    ),
            )
            .child(pair(p, "BIOS", bios))
            .child(pair(p, self.tr(Text::Renderer), self.renderer_label()))
            .child(pair(
                p,
                self.tr(Text::SectorsRead),
                self.snapshot.cdrom_reads.to_string(),
            ))
            .child(
                action(
                    &self.fonts,
                    p,
                    "open-exe",
                    self.tr(Text::OpenExe),
                    Some("plus"),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.open(false, cx))),
            )
            .child(
                button(&self.fonts, p, "fast-boot", self.tr(Text::FastBoot), false)
                    .on_click(cx.listener(|view, _, _, cx| view.open_disc(true, cx))),
            )
            .child(
                button(&self.fonts, p, "demo", self.tr(Text::Demo), false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Demo, cx))),
            )
    }

    fn memory_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "memory-card".into())
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::MemoryCards),
                "MEMORY CARD",
            ))
            .children(self.snapshot.cards.iter().enumerate().map(|(slot, card)| {
                let (name, state) = card.as_ref().map_or_else(
                    || {
                        (
                            self.tr(Text::Disconnected).to_owned(),
                            self.tr(Text::Disconnected),
                        )
                    },
                    |card| {
                        let name = card
                            .path
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .map_or_else(
                                || self.tr(Text::TemporaryCard).into(),
                                |name| name.to_string_lossy().into_owned(),
                            );
                        (
                            name,
                            if card.pending {
                                self.tr(Text::Saving)
                            } else {
                                self.tr(Text::Synced)
                            },
                        )
                    },
                );
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded_md()
                            .bg(rgb(p.surface_variant))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(format!("{:02}", slot + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().child(format!(
                                "{} {}",
                                self.tr(Text::Slot),
                                slot + 1
                            )))
                            .child(label(p, name).truncate()),
                    )
                    .child(label(p, state))
            }))
            .child(
                button(
                    &self.fonts,
                    p,
                    "flush-cards",
                    self.tr(Text::SyncCards),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::FlushCards, cx))),
            )
            .child(label(p, self.tr(Text::SaveHint)))
    }
}

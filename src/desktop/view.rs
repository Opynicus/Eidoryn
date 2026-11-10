use super::{
    Desktop,
    components::{button, card, label},
};
use eidoryn_psx::{app, systems::ps1::runner::Command};
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn game_display(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.video_background))
            .text_color(rgb(p.text))
            .font_family("sans-serif")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .child(div().flex_1().min_h_0().child(self.video()))
            .children(self.graphics.open.then(|| self.graphics_panel(cx)))
            .children(self.notice.clone().or_else(|| self.snapshot.message.clone()).map(|message| {
                div().px_4().py_2().flex_shrink_0().bg(rgb(p.error))
                    .text_sm().text_color(rgb(p.on_error)).child(message)
            }))
            .child(
                div().p_3().gap_2().flex().items_center().flex_shrink_0().bg(rgb(p.surface))
                    .child(button(p, "game-run", if self.snapshot.running { "Pause" } else { "Run" }, true)
                        .on_click(cx.listener(|view, _, _, cx| view.command(Command::Toggle, cx))))
                    .child(self.audio_controls(cx))
                    .child(self.crt_control(cx))
                    .child(self.graphics_button(cx))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().truncate().text_sm().child(self.snapshot.title.clone()))
                        .child(label(p, format!("{:.1} FPS  ·  {:.0}% speed  ·  Arrows: move  ·  X: cross  ·  Space: pause", self.snapshot.video_fps, 100.0 * self.snapshot.video_fps / self.snapshot.target_fps))))
                    .child(button(p, "inspector", "Inspector  ·  F6", false)
                        .on_click(cx.listener(|view, _, window, cx| view.toggle_view(window, cx))))
                    .child(button(p, "game-fullscreen", if window.is_fullscreen() { "Exit fullscreen  ·  F11" } else { "Fullscreen  ·  F11" }, false)
                        .on_click(cx.listener(|_, _, window, cx| { window.toggle_fullscreen(); cx.notify(); })))
            )
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = &mut self.probe {
            probe.record(self.snapshot.video_frames, "render");
        }
        let p = &self.themes.active().palette;
        if self.snapshot.close_ready {
            window.remove_window();
        }
        if self.game_view || window.is_fullscreen() {
            return self.game_display(window, cx);
        }
        let message = self
            .notice
            .clone()
            .or_else(|| self.snapshot.message.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(rgb(p.background))
            .text_color(rgb(p.text))
            .font_family("sans-serif")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .child(self.header(window, cx))
            .child(self.toolbar(cx))
            .children(self.appearance_open.then(|| self.appearance(cx)))
            .children(self.graphics.open.then(|| self.graphics_panel(cx)))
            .children(message.map(|message| {
                div()
                    .px_4()
                    .py_3()
                    .rounded_xl()
                    .flex_shrink_0()
                    .bg(rgb(p.error))
                    .text_color(rgb(p.on_error))
                    .text_sm()
                    .child(message)
            }))
            .child(self.workspace(window, cx))
            .child(self.footer())
    }
}

impl Desktop {
    fn header(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .items_center()
            .gap_4()
            .flex_shrink_0()
            .child(
                div()
                    .size(px(52.))
                    .rounded_2xl()
                    .bg(rgb(p.primary))
                    .text_color(rgb(p.on_primary))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_xl()
                    .child(
                        gpui::img(std::sync::Arc::clone(&self.icon))
                            .size_full()
                            .rounded_2xl(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(app::NAME),
                    )
                    .child(label(p, "PlayStation emulator")),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(p.border))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(format!("v{}", app::VERSION)),
            )
            .child(self.audio_controls(cx))
            .child(self.graphics_button(cx))
            .child(
                button(
                    p,
                    "appearance",
                    format!("Theme · {}", self.themes.active().name),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.appearance_open = !view.appearance_open;
                    view.graphics.open = false;
                    cx.notify();
                })),
            )
            .child(
                button(
                    p,
                    "maximize",
                    if window.is_maximized() {
                        "Restore"
                    } else {
                        "Maximize"
                    },
                    false,
                )
                .on_click(cx.listener(|_, _, window, cx| {
                    window.zoom_window();
                    cx.notify();
                })),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let snapshot = &self.snapshot;
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(
                button(
                    p,
                    "run",
                    if snapshot.running {
                        "Pause  ·  Space"
                    } else {
                        "Run  ·  Space"
                    },
                    true,
                )
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::Toggle, cx))),
            )
            .child(
                button(p, "step", "Step  ·  F10", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Step, cx))),
            )
            .child(
                button(p, "slice", "Slice  ·  F9", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Slice, cx))),
            )
            .child(
                button(p, "reset", "Reset", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Reset, cx))),
            )
            .child(div().flex_1())
            .child(
                button(p, "demo", "Demo", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Demo, cx))),
            )
            .child(
                button(p, "exe", "Open EXE", false)
                    .on_click(cx.listener(|_, _, _, cx| Self::open(false, cx))),
            )
            .child(
                button(p, "bios", "Open BIOS", false)
                    .on_click(cx.listener(|_, _, _, cx| Self::open(true, cx))),
            )
    }

    fn workspace(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(self.display_card(cx))
                    .child(self.media(cx))
                    .child(self.disassembly(window.viewport_size().height < px(920.))),
            )
            .child(self.registers(window.viewport_size().height < px(920.), cx))
    }

    #[allow(clippy::too_many_lines)] // One video card: toolbar, canvas and caption.
    fn display_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let snapshot = &self.snapshot;
        let (status, status_surface, status_ink) = if snapshot.message.is_some() {
            ("Stopped", p.error, p.on_error)
        } else if snapshot.running {
            ("Running", p.success, p.on_success)
        } else {
            ("Paused", p.tonal, p.on_tonal)
        };
        card(p)
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Display"),
                    )
                    .child(label(
                        p,
                        format!("{} × {}", snapshot.dimensions.0, snapshot.dimensions.1),
                    ))
                    .child(
                        button(p, "game-view", "Enlarge  ·  F6", false).on_click(cx.listener(
                            |view, _, window, cx| {
                                view.toggle_view(window, cx);
                            },
                        )),
                    )
                    .child(
                        button(p, "fullscreen", "Fullscreen  ·  F11", false).on_click(cx.listener(
                            |_, _, window, cx| {
                                window.toggle_fullscreen();
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .text_xs()
                            .bg(rgb(status_surface))
                            .text_color(rgb(status_ink))
                            .child(status),
                    ),
            )
            .child(
                div()
                    .mx_3()
                    .flex_1()
                    .min_h_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .p_3()
                    .bg(rgb(p.video_background))
                    .child(self.video()),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_shrink_0()
                    .text_xs()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(p.muted))
                            .child(snapshot.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(p.primary))
                            .child(format!("{:.1} FPS", snapshot.video_fps)),
                    )
                    .child(self.crt_control(cx)),
            )
    }

    fn footer(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        let snapshot = &self.snapshot;
        div()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(rgb(p.muted))
            .flex_shrink_0()
            .child(format!(
                "{:.1} / {:.1} FPS   ·   {:.0}% speed   ·   {} GPU   ·   {} instructions",
                snapshot.video_fps,
                snapshot.target_fps,
                100.0 * snapshot.video_fps / snapshot.target_fps,
                snapshot.renderer.label(),
                snapshot.cpu.instructions
            ))
            .child("F6 · Game    F7 · CRT    F8 · Graphics    F11 · Fullscreen")
    }
}

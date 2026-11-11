use super::{
    Desktop,
    components::{action, button, card, heading, icon_button, label, pair},
    icons::icon,
};
use eidoryn_psx::systems::ps1::runner::Command;
use gpui::{Context, Window, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn console_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let stacked = window.viewport_size().width < px(1400.);
        let height = (f32::from(window.viewport_size().height) - 440.).clamp(240., 390.);
        let display = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_3()
            .child(self.display_card(height))
            .child(self.transport(false, cx))
            .child(self.shortcuts());
        let details = if stacked {
            div()
                .flex()
                .gap_4()
                .child(self.session_card(cx).flex_1().min_w_0())
                .child(self.memory_card(cx).flex_1().min_w_0())
        } else {
            div()
                .w(px(292.))
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
            .gap_5()
            .child(
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
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(p.primary))
                                    .child("PLAYSTATION / 工作区"),
                            )
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("欢迎回来，准备开始。"),
                            )
                            .child(label(p, "把注意力留给游戏，其余交给控制台。")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .gap_2()
                            .child(
                                action(p, "open-bios", "选择 BIOS", Some("plus"), false)
                                    .on_click(cx.listener(|_, _, _, cx| Self::open(true, cx))),
                            )
                            .child(
                                action(p, "open-disc", "打开光盘镜像", Some("disc"), true)
                                    .on_click(
                                        cx.listener(|view, _, _, cx| view.open_disc(false, cx)),
                                    ),
                            ),
                    ),
            )
            .child(if stacked {
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(display)
                    .child(details)
            } else {
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(display)
                    .child(details)
            })
    }

    fn display_card(&self, height: f32) -> gpui::Div {
        let p = &self.themes.active().palette;
        let s = &self.snapshot;
        let (status, background, text) = if s.message.is_some() {
            ("已停止", p.error, p.on_error)
        } else if s.running {
            ("运行中", p.success, p.on_success)
        } else {
            ("已暂停", p.tonal, p.on_tonal)
        };
        card(p)
            .overflow_hidden()
            .child(
                div()
                    .h(px(52.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("screen", p.muted))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("画面输出"),
                    )
                    .child(label(p, "DISPLAY"))
                    .child(div().flex_1())
                    .child(label(
                        p,
                        format!("{} × {}   ·   4:3", s.dimensions.0, s.dimensions.1),
                    )),
            )
            .child(
                div()
                    .mx_3()
                    .p_3()
                    .h(px(height))
                    .rounded_lg()
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
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .truncate()
                                    .child(s.title.clone()),
                            )
                            .child(label(
                                p,
                                format!(
                                    "PlayStation  ·  {:.1} FPS  ·  {:.0}% 速度",
                                    s.video_fps,
                                    100. * s.video_fps / s.target_fps
                                ),
                            )),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(background))
                            .text_color(rgb(text))
                            .text_xs()
                            .child(format!("●  {status}")),
                    ),
            )
    }

    pub(super) fn transport(&self, game: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let running = self.snapshot.running;
        card(p)
            .p_3()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                action(
                    p,
                    "transport-run",
                    if running { "暂停" } else { "运行" },
                    Some(if running { "pause" } else { "play" }),
                    true,
                )
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::Toggle, cx))),
            )
            .child(
                icon_button(p, "transport-reset", "reset")
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Reset, cx))),
            )
            .child(self.crt_control(cx))
            .children(game.then(|| {
                action(p, "transport-graphics", "图形", Some("settings"), false)
                    .on_click(cx.listener(|view, _, _, cx| view.open_settings(false, cx)))
            }))
            .child(div().flex_1().min_w_0())
            .child(self.audio_controls(cx))
            .child(if game {
                action(p, "transport-fullscreen", "全屏", Some("expand"), false).on_click(
                    cx.listener(|_, _, window, cx| {
                        window.toggle_fullscreen();
                        cx.notify();
                    }),
                )
            } else {
                action(p, "transport-game", "游戏视图", Some("expand"), false)
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
                    ("Space", "运行 / 暂停"),
                    ("F6", "游戏视图"),
                    ("F7", "CRT"),
                    ("F8", "图形"),
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
                                .py_1()
                                .rounded_md()
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
                || "未选择".into(),
                |name| name.to_string_lossy().into_owned(),
            );
        card(p)
            .p_4()
            .gap_3()
            .child(heading(p, "当前会话", "PLAYSTATION"))
            .child(
                div()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_lg()
                    .bg(rgb(p.surface_variant))
                    .child(
                        div()
                            .size(px(38.))
                            .rounded_lg()
                            .bg(rgb(p.tonal))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("disc", p.primary)),
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
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .truncate()
                                    .child(self.snapshot.title.clone()),
                            )
                            .child(label(
                                p,
                                if self.snapshot.disc_mounted {
                                    "PlayStation · 光盘镜像"
                                } else {
                                    "PlayStation · 当前会话"
                                },
                            )),
                    ),
            )
            .child(pair(p, "BIOS", bios))
            .child(pair(p, "渲染模式", self.snapshot.renderer.label()))
            .child(pair(p, "读取扇区", self.snapshot.cdrom_reads.to_string()))
            .child(
                action(p, "open-exe", "打开 EXE", Some("plus"), false)
                    .on_click(cx.listener(|_, _, _, cx| Self::open(false, cx))),
            )
            .child(
                button(p, "fast-boot", "快速启动（自制程序）", false)
                    .on_click(cx.listener(|view, _, _, cx| view.open_disc(true, cx))),
            )
            .child(
                button(p, "demo", "启动内置演示", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::Demo, cx))),
            )
    }

    fn memory_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .p_4()
            .gap_3()
            .child(heading(p, "记忆卡", "MEMORY CARD"))
            .children(self.snapshot.cards.iter().enumerate().map(|(slot, card)| {
                let (name, state) = card.as_ref().map_or_else(
                    || ("未连接".to_owned(), "未连接"),
                    |card| {
                        let name = card
                            .path
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .map_or_else(
                                || "临时记忆卡".into(),
                                |name| name.to_string_lossy().into_owned(),
                            );
                        (
                            name,
                            if card.pending {
                                "保存中…"
                            } else {
                                "已同步"
                            },
                        )
                    },
                );
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
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
                            .child(div().text_sm().child(format!("卡槽 {}", slot + 1)))
                            .child(label(p, name).truncate()),
                    )
                    .child(label(p, state))
            }))
            .child(
                button(p, "flush-cards", "同步卡数据", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::FlushCards, cx))),
            )
            .child(label(p, "请在游戏内完成存档；同步只会写入已有卡数据。"))
    }
}

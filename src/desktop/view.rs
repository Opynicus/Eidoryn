use super::{
    Desktop, WorkspacePage,
    components::{action, button, icon_button, label},
    icons::icon,
};
use eidoryn_psx::{app, theme::Palette};
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb, rgba};

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = &mut self.probe {
            probe.record(self.snapshot.video_frames, "render");
        }
        if self.snapshot.close_ready {
            window.remove_window();
        }
        let p = &self.themes.active().palette;
        let game = self.game_view || window.is_fullscreen();
        let body = if game {
            self.game_display(window, cx)
        } else {
            self.workspace(window, cx)
        };
        let mut root = div()
            .relative()
            .size_full()
            .bg(rgb(p.background))
            .text_color(rgb(p.text))
            .font_family("sans-serif")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .child(body);
        if self.graphics.open {
            root = root
                .child(
                    div()
                        .id("settings-backdrop")
                        .occlude()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .bg(rgba(0x00000099))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.graphics.open = false;
                            cx.notify();
                        })),
                )
                .child(self.settings_drawer(cx));
        }
        root
    }
}

impl Desktop {
    fn workspace(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let compact = window.viewport_size().width <= px(1250.);
        let page = match self.page {
            WorkspacePage::Console => self.console_page(window, cx),
            WorkspacePage::Debug => self.debug_page(window, cx),
        };
        div()
            .size_full()
            .flex()
            .child(self.sidebar(compact, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.topbar(window, cx))
                    .child(
                        div()
                            .id("workspace-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(if compact { px(26.) } else { px(38.) })
                            .py_6()
                            .child(
                                div()
                                    .w_full()
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .children(self.notice_banner())
                                    .child(page),
                            ),
                    )
                    .child(
                        div()
                            .h(px(30.))
                            .px_6()
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "{:.1} / {:.1} FPS  ·  {:.0}% 速度  ·  {}",
                                self.snapshot.video_fps,
                                self.snapshot.target_fps,
                                100. * self.snapshot.video_fps / self.snapshot.target_fps,
                                self.snapshot.renderer.label()
                            ))
                            .child("F6 游戏视图  ·  F7 CRT  ·  F8 图形  ·  F11 全屏"),
                    ),
            )
    }

    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let console = nav_item(
            p,
            "nav-console",
            "grid",
            "控制台",
            compact,
            self.page == WorkspacePage::Console,
        )
        .on_click(
            cx.listener(|view, _, window, cx| view.show_page(WorkspacePage::Console, window, cx)),
        );
        let game = nav_item(p, "nav-game", "screen", "游戏视图", compact, false)
            .on_click(cx.listener(|view, _, window, cx| view.toggle_view(window, cx)));
        let debug = nav_item(
            p,
            "nav-debug",
            "code",
            "调试器",
            compact,
            self.page == WorkspacePage::Debug,
        )
        .on_click(
            cx.listener(|view, _, window, cx| view.show_page(WorkspacePage::Debug, window, cx)),
        );
        div()
            .w(if compact { px(76.) } else { px(218.) })
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_5()
            .bg(rgb(p.surface))
            .border_r_1()
            .border_color(rgb(p.border))
            .child(self.sidebar_brand(compact))
            .when(!compact, |element| {
                element.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child("工作区"),
                )
            })
            .child(console)
            .child(game)
            .child(debug)
            .child(div().h(px(1.)).my_4().bg(rgb(p.border)))
            .when(!compact, |element| {
                element.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child("模拟平台"),
                )
            })
            .child(platform(p, "PS", "PlayStation", "当前", compact, true))
            .child(platform(
                p,
                "GB",
                "Game Boy Advance",
                "未支持",
                compact,
                false,
            ))
            .child(platform(p, "P2", "PlayStation 2", "未支持", compact, false))
            .child(div().flex_1())
            .child(
                nav_item(
                    p,
                    "nav-settings",
                    "settings",
                    "图形与外观",
                    compact,
                    self.graphics.open,
                )
                .on_click(cx.listener(|view, _, _, cx| view.open_settings(false, cx))),
            )
            .when(!compact, |element| {
                element.child(
                    div()
                        .px_3()
                        .pt_4()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(format!("Eidoryn  ·  v{}", app::VERSION)),
                )
            })
    }

    fn sidebar_brand(&self, compact: bool) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_1()
            .mb_6()
            .when(compact, gpui::Styled::justify_center)
            .child(
                gpui::img(std::sync::Arc::clone(&self.icon))
                    .size(px(38.))
                    .rounded_lg(),
            )
            .when(!compact, |element| {
                element.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(app::NAME),
                        )
                        .child(label(p, "THE PLAY SPACE")),
                )
            })
    }

    fn topbar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .h(px(66.))
            .px_6()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(label(p, "工作区   /"))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(match self.page {
                        WorkspacePage::Console => "控制台",
                        WorkspacePage::Debug => "调试器",
                    }),
            )
            .child(div().flex_1())
            .child(label(
                p,
                format!(
                    "{}  ·  {}",
                    self.themes.active().name,
                    self.snapshot.renderer.label()
                ),
            ))
            .child(
                button(
                    p,
                    "maximize",
                    if window.is_maximized() {
                        "还原"
                    } else {
                        "最大化"
                    },
                    false,
                )
                .on_click(cx.listener(|_, _, window, cx| {
                    window.zoom_window();
                    cx.notify();
                })),
            )
            .child(
                icon_button(p, "top-settings", "settings")
                    .on_click(cx.listener(|view, _, _, cx| view.open_settings(false, cx))),
            )
    }

    fn game_display(&self, _window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.video_background))
            .child(
                div()
                    .h(px(65.))
                    .px_6()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        action(p, "game-back", "返回控制台", Some("back"), false).on_click(
                            cx.listener(|view, _, window, cx| {
                                view.show_page(WorkspacePage::Console, window, cx);
                            }),
                        ),
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
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(self.snapshot.title.clone()),
                            )
                            .child(label(
                                p,
                                format!(
                                    "PLAYSTATION  ·  {:.1} FPS  ·  {:.0}% 速度",
                                    self.snapshot.video_fps,
                                    100. * self.snapshot.video_fps / self.snapshot.target_fps
                                ),
                            )),
                    ),
            )
            .children(self.notice_banner())
            .child(div().flex_1().min_h_0().px_6().py_3().child(self.video()))
            .child(
                div().px_6().pb_6().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(px(920.))
                        .child(self.transport(true, cx)),
                ),
            )
    }

    fn notice_banner(&self) -> Option<gpui::Div> {
        let p = &self.themes.active().palette;
        self.notice
            .clone()
            .or_else(|| self.snapshot.message.clone())
            .map(|message| {
                div()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(rgb(p.error))
                    .text_color(rgb(p.on_error))
                    .text_sm()
                    .child(message)
            })
    }

    fn settings_drawer(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = &self.themes.active().palette;
        div()
            .id("settings-drawer")
            .occlude()
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(400.))
            .flex()
            .flex_col()
            .bg(rgb(p.surface))
            .border_l_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .p_5()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("图形与外观"),
                            )
                            .child(label(p, "调整你的游玩体验")),
                    )
                    .child(
                        icon_button(p, "close-settings", "close").on_click(cx.listener(
                            |view, _, _, cx| {
                                view.graphics.open = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .pb_3()
                    .flex()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        button(p, "tab-graphics", "图形设置", !self.appearance_open).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.appearance_open = false;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(p, "tab-appearance", "外观主题", self.appearance_open).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.appearance_open = true;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .children(self.notice_banner())
                            .child(if self.appearance_open {
                                self.appearance(cx)
                            } else {
                                self.graphics_panel(cx)
                            }),
                    ),
            )
    }
}

fn nav_item(
    p: &Palette,
    id: &'static str,
    symbol: &'static str,
    text: &'static str,
    compact: bool,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.surface_variant;
    div()
        .id(id)
        .debug_selector(|| id.to_owned())
        .h(px(42.))
        .px_3()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_3()
        .rounded_lg()
        .bg(rgb(if selected { p.tonal } else { p.surface }))
        .text_color(rgb(if selected { p.text } else { p.muted }))
        .cursor_pointer()
        .when(compact, gpui::Styled::justify_center)
        .hover(move |style| style.bg(rgb(hover)))
        .child(icon(symbol, if selected { p.primary } else { p.muted }))
        .when(!compact, |element| {
            element.child(div().text_sm().child(text))
        })
}

fn platform(
    p: &Palette,
    short: &'static str,
    name: &'static str,
    state: &'static str,
    compact: bool,
    selected: bool,
) -> gpui::Div {
    div()
        .h(px(42.))
        .px_3()
        .flex()
        .items_center()
        .gap_3()
        .when(compact, gpui::Styled::justify_center)
        .child(
            div()
                .size(px(28.))
                .rounded_md()
                .bg(rgb(if selected { p.tonal } else { p.surface_variant }))
                .text_color(rgb(if selected { p.primary } else { p.muted }))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(gpui::FontWeight::BOLD)
                .child(short),
        )
        .when(!compact, |element| {
            element
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(if selected { p.text } else { p.muted }))
                        .child(name),
                )
                .child(label(p, state))
        })
}

use super::{
    Desktop, SettingsTab, WorkspacePage,
    components::{action, button, icon_button, label},
    icons::icon,
};
use eidoryn_psx::i18n::Text;
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
        let p = self.themes.active().palette;
        let game = self.game_view || window.is_fullscreen();
        self.sync_snapshot_mode(!game && self.page == WorkspacePage::Debug);
        self.sync_video(game || self.page == WorkspacePage::Console, window);
        let pure_video = window.is_fullscreen() && self.clean_fullscreen;
        let body = if pure_video {
            div()
                .relative()
                .size_full()
                .debug_selector(|| "pure-display".into())
                .bg(rgb(p.video_background))
                .child(self.video())
                .children(
                    self.notice_banner()
                        .map(|banner| banner.absolute().top_0().left_0().right_0()),
                )
        } else if game {
            self.game_display(window, cx)
        } else {
            self.workspace(window, cx)
        };
        let mut root = div()
            .id("desktop-root")
            .tab_group()
            .relative()
            .size_full()
            .bg(rgb(p.background))
            .text_color(rgb(p.text))
            .when(
                !pure_video
                    || self.graphics.open
                    || self.notice.is_some()
                    || self.snapshot.message.is_some(),
                |root| root.font(self.fonts.normal()),
            )
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::capture_binding))
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .on_click(cx.listener(|view, event, window, _| {
                if matches!(event, gpui::ClickEvent::Mouse(_)) {
                    window.focus(&view.focus);
                }
            }))
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
                        .on_click(
                            cx.listener(|view, _, window, cx| view.close_settings(window, cx)),
                        ),
                )
                .child(self.settings_drawer(cx));
        }
        root
    }
}

impl Desktop {
    fn workspace(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.themes.active().palette;
        let speed = self.tr(Text::Speed);
        let compact = window.viewport_size().width <= px(1250.);
        let page = match self.page {
            WorkspacePage::Console => self.console_page(window, cx),
            WorkspacePage::Debug => self.debug_page(window, cx),
        };
        div()
            .size_full()
            .flex()
            .child(self.cached_sidebar(compact, cx))
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
                                "{:.1} / {:.1} FPS  ·  {:.0}% {speed}  ·  {}",
                                self.snapshot.video_fps,
                                self.snapshot.target_fps,
                                100. * self.snapshot.video_fps / self.snapshot.target_fps,
                                self.renderer_label()
                            ))
                            .child(self.tr(Text::Shortcuts)),
                    ),
            )
    }

    pub(super) fn sidebar_content(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let console = nav_item(
            p,
            "nav-console",
            "grid",
            self.tr(Text::Console),
            compact,
            self.page == WorkspacePage::Console,
        )
        .on_click(
            cx.listener(|view, _, window, cx| view.show_page(WorkspacePage::Console, window, cx)),
        );
        let game = nav_item(
            p,
            "nav-game",
            "screen",
            self.tr(Text::GameView),
            compact,
            false,
        )
        .on_click(cx.listener(|view, _, window, cx| view.toggle_view(window, cx)));
        let debug = nav_item(
            p,
            "nav-debug",
            "code",
            self.tr(Text::Debugger),
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
                        .child(self.tr(Text::Workspace)),
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
                        .child(self.tr(Text::Platforms)),
                )
            })
            .child(self.platform_list(compact))
            .child(div().flex_1())
            .child(
                nav_item(
                    p,
                    "nav-settings",
                    "settings",
                    self.tr(Text::Settings),
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
                        .child(concat!("Eidoryn  ·  v", env!("CARGO_PKG_VERSION"))),
                )
            })
    }

    fn platform_list(&self, compact: bool) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .child(platform(
                &self.fonts,
                p,
                "PS",
                "PlayStation",
                self.tr(Text::Current),
                compact,
                true,
            ))
            .child(platform(
                &self.fonts,
                p,
                "GB",
                "Game Boy Advance",
                self.tr(Text::Unsupported),
                compact,
                false,
            ))
            .child(platform(
                &self.fonts,
                p,
                "P2",
                "PlayStation 2",
                self.tr(Text::Unsupported),
                compact,
                false,
            ))
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
            .children(
                self.icon
                    .get_or_init(|| match super::icons::app_icon() {
                        Ok(icon) => Some(icon),
                        Err(error) => {
                            eprintln!("Eidoryn sidebar icon: {error}");
                            None
                        }
                    })
                    .as_ref()
                    .map(|icon| {
                        gpui::img(std::sync::Arc::clone(icon))
                            .size(px(38.))
                            .rounded_lg()
                    }),
            )
            .when(!compact, |element| {
                element.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_lg().font(self.fonts.semibold()).child(app::NAME))
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
            .child(label(p, self.tr(Text::WorkspaceCrumb)))
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.medium())
                    .child(match self.page {
                        WorkspacePage::Console => self.tr(Text::Console),
                        WorkspacePage::Debug => self.tr(Text::Debugger),
                    }),
            )
            .child(div().flex_1())
            .child(label(
                p,
                format!(
                    "{}  ·  {}",
                    self.themes.active().name,
                    self.renderer_label()
                ),
            ))
            .child(
                button(
                    &self.fonts,
                    p,
                    "maximize",
                    if window.is_maximized() {
                        self.tr(Text::Restore)
                    } else {
                        self.tr(Text::Maximize)
                    },
                    false,
                )
                .on_click(cx.listener(|_, _, window, cx| {
                    window.zoom_window();
                    cx.notify();
                })),
            )
            .child(
                icon_button(p, "top-settings", "settings", self.tr(Text::Settings))
                    .on_click(cx.listener(|view, _, _, cx| view.open_settings(false, cx))),
            )
    }

    fn game_display(&self, _window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let speed = self.tr(Text::Speed);
        div()
            .debug_selector(|| "game-controls".into())
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
                        action(
                            &self.fonts,
                            p,
                            "game-back",
                            self.tr(Text::BackConsole),
                            Some("back"),
                            false,
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.show_page(WorkspacePage::Console, window, cx);
                        })),
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
                                    .font(self.fonts.semibold())
                                    .truncate()
                                    .child(self.display_title().to_owned()),
                            )
                            .child(label(
                                p,
                                format!(
                                    "PLAYSTATION  ·  {:.1} FPS  ·  {:.0}% {speed}",
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
            .track_focus(&self.settings_focus)
            .occlude()
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(gpui::rems(25.))
            .max_w_full()
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
                                    .font(self.fonts.semibold())
                                    .child(self.tr(Text::Settings)),
                            )
                            .child(label(p, self.tr(Text::SettingsHint))),
                    )
                    .child(
                        icon_button(p, "close-settings", "close", self.tr(Text::Close)).on_click(
                            cx.listener(|view, _, window, cx| view.close_settings(window, cx)),
                        ),
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
                    .children(
                        [
                            (
                                "tab-graphics",
                                Text::GraphicsSettings,
                                SettingsTab::Graphics,
                            ),
                            ("tab-appearance", Text::Appearance, SettingsTab::Appearance),
                            ("tab-controller", Text::Controller, SettingsTab::Controller),
                        ]
                        .into_iter()
                        .map(|(id, title, tab)| {
                            button(&self.fonts, p, id, self.tr(title), self.settings_tab == tab)
                                .debug_selector(move || id.into())
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.rebinding = None;
                                    view.settings_tab = tab;
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .child(label(p, self.tr(Text::KeyboardHint)).px_5().py_2())
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
                            .child(match self.settings_tab {
                                SettingsTab::Graphics => self.graphics_panel(cx),
                                SettingsTab::Appearance => self.appearance(cx),
                                SettingsTab::Controller => self.controller_panel(cx),
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
    let palette = *p;
    div()
        .id(id)
        .debug_selector(|| id.to_owned())
        .tab_index(0)
        .border_1()
        .border_color(rgb(p.surface))
        .focus(move |style| style.border_color(rgb(palette.primary)))
        .tooltip(move |_, cx| super::components::tooltip(palette, text.into(), cx))
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
    fonts: &super::fonts::Fonts,
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
                .font(fonts.bold())
                .child(short),
        )
        .when(!compact, |element| {
            element.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(rgb(if selected { p.text } else { p.muted }))
                            .child(name),
                    )
                    .child(label(p, state)),
            )
        })
}

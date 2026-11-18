use super::{
    Desktop, SettingsTab, WorkspacePage,
    components::{ghost_button, ghost_icon_button, label},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::systems::ps1::runner::ExecutionState;
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb, rgba};

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_save_state_selection();
        if self.graphics.open && self.settings_tab == SettingsTab::Graphics {
            self.detect_hardware(cx);
        }
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
            self.fullscreen_video()
        } else {
            self.workspace(game, window, cx)
        };
        let mut root = div()
            .id("desktop-root")
            .tab_group()
            .relative()
            .size_full()
            .bg(rgb(p.background))
            .text_color(rgb(p.text))
            .text_size(px(14.))
            .when(
                !pure_video
                    || self.graphics.open
                    || self.renaming_state.is_some()
                    || self.notice.is_some()
                    || self.snapshot.message.is_some()
                    || self.snapshot.success_message.is_some(),
                |root| root.font(self.fonts.normal()),
            )
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::capture_binding))
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .when(self.navigation.is_dragging(), |root| {
                root.on_mouse_move(cx.listener(|view, event: &gpui::MouseMoveEvent, _, cx| {
                    if event.dragging() {
                        view.navigation.track_drag(event.position.x);
                    } else {
                        view.navigation.cancel_drag();
                    }
                    cx.notify();
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|view, event: &gpui::MouseUpEvent, _, cx| {
                        view.navigation.finish_drag(event.position.x);
                        cx.notify();
                    }),
                )
            })
            .on_click(cx.listener(|view, event, window, cx| {
                if !view.graphics.open
                    && view.snapshot_confirmation.is_none()
                    && view.renaming_state.is_none()
                    && matches!(event, gpui::ClickEvent::Mouse(_))
                {
                    window.focus(&view.focus, cx);
                }
            }))
            .child(body);
        if self.graphics.open {
            if !self.settings_focus.contains_focused(window, cx) {
                window.focus(&self.settings_focus, cx);
            }
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
        if self.renaming_state.is_some() {
            root = root.child(self.rename_state_dialog(cx));
        }
        root.children(self.delete_snapshot_dialog(cx))
    }
}

impl Desktop {
    fn fullscreen_video(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .relative()
            .size_full()
            .debug_selector(|| "pure-display".into())
            .bg(rgb(p.video_background))
            .child(self.video())
            .children(
                self.workspace_notice()
                    .map(|banner| banner.absolute().top_0().left_0().right_0()),
            )
    }

    pub(super) fn workspace_title(&self, window: &Window) -> &'static str {
        self.tr(if self.game_view || window.is_fullscreen() {
            Text::GameView
        } else {
            match self.page {
                WorkspacePage::Console => Text::Console,
                WorkspacePage::SaveStates => Text::SaveStates,
                WorkspacePage::Debug => Text::Debugger,
            }
        })
    }

    pub(super) fn playback_label(&self) -> &'static str {
        self.tr(if self.snapshot.rewinding {
            Text::Rewinding
        } else {
            match self.snapshot.execution_state() {
                ExecutionState::Stopped => Text::Stopped,
                ExecutionState::Paused => Text::Paused,
                ExecutionState::Running => Text::Running,
            }
        })
    }

    fn content_padding(window: &Window) -> gpui::Pixels {
        if window.viewport_size().width <= px(1100.) {
            px(24.)
        } else {
            px(32.)
        }
    }

    pub(super) fn content_width(&self, window: &Window) -> gpui::Pixels {
        (window.viewport_size().width
            - self.navigation.width(window)
            - Self::content_padding(window) * 2.)
            .min(px(1500.))
    }

    fn workspace(&mut self, game: bool, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.themes.active().palette;
        let page = if game {
            self.game_page(window, cx)
        } else {
            match self.page {
                WorkspacePage::Console => self.console_page(window, cx),
                WorkspacePage::SaveStates => self.save_states_page(window, cx),
                WorkspacePage::Debug => self.debug_page(window, cx),
            }
        };
        let width = self.navigation.width(window);
        div()
            .relative()
            .size_full()
            .flex()
            .when(!self.navigation.hidden, |row| {
                row.child(self.cached_sidebar(width, game, cx))
            })
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
                            .id(gpui::SharedString::from(self.workspace_title(window)))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(Self::content_padding(window))
                            .pt(px(22.))
                            .pb(px(32.))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(1500.))
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .children(self.workspace_notice())
                                    .child(page),
                            ),
                    )
                    .child(
                        div()
                            .h(px(28.))
                            .flex_shrink_0()
                            .px(Self::content_padding(window))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .text_size(px(11.))
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "{:.1} / {:.1} FPS  ·  {:.0}% {}  ·  {}",
                                self.snapshot.video_fps,
                                self.snapshot.target_fps,
                                100. * self.snapshot.video_fps / self.snapshot.target_fps,
                                self.tr(Text::Speed),
                                self.renderer_label()
                            ))
                            .when(window.viewport_size().width >= px(1100.), |bar| {
                                bar.child(self.tr(Text::Shortcuts))
                            }),
                    ),
            )
            .when(!self.navigation.hidden, |row| {
                row.child(self.sidebar_handle(width, cx))
            })
    }

    fn topbar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let toggle = self.tr(if self.navigation.hidden {
            Text::ShowSidebar
        } else {
            Text::HideSidebar
        });
        div()
            .h(px(64.))
            .px(Self::content_padding(window))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                ghost_icon_button(p, "toggle-sidebar", "menu", toggle)
                    .debug_selector(|| "toggle-sidebar".into())
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.navigation.hidden = !view.navigation.hidden;
                        view.navigation.cancel_drag();
                        cx.notify();
                    })),
            )
            .child(
                label(
                    p,
                    format!(
                        "{}  /  {}",
                        self.tr(Text::Workspace),
                        self.workspace_title(window)
                    ),
                )
                .text_size(px(13.)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(p.border))
                    .px_3()
                    .py_1()
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(rgb(if self.snapshot.running {
                                p.running_indicator()
                            } else {
                                p.paused_indicator()
                            })),
                    )
                    .child(label(p, self.playback_label()).text_size(px(12.))),
            )
            .child(
                ghost_button(
                    &self.fonts,
                    p,
                    "top-settings",
                    self.tr(Text::Settings),
                    Some("settings"),
                )
                .on_click(cx.listener(|view, _, _, cx| view.open_settings(cx))),
            )
    }

    fn workspace_notice(&self) -> Option<gpui::Div> {
        if self.graphics.open {
            None
        } else {
            self.notice_banner()
        }
    }
    pub(super) fn notice_banner(&self) -> Option<gpui::Div> {
        let p = &self.themes.active().palette;
        self.notice
            .as_ref()
            .or(self.snapshot.message.as_ref())
            .map(|message| {
                (
                    message,
                    p.error,
                    p.on_error,
                    p.error_border(),
                    "error-notice",
                )
            })
            .or_else(|| {
                self.snapshot.success_message.as_ref().map(|message| {
                    (
                        message,
                        p.success,
                        p.on_success,
                        p.success_border(),
                        "success-notice",
                    )
                })
            })
            .map(|(message, background, foreground, border, selector)| {
                div()
                    .debug_selector(move || selector.into())
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(border))
                    .bg(rgb(background))
                    .text_color(rgb(foreground))
                    .text_sm()
                    .child(message.clone())
            })
    }
}

use super::{
    Desktop, SettingsTab, WorkspacePage,
    components::{ghost_button, ghost_icon_button, label},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::shortcuts::ShortcutAction;
use eidoryn_psx::systems::ps1::runner::ExecutionState;
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb, rgba};

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.gba.focus_game)
            && self.is_gba()
            && self.page == WorkspacePage::Game
        {
            window.focus(&self.focus, cx);
        }
        self.sync_save_state_selection();
        if self.graphics.open && self.settings_tab == SettingsTab::Graphics {
            self.detect_hardware(cx);
        }
        if let Some(probe) = &mut self.probe {
            probe.record(self.snapshot.video_frames, "render");
        }
        if self.snapshot.close_ready && !self.saving_library_changes() {
            self.library.shutdown();
            window.remove_window();
        }
        let p = self.workspace_palette();
        self.sync_presentation(window, cx);
        let video_only = self.presentation.shows_video();
        let body = if video_only {
            self.player(window, cx)
        } else {
            self.workspace(window, cx)
        };
        let root = div()
            .id("desktop-root")
            .tab_group()
            .relative()
            .size_full()
            .bg(rgb(p.background))
            .text_color(rgb(p.text))
            .text_size(px(14.))
            .when(
                !video_only
                    || (self.presentation.native_fullscreen && !self.clean_fullscreen)
                    || self.graphics.open
                    || self.renaming_state.is_some()
                    || self.notice.is_some()
                    || self.snapshot.message.is_some()
                    || self.snapshot.success_message.is_some(),
                |root| root.font(self.fonts.normal()),
            )
            .on_drop(
                cx.listener(|view, paths: &gpui::ExternalPaths, window, cx| {
                    if !view.library_dialog_focus.open && !view.graphics.open {
                        view.drop_discs(paths, window, cx);
                    }
                }),
            )
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::capture_binding))
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(
                |view, event: &gpui::ModifiersChangedEvent, _, cx| {
                    view.release_rewind_key(None, &event.modifiers, cx);
                },
            ))
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
                if view.page != WorkspacePage::Library
                    && !view.graphics.open
                    && view.snapshot_confirmation.is_none()
                    && view.renaming_state.is_none()
                    && matches!(event, gpui::ClickEvent::Mouse(_))
                {
                    window.focus(&view.focus, cx);
                }
            }))
            .child(body);
        self.workspace_overlays(root, window, cx)
    }
}

impl Desktop {
    fn workspace_overlays(
        &mut self,
        root: gpui::Stateful<gpui::Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let library_dialog = if self.page == WorkspacePage::Library
            && !self.presentation.shows_video()
            && !self.graphics.open
        {
            self.detail_dialog(cx)
                .or_else(|| self.task_dialog(cx))
                .or_else(|| self.overview_dialog(cx))
        } else {
            None
        };
        self.sync_library_dialog_focus(library_dialog.is_some(), window, cx);
        let mut root = root.children(library_dialog);
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

    fn sync_presentation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fullscreen = window.is_fullscreen();
        let fullscreen_changed = fullscreen != self.presentation.native_fullscreen;
        if let Some(page) = self.presentation.window_changed(fullscreen) {
            self.page = page;
            cx.notify();
        }
        let video_only = self.presentation.shows_video();
        if fullscreen_changed
            || video_only
            || self.page != WorkspacePage::Game
            || self.graphics.open
        {
            self.snapshot_menu = None;
        }
        self.sync_snapshot_mode(!video_only && self.page == WorkspacePage::Debug);
        self.sync_video(
            video_only || matches!(self.page, WorkspacePage::Console | WorkspacePage::Game),
            window,
        );
    }

    fn fullscreen_controls(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .debug_selector(|| "fullscreen-controls".into())
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_2()
            .p_2()
            .rounded_lg()
            .bg(rgb(p.surface))
            .child(self.play_button(cx))
            .when(!self.is_gba(), |bar| {
                bar.child(self.quick_state_controls(cx))
                    .child(self.audio_controls(cx))
            })
            .child(
                ghost_icon_button(
                    p,
                    "player-settings",
                    "settings",
                    self.shortcut_label(self.tr(Text::Settings), ShortcutAction::Settings, None),
                )
                .on_click(cx.listener(|view, _, _, cx| view.open_settings(cx))),
            )
            .child(self.fullscreen_button(window, cx))
    }

    fn player(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let fullscreen = window.is_fullscreen();
        div()
            .relative()
            .size_full()
            .debug_selector(move || {
                if fullscreen {
                    "fullscreen-display"
                } else {
                    "windowed-display"
                }
                .into()
            })
            .bg(rgb(p.video_background))
            .child(self.video())
            .when(fullscreen && !self.clean_fullscreen, |stage| {
                stage.child(
                    div()
                        .absolute()
                        .bottom_3()
                        .left_3()
                        .right_3()
                        .flex()
                        .justify_center()
                        .child(self.fullscreen_controls(window, cx)),
                )
            })
            .children(
                self.workspace_notice()
                    .map(|banner| banner.absolute().top_0().left_0().right_0()),
            )
    }

    fn workspace_title(&self) -> &'static str {
        self.tr(match self.page {
            WorkspacePage::Console => Text::Console,
            WorkspacePage::Game => Text::GameView,
            WorkspacePage::Library => Text::Library,
            WorkspacePage::SaveStates => Text::SaveStates,
            WorkspacePage::Debug => Text::Debugger,
        })
    }

    pub(super) fn playback_label(&self) -> &'static str {
        if self.is_gba() {
            return self.tr(if self.gba.snapshot.running {
                Text::Running
            } else if self.gba.snapshot.path.is_some() {
                Text::Paused
            } else {
                Text::Stopped
            });
        }
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

    fn content_padding(&self, window: &Window) -> gpui::Pixels {
        if self.page == WorkspacePage::Library {
            return if window.viewport_size().width <= px(1050.) {
                px(28.)
            } else {
                px(40.)
            };
        }
        if window.viewport_size().width <= px(1100.) {
            px(24.)
        } else {
            px(32.)
        }
    }

    pub(super) fn content_width(&self, window: &Window) -> gpui::Pixels {
        (window.viewport_size().width
            - self.navigation.width(window)
            - self.content_padding(window) * 2.)
            .min(px(1500.))
    }

    fn workspace(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let page = match self.page {
            WorkspacePage::Console => self.console_page(window, cx),
            WorkspacePage::Game => self.game_page(window, cx),
            WorkspacePage::Library => self.library_page(window, cx),
            WorkspacePage::SaveStates => self.save_states_page(window, cx),
            WorkspacePage::Debug => self.debug_page(window, cx),
        };
        let width = self.navigation.width(window);
        div()
            .relative()
            .size_full()
            .flex()
            .when(!self.navigation.hidden, |row| {
                row.child(self.cached_sidebar(width, cx))
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
                            .id(gpui::SharedString::from(self.workspace_title()))
                            .flex_1()
                            .min_h_0()
                            .when(self.page == WorkspacePage::Library, |s| s.flex().flex_col())
                            .when(self.page != WorkspacePage::Library, |s| {
                                s.overflow_y_scroll()
                            })
                            .px(self.content_padding(window))
                            .pt(
                                if window.is_fullscreen() && self.page == WorkspacePage::Game {
                                    px(2.)
                                } else if self.page == WorkspacePage::Library {
                                    px(28.)
                                } else {
                                    px(22.)
                                },
                            )
                            .pb(px(32.))
                            .child(
                                div()
                                    .w_full()
                                    .when(self.page == WorkspacePage::Library, |s| {
                                        s.flex_1().min_h_0()
                                    })
                                    .max_w(px(1500.))
                                    .mx_auto()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .children(self.workspace_notice())
                                    .child(page),
                            ),
                    )
                    .when(self.page != WorkspacePage::Library, |column| {
                        column.child(self.emulation_status(window))
                    }),
            )
            .when(!self.navigation.hidden, |row| {
                row.child(self.sidebar_handle(width, cx))
            })
    }

    fn emulation_status(&self, window: &Window) -> gpui::Div {
        let p = self.themes.active().palette;
        let status = if self.is_gba() {
            format!(
                "Game Boy Advance  ·  {} × {}",
                eidoryn_gba::WIDTH,
                eidoryn_gba::HEIGHT
            )
        } else {
            format!(
                "{:.1} / {:.1} FPS  ·  {:.0}% {}  ·  {}",
                self.snapshot.video_fps,
                self.snapshot.target_fps,
                100. * self.snapshot.video_fps / self.snapshot.target_fps,
                self.tr(Text::Speed),
                self.renderer_label()
            )
        };
        div()
            .h(px(28.))
            .flex_shrink_0()
            .px(self.content_padding(window))
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(rgb(p.border))
            .text_size(px(11.))
            .text_color(rgb(p.muted))
            .child(status)
            .when(window.viewport_size().width >= px(1100.), |bar| {
                let hints = [
                    (ShortcutAction::Enlarge, self.tr(Text::EnlargeGame)),
                    (ShortcutAction::Crt, "CRT"),
                    (ShortcutAction::Settings, self.tr(Text::Settings)),
                    (ShortcutAction::Fullscreen, self.tr(Text::Fullscreen)),
                ]
                .into_iter()
                .filter_map(|(action, title)| {
                    let chord = self.preferences.shortcuts.binding(action)?;
                    Some(format!("{} {title}", chord.label()))
                })
                .collect::<Vec<_>>()
                .join("  ·  ");
                bar.child(div().min_w_0().truncate().child(hints))
            })
    }

    fn topbar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.workspace_palette();
        let library = self.page == WorkspacePage::Library;
        let breadcrumb = if library {
            self.library.subpage_title().map_or_else(
                || self.tr(Text::Library).to_owned(),
                |page| format!("{}  /  {}", self.tr(Text::Library), self.tr(page)),
            )
        } else {
            format!(
                "{}  /  {}",
                self.tr(Text::Workspace),
                self.workspace_title()
            )
        };
        let toggle = self.tr(if self.navigation.hidden {
            Text::ShowSidebar
        } else {
            Text::HideSidebar
        });
        div()
            .h(px(if library { 70. } else { 64. }))
            .px(self.content_padding(window))
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
            .child(label(p, breadcrumb).text_size(px(13.)))
            .child(div().flex_1())
            .child(self.core_selector(cx))
            .when(window.is_fullscreen(), |bar| {
                bar.child(self.enlargement_button(window, cx))
            })
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
                    .child(div().size(px(7.)).rounded_full().bg(rgb(if self.running() {
                        p.running_indicator()
                    } else {
                        p.paused_indicator()
                    })))
                    .child(label(p, self.playback_label()).text_size(px(12.))),
            )
            .child(
                ghost_button(
                    &self.fonts,
                    p,
                    "top-settings",
                    self.shortcut_label(self.tr(Text::Settings), ShortcutAction::Settings, None),
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
            .or(if self.is_gba() {
                self.gba.snapshot.error.as_ref()
            } else {
                self.snapshot.message.as_ref()
            })
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
                self.snapshot
                    .success_message
                    .as_ref()
                    .filter(|_| !self.is_gba())
                    .map(|message| {
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

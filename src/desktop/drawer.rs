//! Preferences overlay and its collapsible sections.
use super::{
    Desktop, SettingsTab,
    components::{ghost_icon_button, label},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::shortcuts::ShortcutAction;
use gpui::{Context, Div, Stateful, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn settings_drawer(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = &self.themes.active().palette;
        div()
            .id("settings-drawer")
            .track_focus(&self.settings_focus)
            .occlude()
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(560.))
            .max_w_full()
            .flex()
            .flex_col()
            .bg(rgb(p.drawer_surface()))
            .border_l_1()
            .border_color(rgb(p.drawer_border()))
            .child(
                div()
                    .px_6()
                    .py_4()
                    .flex()
                    .items_start()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(23.))
                                    .font(self.fonts.title())
                                    .child(self.tr(Text::Settings)),
                            )
                            .child(
                                label(
                                    p,
                                    format!("{} · {}", self.display_title(), self.playback_label()),
                                )
                                .truncate(),
                            ),
                    )
                    .child(
                        ghost_icon_button(
                            p,
                            "close-settings",
                            "close",
                            self.shortcut_label(
                                self.tr(Text::Close),
                                ShortcutAction::Settings,
                                Some("Esc"),
                            ),
                        )
                        .on_click(
                            cx.listener(|view, _, window, cx| view.close_settings(window, cx)),
                        ),
                    ),
            )
            .child(self.settings_tabs(cx))
            .child(
                div()
                    .id(match self.settings_tab {
                        SettingsTab::Graphics => "graphics-content",
                        SettingsTab::Appearance => "appearance-content",
                        SettingsTab::Controller => "controller-content",
                        SettingsTab::Rewind => "rewind-content",
                        SettingsTab::Shortcuts => "shortcuts-content",
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(self.notice_banner())
                            .child(match self.settings_tab {
                                SettingsTab::Graphics => self.graphics_panel(cx),
                                SettingsTab::Appearance => self.appearance(cx),
                                SettingsTab::Controller => self.controller_panel(cx),
                                SettingsTab::Rewind => self.rewind_settings(cx),
                                SettingsTab::Shortcuts => self.shortcuts_panel(cx),
                            }),
                    ),
            )
            .child(
                div()
                    .px_6()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(match self.settings_tab {
                        SettingsTab::Controller | SettingsTab::Shortcuts => {
                            self.binding_actions(cx)
                        }
                        _ => label(p, self.tr(Text::KeyboardHint)),
                    }),
            )
    }

    fn settings_tabs(&self, cx: &mut Context<Self>) -> Div {
        let p = self.themes.active().palette;
        div()
            .flex()
            .px_4()
            .gap_1()
            .flex_wrap()
            .border_b_1()
            .border_color(rgb(p.border))
            .children(
                [
                    ("tab-graphics", Text::Graphics, SettingsTab::Graphics),
                    ("tab-appearance", Text::Appearance, SettingsTab::Appearance),
                    ("tab-controller", Text::Controller, SettingsTab::Controller),
                    ("tab-rewind", Text::Rewind, SettingsTab::Rewind),
                    ("tab-shortcuts", Text::Shortcuts, SettingsTab::Shortcuts),
                ]
                .into_iter()
                .map(|(id, title, tab)| {
                    let active = self.settings_tab == tab;
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .tab_index(0)
                        .cursor_pointer()
                        .px_3()
                        .py_3()
                        .text_sm()
                        .font(self.fonts.medium())
                        .border_b_2()
                        .border_color(gpui::transparent_black())
                        .when(active, |tab| tab.border_color(rgb(p.primary)))
                        .text_color(rgb(if active { p.primary } else { p.muted }))
                        .focus(move |style| style.bg(rgb(p.tonal)).text_color(rgb(p.on_tonal)))
                        .hover(move |style| style.bg(rgb(p.tonal_hover)))
                        .child(self.tr(title))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.rebinding = None;
                            view.shortcut_editor.recording = None;
                            view.rewind_disable_origin = None;
                            view.settings_tab = tab;
                            cx.notify();
                        }))
                }),
            )
    }

    pub(super) fn setting_group(
        &self,
        title: Text,
        cx: &mut Context<Self>,
        content: impl FnOnce(&mut Context<Self>) -> Div,
    ) -> Div {
        let p = self.themes.active().palette;
        let id = title.id();
        let open = !self.closed_settings_groups.contains(&title);
        div()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.settings_group()))
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .tab_index(0)
                    .cursor_pointer()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .hover(move |style| style.bg(rgb(p.tonal_hover)))
                    .focus(move |style| style.bg(rgb(p.tonal)).text_color(rgb(p.on_tonal)))
                    .child(
                        div()
                            .text_sm()
                            .font(self.fonts.semibold())
                            .child(self.tr(title)),
                    )
                    .child(
                        div()
                            .text_color(rgb(p.muted))
                            .child(if open { "−" } else { "+" }),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.closed_settings_groups.remove(&title) {
                            view.closed_settings_groups.insert(title);
                            if title == Text::RewindRecording {
                                view.rewind_disable_origin = None;
                            }
                            if title == Text::KeyBindings {
                                view.rebinding = None;
                            }
                        }
                        cx.notify();
                    })),
            )
            .when(open, |group| {
                group.child(
                    div()
                        .p_4()
                        .border_t_1()
                        .border_color(rgb(p.border))
                        .child(content(cx)),
                )
            })
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use crate::desktop::tests::{desktop, press_keys};
    use eidoryn_psx::i18n::Text;
    use gpui::{Modifiers, TestAppContext};

    #[gpui::test]
    fn collapsed_groups_keep_their_state_when_switching_tabs(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(desktop);
        press_keys(cx, "f8");
        for selector in [
            "tab-rewind",
            Text::RewindRecording.id(),
            "tab-appearance",
            "tab-rewind",
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(bounds.center(), Modifiers::none());
            cx.run_until_parked();
        }
        assert!(cx.debug_bounds("rewind-enabled").is_none());
        assert!(cx.debug_bounds("rewind-duration").is_some());
        let header = cx.debug_bounds(Text::RewindRecording.id()).unwrap();
        cx.simulate_click(header.center(), Modifiers::none());
        assert!(cx.debug_bounds("rewind-enabled").is_some());
    }
}

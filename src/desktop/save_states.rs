use super::{
    Desktop,
    components::{button, card, label},
};
use eidoryn_psx::{i18n::Text, systems::ps1::runner::Command};
use gpui::{
    Anchor, AnchoredPositionMode, Bounds, Context, Focusable, Window, anchored, deferred, div,
    point, prelude::*, px, rgb, rgba, size,
};

pub(super) struct RenameState {
    pub path: std::path::PathBuf,
    pub input: gpui::Entity<super::text_input::TextInput>,
    pub focus: gpui::FocusHandle,
}

impl Desktop {
    pub(super) fn save_states_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .debug_selector(|| "save-states-page".into())
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(26.))
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::SaveStates)),
            )
            .child(label(p, self.tr(Text::SaveStatesHint)).text_size(px(13.)))
            .child(
                card(p)
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font(self.fonts.medium())
                            .child(self.display_title().to_owned()),
                    )
                    .child(label(p, self.tr(Text::SaveStatesLocation)))
                    .child(self.save_state_controls(cx))
                    .child(self.quick_state_controls(cx))
                    .child(self.rewind_controls(cx))
                    .child(
                        button(
                            &self.fonts,
                            p,
                            "refresh-save-states",
                            self.tr(Text::RefreshSaveStates),
                            false,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.command(Command::RefreshSaveStates, cx);
                        })),
                    ),
            )
            .when(self.snapshot.save_states.is_empty(), |page| {
                page.child(card(p).p_4().child(self.tr(Text::NoSaveStates)))
            })
    }

    pub(super) fn sync_save_state_selection(&mut self) {
        if !self
            .snapshot
            .save_states
            .iter()
            .any(|state| Some(&state.path) == self.selected_save_state.as_ref())
        {
            self.selected_save_state = self
                .snapshot
                .save_states
                .first()
                .map(|state| state.path.clone());
            self.save_state_picker_open = false;
        }
    }

    pub(super) fn quick_state_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                button(
                    &self.fonts,
                    p,
                    "quick-save",
                    format!("{} · Ctrl+Shift+S", self.tr(Text::QuickSave)),
                    false,
                )
                .debug_selector(|| "quick-save".into())
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::QuickSave, cx))),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    "quick-load",
                    format!("{} · Ctrl+Shift+L", self.tr(Text::QuickLoad)),
                    false,
                )
                .debug_selector(|| "quick-load".into())
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::QuickLoad, cx))),
            )
            .child(label(p, self.tr(Text::QuickStateHint)))
    }

    pub(super) fn save_state_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "up" | "down" | "home" | "end" => {
                let states = &self.snapshot.save_states;
                if !states.is_empty() {
                    let current = states
                        .iter()
                        .position(|state| Some(&state.path) == self.selected_save_state.as_ref())
                        .unwrap_or(0);
                    let index = match key {
                        "home" => 0,
                        "end" => states.len() - 1,
                        "up" => current.saturating_sub(1),
                        _ => (current + 1).min(states.len() - 1),
                    };
                    self.selected_save_state = Some(states[index].path.clone());
                }
            }
            "escape" | "enter" | "space" | "tab" => self.save_state_picker_open = false,
            _ => {}
        }
        cx.notify();
        key != "tab"
    }

    pub(super) fn save_state_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let selected = self
            .snapshot
            .save_states
            .iter()
            .find(|state| Some(&state.path) == self.selected_save_state.as_ref());
        let enabled = selected.is_some();
        let title = selected.map_or_else(
            || self.tr(Text::NoStateToLoad).to_owned(),
            |state| state.name.clone(),
        );
        let picker_label = format!("{}: {title}", self.tr(Text::SelectSaveState));
        let palette = *p;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                button(
                    &self.fonts,
                    p,
                    "save-state",
                    self.tr(Text::SaveState),
                    false,
                )
                .debug_selector(|| "save-state".into())
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::SaveState, cx))),
            )
            .child(
                div()
                    .relative()
                    .w(px(280.))
                    .max_w_full()
                    .child(
                        button(&self.fonts, p, "save-state-picker", "", false)
                            .tooltip(move |_, cx| {
                                super::components::tooltip(palette, picker_label.clone().into(), cx)
                            })
                            .debug_selector(|| "save-state-picker".into())
                            .w_full()
                            .justify_between()
                            .child(div().flex_1().min_w_0().truncate().child(title))
                            .child("▾")
                            .when(!enabled, |picker| {
                                picker.opacity(0.45).tab_index(-1).cursor_default()
                            })
                            .when(enabled, |picker| {
                                picker.on_click(cx.listener(|view, _, window, cx| {
                                    view.release_buttons(cx);
                                    view.save_state_picker_open = true;
                                    view.command(Command::RefreshSaveStates, cx);
                                    window.focus(&view.focus, cx);
                                }))
                            }),
                    )
                    .when(self.save_state_picker_open && enabled, |picker| {
                        picker.child(
                            deferred(
                                anchored()
                                    .attach(
                                        Bounds::new(point(px(0.), px(0.)), size(px(280.), px(36.))),
                                        Anchor::BottomLeft,
                                    )
                                    .position_mode(AnchoredPositionMode::Local)
                                    .child(self.save_state_options(cx)),
                            )
                            .with_priority(1),
                        )
                    }),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    "load-state",
                    self.tr(Text::LoadState),
                    false,
                )
                .debug_selector(|| "load-state".into())
                .when(!enabled, |button| {
                    button.opacity(0.45).tab_index(-1).cursor_default()
                })
                .when(enabled, |button| {
                    button.on_click(cx.listener(|view, _, _, cx| {
                        if let Some(state) =
                            view.snapshot.save_states.iter().find(|state| {
                                Some(&state.path) == view.selected_save_state.as_ref()
                            })
                        {
                            let path = state.path.clone();
                            view.save_state_picker_open = false;
                            view.command(Command::LoadState { path }, cx);
                        }
                    }))
                }),
            )
            .child(self.rename_state_button(enabled, cx))
    }

    fn rename_state_button(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        button(
            &self.fonts,
            &self.themes.active().palette,
            "rename-state",
            self.tr(Text::RenameState),
            false,
        )
        .debug_selector(|| "rename-state".into())
        .when(!enabled, |button| {
            button.opacity(0.45).tab_index(-1).cursor_default()
        })
        .when(enabled, |button| {
            button.on_click(cx.listener(|view, _, window, cx| view.begin_rename(window, cx)))
        })
    }

    pub(super) fn rename_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = &self.renaming_state else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" if !rename.input.read(cx).is_composing() => self.close_rename(window, cx),
            "enter"
                if !rename.input.read(cx).is_composing()
                    && rename.input.focus_handle(cx).is_focused(window) =>
            {
                self.submit_rename(window, cx);
            }
            "tab" => self.move_focus(event.keystroke.modifiers.shift, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(super) fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_rewind(cx);
        let Some(state) = self
            .snapshot
            .save_states
            .iter()
            .find(|state| Some(&state.path) == self.selected_save_state.as_ref())
        else {
            return;
        };
        let path = state.path.clone();
        let name = state.name.clone();
        self.release_buttons(cx);
        self.save_state_picker_open = false;
        self.notice = None;
        let palette = self.themes.active().palette;
        let input = cx.new(|cx| super::text_input::TextInput::new(name, palette, cx));
        window.focus(&input.focus_handle(cx), cx);
        self.renaming_state = Some(RenameState {
            path,
            input,
            focus: cx.focus_handle(),
        });
        cx.notify();
    }

    pub(super) fn close_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_state = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = &self.renaming_state else {
            return;
        };
        let path = rename.path.clone();
        let name = rename.input.read(cx).value().to_owned();
        self.command(Command::RenameState { path, name }, cx);
        self.close_rename(window, cx);
    }

    pub(super) fn rename_state_dialog(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let rename = self.renaming_state.as_ref().expect("rename dialog is open");
        let p = &self.themes.active().palette;
        div()
            .id("rename-overlay")
            .occlude()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(rgba(0x00000099))
            .on_click(cx.listener(|view, _, window, cx| view.close_rename(window, cx)))
            .child(
                card(p)
                    .id("rename-dialog")
                    .debug_selector(|| "rename-dialog".into())
                    .track_focus(&rename.focus)
                    .tab_group()
                    .w(px(440.))
                    .max_w_full()
                    .p_5()
                    .gap_4()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_lg()
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::RenameState)),
                    )
                    .child(label(p, self.tr(Text::RenameStateHint)))
                    .child(rename.input.clone())
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                button(
                                    &self.fonts,
                                    p,
                                    "rename-cancel",
                                    self.tr(Text::RenameCancel),
                                    false,
                                )
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        view.close_rename(window, cx);
                                    },
                                )),
                            )
                            .child(
                                button(
                                    &self.fonts,
                                    p,
                                    "rename-confirm",
                                    self.tr(Text::RenameConfirm),
                                    true,
                                )
                                .debug_selector(|| "rename-confirm".into())
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        view.submit_rename(window, cx);
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn save_state_options(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = self.themes.active().palette;
        card(&p)
            .id("save-state-options")
            .debug_selector(|| "save-state-options".into())
            .occlude()
            .w(px(280.))
            .max_h(px(240.))
            .p_1()
            .gap_1()
            .overflow_y_scroll()
            .on_mouse_down_out(cx.listener(|view, _, _, cx| {
                view.save_state_picker_open = false;
                cx.notify();
            }))
            .children(
                self.snapshot
                    .save_states
                    .iter()
                    .enumerate()
                    .map(|(index, state)| {
                        let path = state.path.clone();
                        let selected = self.selected_save_state.as_ref() == Some(&state.path);
                        div()
                            .id(("save-state-option", index))
                            .debug_selector(move || format!("save-state-option-{index}"))
                            .px_3()
                            .py_2()
                            .flex_shrink_0()
                            .text_sm()
                            .truncate()
                            .rounded_md()
                            .bg(rgb(if selected { p.tonal } else { p.surface }))
                            .hover(move |style| style.bg(rgb(p.tonal_hover)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.selected_save_state = Some(path.clone());
                                view.save_state_picker_open = false;
                                window.focus(&view.focus, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .child(state.name.clone())
                    }),
            )
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

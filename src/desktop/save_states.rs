mod menu;
pub(super) use menu::SnapshotMenu;

use super::{
    Desktop,
    components::{button, card, ghost_button, label, page_heading, shortcut},
};
use eidoryn_psx::{
    i18n::Text,
    shortcuts::ShortcutAction,
    systems::{
        checkpoints::{QuickSlot, SaveStateInfo, checkpoint_time},
        ps1::runner::Command,
    },
};
use gpui::{Context, Focusable, Window, div, prelude::*, px, rgb, rgba};

pub(super) struct SnapshotConfirmation {
    state: SaveStateInfo,
    cancel: gpui::FocusHandle,
    confirm: gpui::FocusHandle,
}

pub(super) struct RenameState {
    pub path: std::path::PathBuf,
    pub input: gpui::Entity<super::text_input::TextInput>,
    pub focus: gpui::FocusHandle,
}

impl Desktop {
    fn state_history(&self) -> &[SaveStateInfo] {
        if self.is_gba() {
            &self.gba.snapshot.save_states
        } else {
            &self.snapshot.save_states
        }
    }

    fn quick_slot(&self) -> (QuickSlot, u64) {
        if self.is_gba() {
            (
                self.gba.snapshot.quick_slot,
                self.gba.snapshot.quick_slot_revision,
            )
        } else {
            (self.snapshot.quick_slot, self.snapshot.quick_slot_revision)
        }
    }

    pub(super) fn save_states_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let stacked = self.content_width(window) < px(740.);
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .flex_wrap()
            .child(page_heading(
                &self.fonts,
                p,
                self.tr(Text::SaveAndReturn),
                self.tr(Text::SaveStates),
                self.tr(Text::SaveStatesHint),
            ))
            .child(
                button(
                    &self.fonts,
                    p,
                    "save-state",
                    self.shortcut_label(self.tr(Text::SaveState), ShortcutAction::Snapshot, None),
                    true,
                )
                .debug_selector(|| "save-state".into())
                .when(self.is_gba() && self.gba.snapshot.path.is_none(), |b| {
                    b.tab_index(-1).opacity(0.45).cursor_default()
                })
                .when(!self.is_gba() || self.gba.snapshot.path.is_some(), |b| {
                    b.on_click(cx.listener(|view, _, _, cx| view.command(Command::SaveState, cx)))
                }),
            );
        let side = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .when(!stacked, |side| side.w(px(276.)).flex_shrink_0())
            .child(self.quick_slot_card().child(self.quick_state_controls(cx)))
            .child(
                card(p)
                    .p_4()
                    .gap_2()
                    .child(label(p, self.tr(Text::GameExecution)))
                    .child(
                        div()
                            .font(self.fonts.semibold())
                            .child(self.playback_label()),
                    ),
            );
        div()
            .debug_selector(|| "save-states-page".into())
            .flex()
            .flex_col()
            .gap_4()
            .child(header)
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(stacked, gpui::Styled::flex_col)
                    .when(!stacked, gpui::Styled::items_start)
                    .child(self.snapshot_history(cx))
                    .child(side),
            )
    }

    fn snapshot_history(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .flex_1()
            .min_w_0()
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font(self.fonts.semibold())
                                    .child(self.tr(Text::SnapshotHistory)),
                            )
                            .child(
                                label(
                                    p,
                                    format!(
                                        "{}  ·  {}",
                                        self.display_title(),
                                        self.state_history().len()
                                    ),
                                )
                                .truncate(),
                            ),
                    )
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
            .when(self.state_history().is_empty(), |card| {
                card.child(
                    label(p, self.tr(Text::NoSaveStates))
                        .w_full()
                        .py_6()
                        .text_center(),
                )
            })
            .children(
                self.state_history()
                    .iter()
                    .enumerate()
                    .map(|(index, state)| self.history_row(index, state, cx)),
            )
    }

    fn history_row(
        &self,
        index: usize,
        state: &SaveStateInfo,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = self.themes.active().palette;
        let selected = self.selected_save_state.as_ref() == Some(&state.path);
        let time = checkpoint_time(state.modified)
            .unwrap_or_else(|| self.tr(Text::SnapshotTimeUnknown).to_owned());
        let name = state.name.clone();
        let path = state.path.display().to_string();
        let info = div()
            .id(("snapshot-info", index))
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .tooltip(move |_, cx| super::components::tooltip(p, path.clone().into(), cx))
            .child(div().text_sm().font(self.fonts.semibold()).child(name))
            .child(label(&p, time))
            .when(selected, |info| {
                info.child(label(&p, self.tr(Text::SnapshotSelected)).text_color(rgb(p.primary)))
            });

        div()
            .min_w_0()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected {
                p.history_selected_border()
            } else {
                p.border
            }))
            .bg(rgb(if selected {
                p.history_selected()
            } else {
                p.history_surface()
            }))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(info)
            .child(self.history_actions(index, state, cx))
    }

    fn history_actions(
        &self,
        index: usize,
        state: &SaveStateInfo,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = &self.themes.active().palette;
        let loaded = state.path.clone();
        let renamed = state.path.clone();
        let delete_target = state.clone();
        div()
            .flex()
            .gap_2()
            .flex_wrap()
            .child(
                button(
                    &self.fonts,
                    p,
                    ("load-snapshot", index),
                    self.tr(Text::LoadState),
                    false,
                )
                .debug_selector(move || format!("load-snapshot-{index}"))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.load_history_snapshot(loaded.clone(), cx);
                })),
            )
            .child(
                ghost_button(
                    &self.fonts,
                    p,
                    ("rename-snapshot", index),
                    self.tr(Text::RenameState),
                    None,
                )
                .debug_selector(move || format!("rename-snapshot-{index}"))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.selected_save_state = Some(renamed.clone());
                    view.begin_rename(window, cx);
                })),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    ("delete-snapshot", index),
                    self.tr(Text::DeleteSnapshot),
                    false,
                )
                .text_color(rgb(p.danger_text()))
                .debug_selector(move || format!("delete-snapshot-{index}"))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.confirm_delete_snapshot(delete_target.clone(), window, cx);
                })),
            )
    }

    pub(super) fn quick_slot_card(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        let (title, modified) = match self.quick_slot().0 {
            QuickSlot::Saved(time) => (self.display_title(), Some(time)),
            QuickSlot::Empty => (self.tr(Text::EmptyQuickSlot), None),
            QuickSlot::Unavailable => (self.tr(Text::QuickSlotUnavailable), None),
        };
        card(p)
            .p_4()
            .gap_2()
            .child(
                div()
                    .text_size(px(16.))
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::QuickSlot)),
            )
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.medium())
                    .child(title.to_owned()),
            )
            .when_some(modified, |card, time| {
                card.child(label(
                    p,
                    checkpoint_time(time)
                        .unwrap_or_else(|| self.tr(Text::SnapshotTimeUnknown).to_owned()),
                ))
            })
            .child(label(p, self.tr(Text::QuickStateHint)))
    }

    fn load_history_snapshot(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.selected_save_state = Some(path.clone());
        self.command(Command::LoadState { path }, cx);
    }

    pub(super) fn sync_save_state_selection(&mut self) {
        if !self
            .state_history()
            .iter()
            .any(|state| Some(&state.path) == self.selected_save_state.as_ref())
        {
            self.selected_save_state = self.state_history().first().map(|state| state.path.clone());
        }
    }

    pub(super) fn quick_state_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let (slot, revision) = self.quick_slot();
        let can_load = matches!(slot, QuickSlot::Saved(_));
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
                    self.shortcut_label(self.tr(Text::QuickSave), ShortcutAction::QuickSave, None),
                    false,
                )
                .debug_selector(|| "quick-save".into())
                .when(slot == QuickSlot::Unavailable, |button| {
                    button.tab_index(-1).opacity(0.45).cursor_default()
                })
                .when(slot != QuickSlot::Unavailable, |button| {
                    button.on_click(cx.listener(move |view, _, _, cx| {
                        view.command(Command::QuickSaveIfUnchanged { revision, slot }, cx);
                    }))
                }),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    "quick-load",
                    self.shortcut_label(self.tr(Text::QuickLoad), ShortcutAction::QuickLoad, None),
                    false,
                )
                .debug_selector(|| "quick-load".into())
                .when(can_load, |button| {
                    button.on_click(
                        cx.listener(|view, _, _, cx| view.command(Command::QuickLoad, cx)),
                    )
                })
                .when(!can_load, |button| {
                    button.tab_index(-1).opacity(0.45).cursor_default()
                }),
            )
    }

    fn confirm_delete_snapshot(
        &mut self,
        state: SaveStateInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_input(cx);
        let cancel = cx.focus_handle();
        window.focus(&cancel, cx);
        self.snapshot_confirmation = Some(SnapshotConfirmation {
            state,
            cancel,
            confirm: cx.focus_handle(),
        });
        cx.notify();
    }

    pub(super) fn delete_snapshot_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let pending = self.snapshot_confirmation.as_ref()?;
        Some(
            div()
                .id("delete-snapshot-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .p_6()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000aa))
                .child(
                    self.snapshot_confirmation_controls(pending, cx)
                        .w(px(440.))
                        .max_w_full()
                        .p_5(),
                ),
        )
    }

    fn snapshot_confirmation_controls(
        &self,
        pending: &SnapshotConfirmation,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = &self.themes.active().palette;
        let name = &pending.state.name;
        let time = checkpoint_time(pending.state.modified)
            .unwrap_or_else(|| self.tr(Text::SnapshotTimeUnknown).to_owned());
        div()
            .on_mouse_down_out(cx.listener(|view, _, window, cx| {
                view.finish_snapshot_action(false, window, cx);
            }))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .p_3()
            .border_1()
            .border_color(rgb(p.dialog_border()))
            .bg(rgb(p.dialog_surface()))
            .child(
                div()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::DeleteSnapshotPrompt)),
            )
            .child(label(p, format!("{name}  ·  {time}")))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        ghost_button(
                            &self.fonts,
                            p,
                            "cancel-snapshot-action",
                            shortcut(self.tr(Text::RenameCancel), "Esc"),
                            None,
                        )
                        .debug_selector(|| "cancel-snapshot-action".into())
                        .track_focus(&pending.cancel)
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.finish_snapshot_action(false, window, cx);
                        })),
                    )
                    .child(
                        button(
                            &self.fonts,
                            p,
                            "confirm-delete-snapshot",
                            self.tr(Text::DeleteSnapshot),
                            false,
                        )
                        .text_color(rgb(p.danger_text()))
                        .debug_selector(|| "confirm-delete-snapshot".into())
                        .track_focus(&pending.confirm)
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.finish_snapshot_action(true, window, cx);
                        })),
                    ),
            )
    }

    pub(super) fn snapshot_confirmation_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = &self.snapshot_confirmation else {
            return;
        };
        let confirm = pending.confirm.is_focused(window);
        match key {
            "escape" => self.finish_snapshot_action(false, window, cx),
            "tab" | "left" | "right" | "up" | "down" => {
                window.focus(
                    if confirm {
                        &pending.cancel
                    } else {
                        &pending.confirm
                    },
                    cx,
                );
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn finish_snapshot_action(
        &mut self,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = self.snapshot_confirmation.take()
            && confirmed
        {
            self.command(
                Command::DeleteState {
                    path: pending.state.path,
                    modified: pending.state.modified,
                },
                cx,
            );
        }
        window.focus(&self.focus, cx);
        cx.notify();
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
        self.snapshot_confirmation = None;
        self.stop_rewind(cx);
        let Some(state) = self
            .state_history()
            .iter()
            .find(|state| Some(&state.path) == self.selected_save_state.as_ref())
        else {
            return;
        };
        let path = state.path.clone();
        let name = state.name.clone();
        self.release_buttons(cx);
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
            .bg(rgba(0x000000aa))
            .on_click(cx.listener(|view, _, window, cx| view.close_rename(window, cx)))
            .child(
                card(p)
                    .id("rename-dialog")
                    .bg(rgb(p.dialog_surface()))
                    .border_color(rgb(p.dialog_border()))
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
                                ghost_button(
                                    &self.fonts,
                                    p,
                                    "rename-cancel",
                                    shortcut(self.tr(Text::RenameCancel), "Esc"),
                                    None,
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
                                    shortcut(self.tr(Text::RenameConfirm), "Enter"),
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
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

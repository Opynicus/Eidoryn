//! Selection is scoped to the current results. Confirmations keep a fixed set of game IDs.
use super::{Desktop, Entry, Text, Workspace, div, icon, label, overview::OverviewMenu};
use gpui::{Context, Div, Role, Stateful, prelude::*, px, rgb};
use std::{collections::BTreeSet, sync::Arc};

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

pub(super) struct Removal {
    ids: Arc<[i64]>,
    titles: Vec<String>,
    state: RemovalState,
}

enum RemovalState {
    Ready,
    Saving,
    Failed(String),
}

impl Workspace {
    pub(super) fn reconcile_checked_games(&mut self) {
        let Some(checked) = self.checked_games.as_mut() else {
            return;
        };
        if !checked.is_empty() {
            *checked = self
                .order
                .iter()
                .map(|&i| self.entries[i].game.id)
                .filter(|id| checked.contains(id))
                .collect();
        }
    }

    fn check_all_results(&mut self) {
        if let Some(checked) = &mut self.checked_games {
            checked.extend(self.order.iter().map(|&i| self.entries[i].game.id));
        }
    }

    pub(super) fn toggle_checked_game(&mut self, id: i64) {
        if let Some(checked) = &mut self.checked_games
            && !checked.remove(&id)
        {
            checked.insert(id);
        }
    }
}

impl Desktop {
    pub(super) fn game_checkbox(&self, entry: &Entry, cx: &Context<Self>) -> Stateful<Div> {
        let p = self.library_palette();
        let id = entry.game.id;
        let checked = self
            .library
            .checked_games
            .as_ref()
            .is_some_and(|ids| ids.contains(&id));
        let title = self
            .tr(Text::LibraryCheckGame)
            .replace("{title}", &entry.title);
        div()
            .id(("lib-check", id as u64))
            .debug_selector(move || format!("lib-check-{id}"))
            .role(Role::CheckBox)
            .aria_label(title)
            .aria_toggled(checked.into())
            .tab_index(0)
            .cursor_pointer()
            .flex_shrink_0()
            .size(px(26.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .border_2()
            .border_color(rgb(if checked { p.primary } else { p.muted }))
            .bg(rgb(if checked { p.primary } else { p.surface }))
            .focus(move |s| s.border_color(rgb(if checked { p.on_primary } else { p.primary })))
            .when(checked, |s| s.child(icon("check", p.on_primary)))
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() <= 1 {
                    view.library.toggle_checked_game(id);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
    }

    pub(super) fn begin_batch_selection(&mut self, cx: &mut Context<Self>) {
        self.library.menu = None;
        self.library.card_menu = None;
        self.library.pending_bios = None;
        self.library.checked_games = Some(BTreeSet::new());
        cx.notify();
    }

    pub(super) fn batch_selection_bar(&self, count: usize, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let all_checked = count == self.library.order.len();
        div()
            .debug_selector(|| "lib-batch-bar".into())
            .min_h(px(54.))
            .flex_shrink_0()
            .py_1()
            .border_t_1()
            .border_color(rgb(p.border))
            .flex()
            .flex_wrap()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div().text_sm().font(self.fonts.semibold()).child(
                            self.tr(Text::LibraryCheckedCount)
                                .replace("{count}", &count.to_string()),
                        ),
                    )
                    .children(
                        self.library
                            .selected
                            .and_then(|id| self.card_metadata_status(id)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.library_button(
                            "lib-check-all",
                            Text::LibrarySelectResults,
                            false,
                            cx,
                            |v, _, cx| {
                                v.library.check_all_results();
                                cx.notify();
                            },
                        )
                        .when(all_checked, |s| {
                            s.opacity(0.45).tab_stop(false).cursor_default()
                        }),
                    )
                    .child(
                        self.library_button(
                            "lib-uncheck-all",
                            Text::LibraryClearSelection,
                            false,
                            cx,
                            |v, _, cx| {
                                if let Some(checked) = &mut v.library.checked_games {
                                    checked.clear();
                                }
                                cx.notify();
                            },
                        )
                        .when(count == 0, |s| {
                            s.opacity(0.45).tab_stop(false).cursor_default()
                        }),
                    )
                    .child(self.library_button(
                        "lib-exit-batch",
                        Text::LibraryCancel,
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.checked_games = None;
                            cx.notify();
                        },
                    ))
                    .child(
                        self.library_button(
                            "lib-remove-checked",
                            Text::LibraryRemoveSelected,
                            false,
                            cx,
                            |v, _, cx| {
                                if let Some(checked) = &v.library.checked_games {
                                    v.confirm_game_removal(checked.iter().copied().collect(), cx);
                                }
                            },
                        )
                        .bg(rgb(p.error))
                        .text_color(rgb(p.danger_text()))
                        .when(count == 0, |s| {
                            s.opacity(0.45).tab_stop(false).cursor_default()
                        }),
                    ),
            )
    }

    pub(super) fn confirm_game_removal(&mut self, mut ids: Vec<i64>, cx: &mut Context<Self>) {
        if ids.is_empty() || self.removal_in_progress() {
            return;
        }
        ids.sort_unstable();
        ids.dedup();
        // The summary is bounded even when the entire collection was selected.
        let titles = self
            .library
            .order
            .iter()
            .map(|&index| &self.library.entries[index])
            .filter(|e| ids.binary_search(&e.game.id).is_ok())
            .take(5)
            .map(|e| e.title.clone())
            .collect();
        self.library.card_menu = None;
        self.library.menu = Some(OverviewMenu::Remove(Removal {
            ids: ids.into(),
            titles,
            state: RemovalState::Ready,
        }));
        cx.notify();
    }

    pub(in crate::desktop) fn removal_in_progress(&self) -> bool {
        matches!(
            self.library.menu,
            Some(OverviewMenu::Remove(Removal {
                state: RemovalState::Saving,
                ..
            }))
        )
    }

    pub(super) fn removal_dialog(&self, removal: &Removal, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let count = removal.ids.len();
        let hint = if count == 1 {
            self.tr(Text::LibraryRemoveHint).to_owned()
        } else {
            self.tr(Text::LibraryRemoveSelectionHint)
                .replace("{count}", &count.to_string())
        };
        let remaining = count - removal.titles.len();
        let saving = matches!(removal.state, RemovalState::Saving);
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().text_color(rgb(p.muted)).child(hint))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        removal
                            .titles
                            .iter()
                            .map(|title| div().text_sm().truncate().child(title.clone())),
                    )
                    .when(remaining > 0, |s| {
                        s.child(label(
                            &p,
                            self.tr(Text::LibraryMoreSelected)
                                .replace("{count}", &remaining.to_string()),
                        ))
                    }),
            )
            .when_some(
                match &removal.state {
                    RemovalState::Failed(error) => Some(error.clone()),
                    _ => None,
                },
                |s, error| {
                    s.child(
                        label(&p, error)
                            .debug_selector(|| "lib-removal-error".into())
                            .text_color(rgb(p.on_error)),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        self.library_button(
                            "lib-cancel-removal",
                            Text::LibraryCancel,
                            false,
                            cx,
                            |v, _, cx| {
                                if !v.removal_in_progress() {
                                    v.library.menu = None;
                                    cx.notify();
                                }
                            },
                        )
                        .when(saving, |s| s.opacity(0.45).tab_stop(false).cursor_default()),
                    )
                    .child(
                        self.library_button(
                            "lib-confirm-removal",
                            if saving {
                                Text::LibraryRemoving
                            } else {
                                Text::LibraryRemove
                            },
                            false,
                            cx,
                            |v, _, cx| v.commit_removal(cx),
                        )
                        .bg(rgb(p.error))
                        .text_color(rgb(p.danger_text()))
                        .when(saving, |s| s.opacity(0.45).tab_stop(false).cursor_default()),
                    ),
            );
        self.library_modal(self.tr(Text::LibraryRemove), body, cx)
    }

    fn commit_removal(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let Some(OverviewMenu::Remove(removal)) = &mut self.library.menu else {
            return;
        };
        if matches!(removal.state, RemovalState::Saving) {
            return;
        }
        removal.state = RemovalState::Saving;
        let ids = Arc::clone(&removal.ids);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (ids, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = store.remove_games(&ids);
                    (ids, result)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                let current = matches!(
                    &v.library.menu,
                    Some(OverviewMenu::Remove(removal)) if Arc::ptr_eq(&removal.ids, &ids)
                );
                match result {
                    Ok(()) => {
                        if current {
                            v.library.menu = None;
                            if let Some(checked) = &mut v.library.checked_games {
                                checked.retain(|id| ids.binary_search(id).is_err());
                                if checked.is_empty() {
                                    v.library.checked_games = None;
                                }
                            }
                        }
                        v.reload_library(cx);
                    }
                    Err(error) => {
                        if let Some(OverviewMenu::Remove(removal)) = &mut v.library.menu
                            && current
                        {
                            removal.state = RemovalState::Failed(error);
                        } else {
                            v.notice = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

use super::{Desktop, Entry, Sort, Text, Workspace, missing_last};
use eidoryn_psx::library::{GameMove, Placement};
use gpui::Context;
use std::{cmp::Ordering, collections::HashMap};

pub(super) const SORT_OPTIONS: [(Sort, Text); 5] = [
    (Sort::Manual, Text::LibrarySortManual),
    (Sort::Title, Text::LibraryTitle),
    (Sort::Date, Text::LibraryDate),
    (Sort::Played, Text::LibraryPlayed),
    (Sort::Time, Text::LibraryTime),
];

fn compare_entries(a: &Entry, b: &Entry, sort: Sort) -> Ordering {
    if sort == Sort::Manual {
        return missing_last(a.game.manual_position, b.game.manual_position, Ord::cmp)
            .then_with(|| a.game.id.cmp(&b.game.id));
    }
    let order = match sort {
        Sort::Date => missing_last(a.date.as_ref(), b.date.as_ref(), Ord::cmp),
        Sort::Played => missing_last(a.game.last_played, b.game.last_played, |a, b| b.cmp(a)),
        Sort::Time => b.game.seconds.cmp(&a.game.seconds).then_with(|| {
            b.game
                .last_played
                .is_some()
                .cmp(&a.game.last_played.is_some())
        }),
        _ => Ordering::Equal,
    };
    order
        .then_with(|| a.sort_title.cmp(&b.sort_title))
        .then_with(|| a.game.id.cmp(&b.game.id))
}

impl Workspace {
    fn reorder(&mut self, sort: Sort) {
        let replay = sort == Sort::Manual && !self.pending_moves.is_empty();
        self.platform_count = self
            .entries
            .iter()
            .filter(|e| e.game.platform_kind() == Some(self.platform))
            .count();
        let matches = |i: usize| {
            self.entries[i].game.platform_kind() == Some(self.platform)
                && self.entries[i].search.contains(&self.query)
        };
        let mut order: Vec<_> = (0..self.entries.len())
            .filter(|&i| replay || matches(i))
            .collect();
        order.sort_unstable_by(|&i, &j| compare_entries(&self.entries[i], &self.entries[j], sort));
        if replay {
            // A new search can arrive while SQLite is committing a drop.
            // Replay against the whole library before applying the current filter.
            let indices: HashMap<_, _> = order
                .iter()
                .map(|&i| (self.entries[i].game.id, i))
                .collect();
            let mut ids: Vec<_> = order.iter().map(|&i| self.entries[i].game.id).collect();
            for movement in &self.pending_moves {
                movement.apply(&mut ids);
            }
            order = ids
                .into_iter()
                .map(|id| indices[&id])
                .filter(|&i| matches(i))
                .collect();
        }
        self.order = order;
        if let Some(menu) = &self.card_menu
            && !self
                .order
                .iter()
                .any(|&i| self.entries[i].game.id == menu.id)
        {
            self.card_menu = None;
        }
        self.reconcile_checked_games();
    }
}

impl Desktop {
    pub(in crate::desktop) fn reorder_library(&mut self) {
        self.library.reorder(self.preferences.library.sort);
    }

    pub(super) fn set_library_sort(&mut self, sort: Sort) {
        if self.preferences.library.sort != sort {
            self.preferences.library.sort = sort;
            self.save_preferences();
            self.reorder_library();
        }
    }

    pub(super) fn move_library_game(
        &mut self,
        source: i64,
        target: i64,
        placement: Placement,
        cx: &mut Context<Self>,
    ) {
        if source == target
            || !self.library.checked_games.is_empty()
            || self.library.store.is_none()
        {
            return;
        }
        let visible = self
            .library
            .order
            .iter()
            .map(|&i| self.library.entries[i].game.id);
        if !visible.clone().any(|id| id == source) || !visible.clone().any(|id| id == target) {
            return;
        }
        let initial_order = if self.preferences.library.sort == Sort::Manual {
            Vec::new()
        } else {
            visible.collect()
        };
        let write_now = self.library.pending_moves.is_empty();
        self.library.pending_moves.push_back(GameMove {
            source,
            target,
            placement,
            initial_order,
        });
        self.dismiss_library_context_menu();
        if self.preferences.library.sort == Sort::Manual {
            self.reorder_library();
        } else {
            self.set_library_sort(Sort::Manual);
        }
        if write_now {
            self.persist_next_move(cx);
        }
        cx.notify();
    }

    fn persist_next_move(&mut self, cx: &mut Context<Self>) {
        let Some(movement) = self.library.pending_moves.front().cloned() else {
            return;
        };
        let Some(store) = self.library.store.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.move_game(&movement) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.library.pending_moves.pop_front();
                match result {
                    Ok(ids) => {
                        let ranks: HashMap<_, _> =
                            (0_i64..).zip(ids).map(|(rank, id)| (id, rank)).collect();
                        for entry in &mut view.library.entries {
                            if let Some(rank) = ranks.get(&entry.game.id) {
                                entry.game.manual_position = Some(*rank);
                            }
                        }
                        view.library.entries_revision =
                            view.library.entries_revision.wrapping_add(1);
                    }
                    Err(error) => {
                        view.notice = Some(error);
                        view.reload_library(cx);
                    }
                }
                view.reorder_library();
                view.persist_next_move(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

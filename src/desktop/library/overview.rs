//! Collection overview, filters, layout controls and local import actions.
use super::*;
use crate::desktop::components::{ACTION_HEIGHT, ghost_button, ghost_icon_button};
use gpui::{Anchor, AnchoredPositionMode, Bounds, Pixels, anchored, deferred, point, size};
use std::{cell::Cell, rc::Rc};

pub(super) enum OverviewMenu {
    Add,
    Rescan,
    Sort(Rc<Cell<Bounds<Pixels>>>),
    Remove(super::selection::Removal),
    ClearMetadata(metadata::CacheScope),
}

impl Desktop {
    pub(in crate::desktop) fn library_page(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.library_palette();
        let page = div()
            .debug_selector(|| "library-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(rgb(p.text));
        if self.library.store.is_none() {
            return page.child(label(&p, self.tr(Text::LibraryUnavailable)));
        }
        if self.library.details
            && let Some(index) = self
                .library
                .entries
                .iter()
                .position(|e| Some(e.game.id) == self.library.selected)
        {
            return page.child(self.library_details(index, window, cx));
        }
        if self.library.settings {
            return page.child(self.cover_settings(cx));
        }
        let selected = self.library.visible_selection();
        let mut page = page
            .child(self.collection_header(cx))
            .child(self.library_scan_status(cx))
            .child(self.collection_section(cx))
            .child(self.collection_filters(cx))
            .child(self.collection_selection(selected, cx));
        let shown = self.library.order.len();
        let total = self.library.entries.len();
        if shown == 0 {
            page = page.child(self.empty_collection(cx));
        } else {
            if self.preferences.library.list {
                page = page.child(self.library_list_header(window));
            }
            page = page.child(self.library_results(window, cx));
        }
        page.children(self.card_menu(cx)).child(
            label(
                &p,
                self.tr(Text::LibraryResultCount)
                    .replace("{shown}", &shown.to_string())
                    .replace("{total}", &total.to_string()),
            )
            .pt_2()
            .flex_shrink_0(),
        )
    }

    fn collection_header(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        div()
            .flex()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap_3()
            .pb_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(&p, "YOUR COLLECTION").font(self.fonts.semibold()))
                    .child(
                        div()
                            .text_size(px(30.))
                            .font(self.fonts.bold())
                            .child(self.tr(Text::Library)),
                    )
                    .child(label(&p, self.tr(Text::LibraryCollectionIntro)).text_sm()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!self.library.roots.is_empty(), |row| {
                        row.child(self.library_action(
                            "lib-rescan",
                            Text::LibraryRescan,
                            Some("folder"),
                            false,
                            cx,
                            |v, _, cx| {
                                v.library.menu = Some(OverviewMenu::Rescan);
                                cx.notify();
                            },
                        ))
                    })
                    .child(self.library_action(
                        "lib-add-game",
                        Text::LibraryAddGame,
                        Some("plus"),
                        true,
                        cx,
                        |v, _, cx| {
                            v.library.menu = Some(OverviewMenu::Add);
                            cx.notify();
                        },
                    )),
            )
    }

    fn collection_section(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let hint = self.tr(Text::LibraryRefreshAllHint);
        let enabled = !self.library.entries.is_empty() && !self.library.scraping.full_requested();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .pt_3()
            .pb_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(19.))
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::LibraryAllGames)),
                    )
                    .child(label(
                        &p,
                        self.tr(Text::LibraryGameCount)
                            .replace("{count}", &self.library.entries.len().to_string()),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        ghost_icon_button(&p, "lib-refresh-all", "reset", hint)
                            .debug_selector(|| "lib-refresh-all".into())
                            .role(gpui::Role::Button)
                            .aria_label(hint)
                            .when(!enabled, |button| {
                                button.opacity(0.45).cursor_default().tab_stop(false)
                            })
                            .when(enabled, |button| {
                                button.on_click(cx.listener(|v, _, _, cx| {
                                    v.refresh_library_data(cx);
                                    cx.stop_propagation();
                                }))
                            }),
                    )
                    .child(
                        ghost_button(
                            &self.fonts,
                            &p,
                            "lib-settings",
                            self.tr(Text::LibrarySettings),
                            Some("settings"),
                        )
                        .debug_selector(|| "lib-settings".into())
                        .role(gpui::Role::Button)
                        .aria_label(self.tr(Text::LibrarySettings))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.library.menu = None;
                            v.library.checked_games.clear();
                            v.library.settings = true;
                            v.library.close_editor();
                            cx.stop_propagation();
                            cx.notify();
                        })),
                    ),
            )
    }

    fn collection_filters(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let input = self.library_search(cx);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .pb_3()
            .child(div().flex_1().min_w(px(130.)).max_w(px(420.)).child(input))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label(&p, self.tr(Text::LibrarySort)))
                    .child(self.collection_sort(cx))
                    .child(
                        div()
                            .flex()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(p.border))
                            .children(
                                [
                                    (false, "lib-grid", "grid", Text::LibraryGrid),
                                    (true, "lib-layout", "list", Text::LibraryList),
                                ]
                                .map(|(list, id, symbol, key)| {
                                    let selected = self.preferences.library.list == list;
                                    ghost_icon_button(&p, id, symbol, self.tr(key))
                                        .debug_selector(move || id.into())
                                        .role(gpui::Role::RadioButton)
                                        .aria_toggled(gpui::Toggled::from(selected))
                                        .aria_label(self.tr(key))
                                        .border_0()
                                        .size(px(34.))
                                        .when(selected, |b| b.bg(rgb(p.tonal)))
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            if v.preferences.library.list != list {
                                                v.preferences.library.list = list;
                                                v.save_preferences();
                                                cx.notify();
                                            }
                                            cx.stop_propagation();
                                        }))
                                }),
                            ),
                    ),
            )
    }

    fn collection_sort(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let current = super::ordering::SORT_OPTIONS
            .into_iter()
            .find(|(sort, _)| *sort == self.preferences.library.sort)
            .map_or(Text::LibraryTitle, |(_, key)| key);
        let display = |key| {
            self.tr(if key == Text::LibraryTitle {
                Text::LibraryTitleAZ
            } else {
                key
            })
        };
        let trigger = action(&self.fonts, &p, "lib-sort", display(current), None, false)
            .debug_selector(|| "lib-sort".into())
            .role(gpui::Role::Button)
            .aria_label(self.tr(Text::LibrarySort))
            .aria_expanded(matches!(self.library.menu, Some(OverviewMenu::Sort(_))))
            .child(icon("chevron-down", p.muted).size(px(13.)))
            .on_click(cx.listener(|v, _, _, cx| {
                v.library.menu = if matches!(v.library.menu, Some(OverviewMenu::Sort(_))) {
                    None
                } else {
                    Some(OverviewMenu::Sort(Rc::default()))
                };
                cx.stop_propagation();
                cx.notify();
            }));
        let wrapper = div().relative().child(trigger);
        let Some(OverviewMenu::Sort(bounds)) = &self.library.menu else {
            return wrapper;
        };
        let trigger_bounds = bounds.clone();
        let outside = bounds.clone();
        let menu = card(&p)
            .occlude()
            .p_1()
            .w(px(160.))
            .shadow_md()
            .bg(rgb(p.dialog_surface()))
            .on_mouse_down_out(cx.listener(move |v, event: &gpui::MouseDownEvent, _, cx| {
                if !outside.get().contains(&event.position) {
                    v.library.menu = None;
                    cx.notify();
                }
            }))
            .children(super::ordering::SORT_OPTIONS.map(|(sort, key)| {
                let selected = self.preferences.library.sort == sort;
                div()
                    .id(key.id())
                    .debug_selector(move || key.id().into())
                    .tab_index(0)
                    .role(gpui::Role::RadioButton)
                    .aria_toggled(gpui::Toggled::from(selected))
                    .aria_label(display(key))
                    .h(px(33.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .hover(move |s| s.bg(rgb(p.tonal_hover)))
                    .focus(move |s| s.bg(rgb(p.tonal)))
                    .child(display(key))
                    .when(selected, |row| row.child(icon("check", p.primary)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.set_library_sort(sort);
                        v.library.menu = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
            }));
        wrapper
            .on_children_prepainted(move |children, _, _| {
                if let Some(first) = children.first() {
                    trigger_bounds.set(*first);
                }
            })
            .child(
                div().absolute().top_0().left_0().child(
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .attach(
                                Bounds::new(point(px(0.), px(0.)), size(px(0.), ACTION_HEIGHT)),
                                Anchor::BottomLeft,
                            )
                            .offset(point(px(0.), px(4.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(menu),
                    )
                    .with_priority(1),
                ),
            )
    }

    fn collection_selection(&self, selected: Option<usize>, cx: &Context<Self>) -> Div {
        if !self.library.checked_games.is_empty() {
            return self.batch_selection_bar(self.library.checked_games.len(), cx);
        }
        let p = self.library_palette();
        let bar = div()
            .min_h(px(54.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(p.border))
            .flex()
            .justify_between()
            .items_center()
            .gap_2();
        let selected = selected.map(|index| &self.library.entries[index]);
        if let Some(entry) = selected {
            bar.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(label(&p, self.tr(Text::LibrarySelected)).flex_shrink_0())
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font(self.fonts.semibold())
                            .child(entry.title.clone()),
                    )
                    .children(self.card_metadata_status(entry.game.id)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(self.library_button(
                        "lib-details",
                        Text::LibraryDetails,
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.details = true;
                            cx.notify();
                        },
                    ))
                    .child(self.library_launch_controls("lib-start", entry.game.id, cx)),
            )
        } else {
            bar.child(label(&p, self.tr(Text::LibrarySelectGame)).text_sm())
                .child(label(&p, self.tr(Text::LibrarySelectHint)))
        }
    }

    fn empty_collection(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let empty = self.library.entries.is_empty();
        let panel = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(p.border));
        if !self.library.loaded {
            return panel.child(label(&p, self.tr(Text::LibraryLoading)));
        }
        panel
            .child(div().font(self.fonts.semibold()).child(self.tr(if empty {
                Text::LibraryEmptyTitle
            } else {
                Text::LibraryNoResults
            })))
            .child(
                label(
                    &p,
                    self.tr(if empty {
                        Text::LibraryEmpty
                    } else {
                        Text::LibraryNoResultsHint
                    }),
                )
                .text_sm(),
            )
            .child(div().mt_2().child(if empty {
                self.library_action(
                    "lib-empty-add",
                    Text::LibraryAddGame,
                    Some("plus"),
                    true,
                    cx,
                    |v, _, cx| {
                        v.library.menu = Some(OverviewMenu::Add);
                        cx.notify();
                    },
                )
            } else {
                self.library_button(
                    "lib-clear-search",
                    Text::LibraryClearSearch,
                    false,
                    cx,
                    |v, window, cx| {
                        if let Some(input) = &v.library.search {
                            input.update(cx, |i, cx| {
                                i.clear(window, cx);
                                window.focus(&i.focus_handle(cx), cx);
                            });
                        }
                    },
                )
            }))
    }

    pub(in crate::desktop) fn close_overview_dialog(&mut self) -> bool {
        if self.removal_in_progress() {
            return true;
        }
        self.dismiss_library_context_menu()
            || self.library.menu.take().is_some()
            || !std::mem::take(&mut self.library.checked_games).is_empty()
    }

    pub(in crate::desktop) fn overview_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let p = self.library_palette();
        let body = div().flex().flex_col().gap_4();
        let actions = div().flex().flex_wrap().justify_end().gap_2();
        match self.library.menu.as_ref()? {
            OverviewMenu::Remove(removal) => Some(self.removal_dialog(removal, cx)),
            OverviewMenu::ClearMetadata(scope) => Some(self.metadata_cache_dialog(*scope, cx)),
            OverviewMenu::Sort(_) => None,
            OverviewMenu::Add => Some(
                self.library_modal(
                    self.tr(Text::LibraryAddGame),
                    body.child(label(&p, self.tr(Text::LibraryAddHint)).text_sm())
                        .child(
                            card(&p)
                                .p_3()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font(self.fonts.semibold())
                                        .child(self.tr(Text::LibraryFormats)),
                                )
                                .child(label(&p, self.tr(Text::LibraryCueHint))),
                        )
                        .child(
                            actions
                                .child(self.library_action(
                                    "library-add-folder",
                                    Text::LibraryAddFolder,
                                    Some("folder"),
                                    false,
                                    cx,
                                    |v, _, cx| {
                                        v.library.menu = None;
                                        v.library_picker(tasks::Picker::Folder, cx);
                                        cx.notify();
                                    },
                                ))
                                .child(self.library_action(
                                    "library-add-images",
                                    Text::LibraryAddImages,
                                    Some("plus"),
                                    true,
                                    cx,
                                    |v, _, cx| {
                                        v.library.menu = None;
                                        v.library_picker(tasks::Picker::Images, cx);
                                        cx.notify();
                                    },
                                )),
                        ),
                    cx,
                ),
            ),
            OverviewMenu::Rescan => Some(
                self.library_modal(
                    self.tr(Text::LibraryRescan),
                    body.child(label(&p, self.tr(Text::LibraryRescanIntro)).text_sm())
                        .child(self.library_scan_roots_list())
                        .child(
                            actions
                                .child(self.library_button(
                                    "lib-cancel-rescan",
                                    Text::LibraryCancel,
                                    false,
                                    cx,
                                    |v, _, cx| {
                                        v.library.menu = None;
                                        cx.notify();
                                    },
                                ))
                                .child(self.library_button(
                                    "lib-confirm-rescan",
                                    Text::LibraryScan,
                                    true,
                                    cx,
                                    |v, _, cx| {
                                        v.library.menu = None;
                                        v.scan_roots(cx);
                                        cx.notify();
                                    },
                                )),
                        ),
                    cx,
                ),
            ),
        }
    }

    fn library_scan_roots_list(&self) -> gpui::Stateful<Div> {
        let p = self.library_palette();
        div()
            .id("library-scan-roots")
            .max_h(px(210.))
            .overflow_y_scroll()
            .children(self.library.roots.iter().map(|root| {
                div()
                    .flex()
                    .flex_col()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().text_sm().child(root.path.display().to_string()))
                    .when(root.recursive, |row| {
                        row.child(label(&p, self.tr(Text::LibraryRecursive)))
                    })
                    .when(!root.available, |row| {
                        row.child(label(&p, self.tr(Text::LibraryOffline)))
                    })
            }))
    }
}

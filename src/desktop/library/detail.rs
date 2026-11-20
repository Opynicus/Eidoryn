pub(super) mod editor;

use self::editor::Edit;
use super::{Desktop, Entry, MetadataLookup, Text, card, div, label, tasks::Picker};
use eidoryn_psx::{
    library::{Location, covers, metadata},
    theme::Palette,
};
use gpui::{Context, Div, SharedString, Window, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn find_releases(&mut self, id: i64, query: String, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        self.library.match_generation = self.library.match_generation.wrapping_add(1);
        let generation = self.library.match_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { metadata::search_candidates(&store, id, &query) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if generation == v.library.match_generation {
                    match result {
                        Ok(rows) => v.library.matches = rows,
                        Err(error) => v.notice = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn fetch_metadata(&mut self, id: i64, window: &Window, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        if matches!(
            self.library.metadata_lookups.get(&id),
            Some(MetadataLookup::Fetching(_))
        ) {
            return;
        }
        let generation = self.library.match_generation;
        self.library.lookup_sequence = self.library.lookup_sequence.wrapping_add(1);
        let request = self.library.lookup_sequence;
        self.library
            .metadata_lookups
            .insert(id, MetadataLookup::Fetching(request));
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { metadata::refresh_online(&store, id) })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                use metadata::RefreshOutcome;
                if v.library.metadata_lookups.get(&id) != Some(&MetadataLookup::Fetching(request)) {
                    return;
                }
                let status = match result {
                    Ok(RefreshOutcome::Candidates) => {
                        if v.page == crate::desktop::WorkspacePage::Library
                            && v.library.details
                            && v.library.selected == Some(id)
                            && v.library.editor.is_none()
                            && v.library.clear_playtime_confirmation.is_none()
                            && v.library.menu.is_none()
                            && !v.library.settings
                            && !v.graphics.open
                            && !v.presentation.shows_video()
                            && generation == v.library.match_generation
                        {
                            v.edit_library(Edit::Match(id), String::new(), window, cx);
                        }
                        MetadataLookup::Finished(Text::LibraryMetadataCandidates)
                    }
                    Ok(RefreshOutcome::Updated) => {
                        MetadataLookup::Finished(Text::LibraryMetadataFetched)
                    }
                    Ok(RefreshOutcome::NotFound) => {
                        MetadataLookup::Finished(Text::LibraryMetadataNotFound)
                    }
                    Ok(RefreshOutcome::NoSerial) => {
                        MetadataLookup::Finished(Text::LibraryMetadataNoSerial)
                    }
                    Err(error) => MetadataLookup::Failed(error),
                };
                v.library.metadata_lookups.insert(id, status);
                v.update_library_games([id], cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn select_release(&mut self, id: i64, release: String, cx: &mut Context<Self>) {
        self.library.close_editor();
        if !matches!(
            self.library.metadata_lookups.get(&id),
            Some(MetadataLookup::Fetching(_))
        ) {
            self.library.metadata_lookups.remove(&id);
        }
        self.library_write(cx, move |s| metadata::select(s, id, &release));
    }

    pub(in crate::desktop) fn close_detail_dialog(&mut self) -> bool {
        if !self.library.details
            || self.library.settings
            || (self.library.editor.is_none() && self.library.clear_playtime_confirmation.is_none())
        {
            return false;
        }
        self.library.close_editor();
        true
    }

    pub(in crate::desktop) fn detail_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.library.details || self.library.settings {
            return None;
        }
        if let Some(editor) = &self.library.editor {
            return Some(self.library_modal(self.tr(editor.title()), self.library_editor(cx), cx));
        }
        let id = self.library.clear_playtime_confirmation?;
        let title = Text::LibraryClearTime;
        let entry = self
            .library
            .entries
            .iter()
            .find(|entry| entry.game.id == id)?;
        let p = self.library_palette();
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().font(self.fonts.semibold()).child(entry.title.clone()))
            .child(
                div()
                    .text_color(rgb(p.muted))
                    .child(self.tr(Text::LibraryClearHint)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .child(self.library_button(
                        "lib-cancel-confirm",
                        Text::LibraryCancel,
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.close_editor();
                            cx.notify();
                        },
                    ))
                    .child(
                        self.library_button("lib-confirm", title, false, cx, move |v, _, cx| {
                            v.library.close_editor();
                            v.clear_playtime(id, cx);
                        })
                        .bg(rgb(p.error))
                        .text_color(rgb(p.danger_text())),
                    ),
            );
        Some(self.library_modal(self.tr(title), body, cx))
    }

    pub(super) fn library_details(
        &self,
        index: usize,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let entry = &self.library.entries[index];
        let width = window.viewport_size().width;
        let stacked = width <= px(1050.);
        let art_size = if width <= px(480.) {
            100.
        } else if width <= px(768.) {
            130.
        } else {
            170.
        };
        let primary = div()
            .debug_selector(|| "library-edition-panels".into())
            .flex()
            .flex_col()
            .min_w_0()
            .gap_4()
            .when(!stacked, |s| s.w(gpui::relative(0.59)))
            .child(self.edition_card(entry, cx))
            .child(self.locations_card(entry, cx));
        let secondary = div()
            .debug_selector(|| "library-management-panels".into())
            .flex()
            .flex_col()
            .min_w_0()
            .gap_4()
            .when(!stacked, gpui::Styled::flex_1)
            .child(self.cover_card(entry, cx))
            .child(self.history_card(entry, cx))
            .child(self.manage_game_card(entry, cx));
        let page = div()
            .flex()
            .flex_col()
            .gap_y(px(20.))
            .pb_5()
            .child(
                div().flex().child(
                    self.library_action(
                        "lib-back",
                        Text::LibraryBack,
                        Some("back"),
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.details = false;
                            v.library.pending_bios = None;
                            v.library.close_editor();
                            cx.notify();
                        },
                    )
                    .border_0()
                    .bg(gpui::transparent_black()),
                ),
            )
            .child(self.detail_hero(entry, art_size, cx))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(stacked, gpui::Styled::flex_col)
                    .child(primary)
                    .child(secondary),
            );
        div()
            .id("library-details")
            .debug_selector(|| "library-details".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(page)
    }

    fn detail_hero(&self, entry: &Entry, art_size: f32, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        let small = art_size < 170.;
        let discs = entry.disc_count();
        let summary = [
            entry.serial.clone(),
            Some(
                self.tr(Text::LibraryDiscCount)
                    .replace("{count}", &discs.to_string()),
            ),
            Some(self.library_duration(entry)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let actions = self
            .library_launch_controls("lib-detail-start", id, cx)
            .child(self.library_action(
                "lib-retry-game",
                Text::LibraryRetryGame,
                Some("reset"),
                false,
                cx,
                move |v, _, cx| v.retry_game(id, cx),
            ))
            .when(entry.scrape_active, |s| {
                s.child(self.library_button(
                    "lib-cancel-job",
                    Text::LibraryCancel,
                    false,
                    cx,
                    move |v, _, cx| v.cancel_game_scrape(id, cx),
                ))
            });
        div()
            .flex()
            .items_center()
            .gap_x(px(if small { 16. } else { 24. }))
            .child(
                self.library_artwork(entry, None)
                    .size(px(art_size))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        label(&p, self.tr(Text::LibraryDetailEyebrow))
                            .text_color(rgb(p.primary))
                            .font(self.fonts.semibold()),
                    )
                    .child(
                        div()
                            .text_size(px(if small { 28. } else { 34. }))
                            .line_height(px(if small { 34. } else { 40. }))
                            .font(self.fonts.bold())
                            .child(entry.title.clone()),
                    )
                    .child(self.library_badges(entry))
                    .child(label(&p, summary).text_size(px(13.)))
                    .child(actions),
            )
    }

    fn panel_heading(&self, title: Text, subtitle: impl Into<SharedString>) -> Div {
        let p = self.library_palette();
        div()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font(self.fonts.bold())
                    .text_size(px(18.))
                    .child(self.tr(title)),
            )
            .child(label(&p, subtitle).text_size(px(12.)))
    }

    fn edition_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        let manual = entry.manual_fields.contains(&true);
        let source = [
            manual.then(|| self.tr(Text::LibraryManualData).to_owned()),
            entry.source.clone(),
            entry.refreshed.map(|t| self.played_at(Some(t))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let played = self.played_at(entry.game.last_played);
        let duration = self.library_duration(entry);
        let fields = [
            (
                Text::LibraryPlatform,
                Some(if entry.game.platform == "PS1" {
                    "PlayStation"
                } else {
                    &entry.game.platform
                }),
            ),
            (Text::LibrarySerial, entry.serial.as_deref()),
            (Text::LibraryRegion, entry.region.as_deref()),
            entry.publication_date(),
            (
                Text::LibraryMatchedRelease,
                entry.matched_release(self.tr(Text::LibraryPending)),
            ),
            (
                Text::LibrarySourceUpdated,
                (!source.is_empty()).then_some(source.as_str()),
            ),
            (Text::LibraryPlayed, Some(&played)),
            (Text::LibraryTime, Some(&duration)),
        ];
        let actions = actions(&p)
            .when_else(
                matches!(
                    self.library.metadata_lookups.get(&id),
                    Some(MetadataLookup::Fetching(_))
                ),
                |s| s.child(label(&p, self.tr(Text::LibraryFetchingMetadata))),
                |s| {
                    s.child(self.library_button(
                        "lib-refresh",
                        Text::LibraryRefresh,
                        false,
                        cx,
                        move |v, w, cx| v.fetch_metadata(id, w, cx),
                    ))
                },
            )
            .child(self.library_button(
                "lib-match",
                Text::LibraryMatch,
                false,
                cx,
                move |v, w, cx| v.edit_library(Edit::Match(id), String::new(), w, cx),
            ));
        let panel =
            section(&p)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .flex_wrap()
                        .gap_2()
                        .child(self.panel_heading(
                            Text::LibraryReleaseInfo,
                            self.tr(Text::LibraryReleaseHint),
                        ))
                        .child(self.library_button(
                            "lib-edit-info",
                            Text::LibraryEditInfo,
                            false,
                            cx,
                            move |v, w, cx| v.edit_release(id, w, cx),
                        )),
                )
                .child(self.detail_fields(&fields))
                .child(actions);
        match self.library.metadata_lookups.get(&id) {
            Some(MetadataLookup::Finished(text)) => panel.child(label(&p, self.tr(*text))),
            Some(MetadataLookup::Failed(error)) => panel.child(
                label(
                    &p,
                    format!("{} {error}", self.tr(Text::LibraryMetadataError)),
                )
                .text_color(rgb(p.danger_text())),
            ),
            _ => panel,
        }
    }

    fn detail_fields(&self, fields: &[(Text, Option<&str>)]) -> Div {
        let p = self.library_palette();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(fields.chunks(2).map(|pair| {
                div()
                    .flex()
                    .gap_3()
                    .children(pair.iter().map(|(key, value)| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label(&p, self.tr(*key)))
                            .child(
                                div().text_size(px(14.)).font(self.fonts.medium()).child(
                                    value.unwrap_or(self.tr(Text::LibraryUnknown)).to_owned(),
                                ),
                            )
                    }))
            }))
    }

    fn locations_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        let selected = entry
            .game
            .locations
            .iter()
            .find(|l| Some(l.id) == self.library.location)
            .or_else(|| entry.game.locations.iter().find(|l| l.available))
            .or_else(|| entry.game.locations.first());
        section(&p)
            .child(
                div()
                    .flex()
                    .items_start()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.panel_heading(
                            Text::LibraryLocations,
                            self.tr(Text::LibraryLocationsHint),
                        ),
                    )
                    .when_some(selected, |s, loc| {
                        let edit = Edit::Group {
                            disc: loc.disc_id,
                            location: loc.id,
                            game: id,
                        };
                        s.child(self.library_button(
                            "lib-group",
                            Text::LibraryManageGroup,
                            false,
                            cx,
                            move |v, w, cx| v.edit_library(edit, String::new(), w, cx),
                        ))
                    }),
            )
            .children(entry.game.locations.iter().enumerate().map(|(i, loc)| {
                self.detail_location(i, loc, selected.is_some_and(|s| s.id == loc.id), cx)
            }))
            .child(label(&p, self.tr(Text::LibraryLocationNote)))
    }

    fn detail_location(&self, i: usize, loc: &Location, selected: bool, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = loc.id;
        let disc = format!("{} {}", self.tr(Text::LibraryDisc), loc.disc_number);
        let heading = loc
            .serial
            .as_ref()
            .map_or_else(|| disc.clone(), |serial| format!("{disc} · {serial}"));
        let radio = div()
            .size(px(16.))
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(rgb(if selected { p.primary } else { p.muted }))
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |s| {
                s.child(div().size(px(8.)).rounded_full().bg(rgb(p.primary)))
            });
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .border_t_1()
            .border_color(rgb(p.border))
            .pt_3()
            .child(
                div()
                    .id(("lib-location", i))
                    .debug_selector(move || format!("lib-location-{i}"))
                    .tab_index(0)
                    .role(gpui::Role::RadioButton)
                    .aria_toggled(selected.into())
                    .aria_label(heading.clone())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .child(radio)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font(self.fonts.semibold()).child(heading))
                            .child(label(&p, loc.path.display().to_string()))
                            .when(!loc.available, |s| {
                                s.child(
                                    label(&p, self.tr(Text::LibraryMissing))
                                        .text_color(rgb(p.danger_text())),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.library.location = Some(id);
                        v.library.pending_bios = None;
                        cx.notify();
                    })),
            )
            .child(self.library_button(
                ("lib-relocate", i),
                Text::LibraryRelocate,
                false,
                cx,
                move |v, _, cx| v.library_picker(Picker::Relocate(id), cx),
            ))
    }

    fn cover_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        let hint = if entry.manual_cover {
            Text::LibraryManualCover
        } else if entry.thumbnail.is_some() {
            Text::LibraryCachedCover
        } else {
            Text::LibraryNoCover
        };
        let panel = section(&p)
            .child(self.panel_heading(Text::LibraryCover, self.tr(hint)))
            .when_some(entry.cover_source.as_ref(), |s, source| {
                s.child(label(
                    &p,
                    format!("{} · {source}", self.tr(Text::LibraryCoverSource)),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.library_button(
                        "lib-local-cover",
                        Text::LibraryLocalCover,
                        false,
                        cx,
                        move |v, _, cx| v.library_picker(Picker::Cover(id), cx),
                    ))
                    .child(
                        self.library_button(
                            "lib-get-cover",
                            Text::LibraryGetCover,
                            false,
                            cx,
                            move |v, _, cx| v.fetch_covers(vec![id], cx),
                        )
                        .when(self.library.scraping.busy(), |b| {
                            b.opacity(0.45).cursor_default()
                        }),
                    )
                    .child(self.library_button(
                        "lib-restore-cover",
                        Text::LibraryResetCover,
                        false,
                        cx,
                        move |v, _, cx| v.library_write(cx, move |s| covers::reset_local(s, id)),
                    ))
                    .child(
                        self.library_button(
                            "lib-repair-thumbnail",
                            Text::LibraryRebuildThumbnail,
                            false,
                            cx,
                            move |v, _, cx| {
                                if v.library
                                    .entries
                                    .iter()
                                    .any(|e| e.game.id == id && e.thumbnail.is_some())
                                {
                                    v.library_write(cx, move |s| covers::regenerate(s, id));
                                }
                            },
                        )
                        .when(entry.thumbnail.is_none(), |s| {
                            s.opacity(0.45).cursor_default().tab_index(-1)
                        }),
                    ),
            )
            .when(self.library.cover.is_some(), |s| {
                s.child(self.cover_progress(cx))
            });
        match entry.cover_status.as_deref() {
            Some("not_found") => panel.child(label(&p, self.tr(Text::LibraryNotFound))),
            Some("retry") => panel.child(label(&p, self.tr(Text::LibraryRetry))),
            Some("failed") => panel.child(label(&p, self.tr(Text::LibraryFailed))),
            _ => panel,
        }
    }

    fn history_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        section(&p)
            .child(self.panel_heading(Text::LibraryHistory, self.tr(Text::LibraryHistoryHint)))
            .child(self.detail_fields(&[
                (
                    Text::LibraryPlayed,
                    Some(&self.played_at(entry.game.last_played)),
                ),
                (Text::LibraryTime, Some(&self.library_duration(entry))),
            ]))
            .child(actions(&p).child(self.library_button(
                "lib-clear-time",
                Text::LibraryClearTime,
                false,
                cx,
                move |v, _, cx| {
                    v.library.clear_playtime_confirmation = Some(id);
                    cx.notify();
                },
            )))
    }

    fn manage_game_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = entry.game.id;
        section(&p)
            .child(self.panel_heading(Text::LibraryManageGame, self.tr(Text::LibraryManageHint)))
            .child(
                div().flex().child(
                    self.library_button(
                        "lib-remove",
                        Text::LibraryRemove,
                        false,
                        cx,
                        move |v, _, cx| {
                            v.confirm_game_removal(vec![id], cx);
                        },
                    )
                    .text_color(rgb(p.danger_text()))
                    .bg(rgb(p.error)),
                ),
            )
    }
}

fn section(p: &Palette) -> Div {
    card(p)
        .p_5()
        .rounded(px(8.))
        .gap_4()
        .min_w_0()
        .flex_shrink_0()
}

fn actions(p: &Palette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .pt_3()
        .border_t_1()
        .border_color(rgb(p.border))
}

use super::{
    Confirm, Desktop, Edit, Editor, Entry, Field, MetadataLookup, Text, TextInput, button, card,
    div, label, tasks::Picker,
};
use crate::desktop::components::pair;
use eidoryn_psx::library::{covers, metadata, scrape::WorkerCount};
use gpui::{Context, Div, Focusable, Window, prelude::*, px};

impl Desktop {
    fn edit_library(
        &mut self,
        purpose: Edit,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.close_editor();
        let palette = self.themes.active().palette;
        let input = cx.new(|cx| TextInput::named("library-editor", value, palette, cx));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.library.editor = Some(Editor { purpose, input });
        if let Edit::Match(id) = purpose {
            self.find_releases(id, String::new(), cx);
        }
        cx.notify();
    }

    fn find_releases(&mut self, id: i64, query: String, cx: &mut Context<Self>) {
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

    fn fetch_metadata(&mut self, id: i64, window: &Window, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        if matches!(
            self.library.metadata_lookups.get(&id),
            Some(MetadataLookup::Fetching)
        ) {
            return;
        }
        let generation = self.library.match_generation;
        self.library
            .metadata_lookups
            .insert(id, MetadataLookup::Fetching);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { metadata::refresh_online(&store, id) })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                use metadata::RefreshOutcome;
                let status = match result {
                    Ok(RefreshOutcome::Candidates) => {
                        if v.page == crate::desktop::WorkspacePage::Library
                            && v.library.details
                            && v.library.selected == Some(id)
                            && v.library.editor.is_none()
                            && v.library.confirmation.is_none()
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

    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.library.editor.as_ref() else {
            return;
        };
        let value = editor.input.read(cx).value().trim().to_owned();
        match editor.purpose {
            Edit::Field(id, field) => {
                self.library_write(cx, move |s| {
                    metadata::set_override(s, id, field, Some(&value))
                });
            }
            Edit::Source => {
                if !value.starts_with("https://") || !value.contains("${serial}") {
                    self.notice = Some(self.tr(Text::LibraryTemplate).to_owned());
                    cx.notify();
                    return;
                }
                self.preferences.library.cover_template = value;
                self.cover_preferences_changed(cx);
            }
            Edit::Match(id) => {
                self.find_releases(id, value, cx);
                return;
            }
            Edit::Group { .. } => {
                cx.notify();
                return;
            }
        }
        self.library.close_editor();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn library_editor(&self, cx: &Context<Self>) -> Div {
        let Some(edit) = &self.library.editor else {
            return div();
        };
        let p = self.themes.active().palette;
        let (key, action) = match edit.purpose {
            Edit::Field(_, Field::Title) => (Text::LibraryEditTitle, Text::LibrarySave),
            Edit::Field(_, Field::Region) => (Text::LibraryEditRegion, Text::LibrarySave),
            Edit::Field(_, Field::Date) => (Text::LibraryDateFormat, Text::LibrarySave),
            Edit::Source => (Text::LibraryTemplate, Text::LibrarySave),
            Edit::Match(_) => (Text::LibraryFindRelease, Text::LibraryMatches),
            Edit::Group { .. } => (Text::LibraryGroupFind, Text::LibrarySearch),
        };
        let panel = card(&p)
            .p_3()
            .gap_2()
            .child(label(&p, self.tr(key)))
            .child(edit.input.clone())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.library_button("lib-save-edit", action, true, cx, Self::save_edit))
                    .when_some(
                        if let Edit::Field(id, field) = edit.purpose {
                            Some((id, field))
                        } else {
                            None
                        },
                        |s, (id, field)| {
                            s.child(self.library_button(
                                "lib-reset-edit",
                                Text::LibraryRestore,
                                false,
                                cx,
                                move |v, w, cx| {
                                    v.library.close_editor();
                                    w.focus(&v.focus, cx);
                                    v.library_write(cx, move |s| {
                                        metadata::set_override(s, id, field, None)
                                    });
                                },
                            ))
                        },
                    )
                    .child(self.library_button(
                        "lib-close-edit",
                        Text::LibraryCancel,
                        false,
                        cx,
                        |v, w, cx| {
                            v.library.close_editor();
                            w.focus(&v.focus, cx);
                            cx.notify();
                        },
                    )),
            );
        self.match_or_group_choices(panel, edit, cx)
    }

    fn select_release(&mut self, id: i64, release: String, cx: &mut Context<Self>) {
        self.library.close_editor();
        if !matches!(
            self.library.metadata_lookups.get(&id),
            Some(MetadataLookup::Fetching)
        ) {
            self.library.metadata_lookups.remove(&id);
        }
        self.library_write(cx, move |s| metadata::select(s, id, &release));
    }

    fn match_or_group_choices(&self, mut panel: Div, edit: &Editor, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        match edit.purpose {
            Edit::Match(id) => {
                panel = panel
                    .when(self.library.matches.is_empty(), |s| {
                        s.child(label(&p, self.tr(Text::LibraryNoResults)))
                    })
                    .children(self.library.matches.iter().enumerate().map(|(i, r)| {
                        let key = r.id.clone();
                        let release = r.clone();
                        let date = if r.date.is_empty() {
                            self.tr(Text::LibraryUnknown)
                        } else {
                            r.date.as_str()
                        };
                        let summary = format!("{} · {} · {} · {date}", r.title, r.serial, r.region);
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(summary))
                            .child(self.library_button(
                                ("lib-match", i),
                                Text::LibrarySelectRelease,
                                false,
                                cx,
                                move |v, _, cx| v.select_release(id, key.clone(), cx),
                            ))
                            .child(
                                self.library_button(
                                    ("lib-edition-cover", i),
                                    Text::LibraryEditionCover,
                                    false,
                                    cx,
                                    move |v, _, cx| {
                                        v.fetch_cover_editions(vec![id], Some(release.clone()), cx);
                                    },
                                )
                                .opacity(
                                    if self.library.scraping.busy() {
                                        0.5
                                    } else {
                                        1.0
                                    },
                                ),
                            )
                    }));
            }
            Edit::Group {
                disc,
                location,
                game,
            } => {
                let query = edit.input.read(cx).value().trim().to_lowercase();
                // The full library is searchable; render a short choice list in the detail pane.
                panel = panel.children(
                    self.library
                        .entries
                        .iter()
                        .filter(|e| e.game.id != game && e.search.contains(&query))
                        .take(20)
                        .enumerate()
                        .map(|(i, e)| {
                            let id = e.game.id;
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().child(e.title.clone()))
                                .child(self.library_button(
                                    ("lib-group-target", i),
                                    Text::LibraryGroupHere,
                                    false,
                                    cx,
                                    move |v, _, cx| {
                                        v.library.close_editor();
                                        v.library_write(cx, move |s| s.group_disc(disc, id));
                                    },
                                ))
                                .when_some(e.game.locations.first().map(|l| l.id), |s, other| {
                                    s.child(self.library_button(
                                        ("lib-copy", i),
                                        Text::LibraryCopy,
                                        false,
                                        cx,
                                        move |v, _, cx| {
                                            v.library.close_editor();
                                            v.library_write(cx, move |s| {
                                                s.confirm_copy(location, other)
                                            });
                                        },
                                    ))
                                })
                        }),
                );
            }
            _ => {}
        }
        panel
    }

    pub(super) fn library_details(&self, index: usize, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let entry = &self.library.entries[index];
        let id = entry.game.id;
        let page = div()
            .id("library-details")
            .debug_selector(|| "library-details".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                self.library_button("lib-back", Text::LibraryBack, false, cx, |v, _, cx| {
                    v.library.details = false;
                    v.library.pending_bios = None;
                    v.library.close_editor();
                    cx.notify();
                }),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .py_3()
            .child(
                div()
                    .text_xl()
                    .font(self.fonts.semibold())
                    .child(entry.title.clone()),
            )
            .child(self.library_launch_controls("lib-detail-start", id, cx));
        if self.library.editor.is_some() {
            body = body.child(self.library_editor(cx));
        }
        body = body
            .child(self.edition_card(entry, cx))
            .child(self.locations_card(entry, cx))
            .child(self.cover_card(entry, cx));
        if self.library.cover.is_some() {
            body = body.child(self.cover_progress(cx));
        }
        if self.library.scraping.busy() {
            body = body.child(self.scrape_status(cx));
        }
        page.child(body.child(self.history_controls(id, cx)))
    }

    fn edition_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let id = entry.game.id;
        let mut summary = card(&p).p_3().gap_2();
        for (key, value) in [
            (Text::LibraryPlatform, Some(entry.game.platform.clone())),
            (Text::LibrarySerial, entry.game.serial.clone()),
            (Text::LibraryRegion, entry.region.clone()),
            (Text::LibraryDate, entry.date.clone()),
            (Text::LibraryMatchedRelease, entry.release.clone()),
            (Text::LibraryMetadataSource, entry.source.clone()),
            (
                Text::LibraryMetadataUpdated,
                entry.refreshed.map(|t| self.played_at(Some(t))),
            ),
            (
                Text::LibraryPlayed,
                Some(self.played_at(entry.game.last_played)),
            ),
            (
                Text::LibraryTime,
                Some(self.play_minutes(entry.game.seconds)),
            ),
        ] {
            summary = summary.child(pair(
                &p,
                self.tr(key),
                value.unwrap_or_else(|| self.tr(Text::LibraryUnknown).into()),
            ));
        }
        if entry.date.is_none()
            && let Some(date) = &entry.work_date
        {
            summary = summary.child(pair(&p, self.tr(Text::LibraryWorkDate), date.clone()));
        }
        summary = summary.child(self.edition_controls(entry, cx));
        match self.library.metadata_lookups.get(&id) {
            Some(MetadataLookup::Finished(text)) => summary.child(label(&p, self.tr(*text))),
            Some(MetadataLookup::Failed(error)) => summary.child(label(
                &p,
                format!("{} {error}", self.tr(Text::LibraryMetadataError)),
            )),
            _ => summary,
        }
    }

    fn edition_controls(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let id = entry.game.id;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                [
                    (
                        Text::LibraryEditTitle,
                        Field::Title,
                        Some(entry.title.clone()),
                    ),
                    (Text::LibraryEditRegion, Field::Region, entry.region.clone()),
                    (Text::LibraryEditDate, Field::Date, entry.date.clone()),
                ]
                .into_iter()
                .map(|(key, field, value)| {
                    self.library_button(key.id(), key, false, cx, move |v, w, cx| {
                        v.edit_library(
                            Edit::Field(id, field),
                            value.clone().unwrap_or_default(),
                            w,
                            cx,
                        );
                    })
                }),
            )
            .when_else(
                matches!(
                    self.library.metadata_lookups.get(&id),
                    Some(MetadataLookup::Fetching)
                ),
                |s| s.child(label(&p, self.tr(Text::LibraryFetchingMetadata))),
                |s| {
                    s.child(self.library_button(
                        "lib-refresh",
                        Text::LibraryRefresh,
                        false,
                        cx,
                        move |v, window, cx| v.fetch_metadata(id, window, cx),
                    ))
                },
            )
            .when(entry.scrape_active, |s| {
                s.child(self.library_button(
                    "lib-cancel-job",
                    Text::LibraryCancel,
                    false,
                    cx,
                    move |v, _, cx| v.cancel_game_scrape(id, cx),
                ))
            })
            .child(self.library_button(
                "lib-retry-game",
                Text::LibraryRetryGame,
                false,
                cx,
                move |v, _, cx| v.retry_game(id, cx),
            ))
            .child(self.library_button(
                "lib-match",
                Text::LibraryMatch,
                false,
                cx,
                move |v, w, cx| v.edit_library(Edit::Match(id), String::new(), w, cx),
            ))
    }

    fn locations_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let id = entry.game.id;
        let selected = entry
            .game
            .locations
            .iter()
            .find(|l| Some(l.id) == self.library.location)
            .or_else(|| entry.game.locations.iter().find(|l| l.available))
            .or_else(|| entry.game.locations.first());
        let mut images = card(&p).p_3().gap_2().child(self.tr(Text::LibraryImages));
        for (i, loc) in entry.game.locations.iter().enumerate() {
            let loc_id = loc.id;
            let text = format!(
                "{} {} · {}{} · {}",
                self.tr(Text::LibraryDisc),
                loc.disc_number,
                loc.serial.as_deref().unwrap_or("PS1"),
                if loc.available {
                    String::new()
                } else {
                    format!(" · {}", self.tr(Text::LibraryMissing))
                },
                loc.path.display()
            );
            images = images.child(
                button(
                    &self.fonts,
                    &p,
                    ("lib-location", i),
                    text,
                    selected.is_some_and(|s| s.id == loc.id),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.library.location = Some(loc_id);
                    v.library.pending_bios = None;
                    cx.notify();
                })),
            );
        }
        if let Some(loc) = selected {
            let loc_id = loc.id;
            let disc_id = loc.disc_id;
            images = images.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.library_button(
                        "lib-relocate",
                        Text::LibraryRelocate,
                        false,
                        cx,
                        move |v, _, cx| v.library_picker(Picker::Relocate(loc_id), cx),
                    ))
                    .child(self.library_button(
                        "lib-group",
                        Text::LibraryGroup,
                        false,
                        cx,
                        move |v, w, cx| {
                            v.edit_library(
                                Edit::Group {
                                    disc: disc_id,
                                    location: loc_id,
                                    game: id,
                                },
                                String::new(),
                                w,
                                cx,
                            );
                        },
                    ))
                    .when(entry.game.locations.len() > 1, |s| {
                        s.child(self.library_button(
                            "lib-separate",
                            Text::LibrarySeparate,
                            false,
                            cx,
                            move |v, _, cx| v.library_write(cx, move |s| s.separate_disc(disc_id)),
                        ))
                    }),
            );
        }
        images
    }

    fn cover_card(&self, entry: &Entry, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let id = entry.game.id;
        let mut covers_card = card(&p)
            .p_3()
            .gap_2()
            .child(self.tr(Text::LibraryCover))
            .when_some(entry.thumbnail.clone(), |s, path| {
                s.child(
                    gpui::img(path)
                        .size(px(120.))
                        .object_fit(gpui::ObjectFit::Contain),
                )
            })
            .when_some(entry.cover_source.clone(), |s, source| {
                s.child(pair(&p, self.tr(Text::LibraryCoverSource), source))
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
                        .opacity(if self.library.scraping.busy() {
                            0.5
                        } else {
                            1.0
                        }),
                    )
                    .child(self.library_button(
                        "lib-restore-cover",
                        Text::LibraryResetCover,
                        false,
                        cx,
                        move |v, _, cx| v.library_write(cx, move |s| covers::reset_local(s, id)),
                    )),
            );
        if entry.thumbnail.is_some() {
            covers_card = covers_card.child(self.library_button(
                "lib-repair-thumbnail",
                Text::LibraryRebuildThumbnail,
                false,
                cx,
                move |v, _, cx| v.library_write(cx, move |s| covers::regenerate(s, id)),
            ));
        }
        if let Some(status) = entry.cover_status.as_deref() {
            let text = match status {
                "ready" => self.tr(Text::LibraryReady),
                "not_found" => self.tr(Text::LibraryNotFound),
                "retry" => self.tr(Text::LibraryRetry),
                "failed" => self.tr(Text::LibraryFailed),
                _ => status,
            };
            covers_card = covers_card.child(label(&p, text.to_owned()));
        }
        covers_card
    }

    fn history_controls(&self, id: i64, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let mut body = div().flex().flex_col().gap_2();
        body = body.child(
            div()
                .flex()
                .gap_2()
                .child(self.library_button(
                    "lib-remove",
                    Text::LibraryRemove,
                    false,
                    cx,
                    move |v, _, cx| {
                        v.library.confirmation = Some(Confirm::Remove(id));
                        cx.notify();
                    },
                ))
                .child(self.library_button(
                    "lib-clear-time",
                    Text::LibraryClearTime,
                    false,
                    cx,
                    move |v, _, cx| {
                        v.library.confirmation = Some(Confirm::Clear(id));
                        cx.notify();
                    },
                )),
        );
        if let Some(confirm) = self.library.confirmation {
            let key = match confirm {
                Confirm::Remove(_) => Text::LibraryRemoveHint,
                Confirm::Clear(_) => Text::LibraryClearHint,
            };
            body = body.child(
                card(&p).p_3().gap_2().child(self.tr(key)).child(
                    div()
                        .flex()
                        .gap_2()
                        .child(self.library_button(
                            "lib-confirm",
                            Text::LibraryConfirm,
                            true,
                            cx,
                            move |v, _, cx| {
                                v.library.confirmation = None;
                                match confirm {
                                    Confirm::Remove(id) => {
                                        v.library_write(cx, move |s| s.remove(id));
                                    }
                                    Confirm::Clear(id) => v.clear_playtime(id, cx),
                                }
                            },
                        ))
                        .child(self.library_button(
                            "lib-cancel-confirm",
                            Text::LibraryCancel,
                            false,
                            cx,
                            |v, _, cx| {
                                v.library.confirmation = None;
                                cx.notify();
                            },
                        )),
                ),
            );
        }
        body
    }

    fn toggle_import_scraping(&mut self, cx: &mut Context<Self>) {
        self.preferences.library.scrape_new_games = !self.preferences.library.scrape_new_games;
        self.configure_imports();
        if !self.preferences.library.scrape_new_games {
            self.library.scraping.stop_automatic();
        }
        self.poll_library_scrape(cx);
        self.save_preferences();
        cx.notify();
    }
    pub(super) fn configure_imports(&mut self) {
        let prefs = &self.preferences.library;
        if let Some(store) = &mut self.library.store {
            store.configure_import_scraping(prefs.scrape_new_games.then_some(prefs.cover_provider));
        }
    }

    fn cover_preferences_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = &self.library.cover {
            task.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.library.auto_attempted.clear();
        self.configure_imports();
        self.save_preferences();
        self.auto_covers(cx);
        cx.notify();
    }

    fn scrape_concurrency_control(&self, covers: bool, cx: &Context<Self>) -> Div {
        let (current, label_text, decrease, increase) = if covers {
            (
                self.preferences.library.cover_workers,
                Text::LibraryCoverConcurrency,
                "lib-cover-workers-down",
                "lib-cover-workers-up",
            )
        } else {
            (
                self.preferences.library.scrape_workers,
                Text::LibraryScrapeConcurrency,
                "lib-workers-down",
                "lib-workers-up",
            )
        };
        let p = &self.themes.active().palette;
        let step = |id: &'static str, sign: &'static str, delta: i64| {
            let next = WorkerCount::from(i64::from(current.get()) + delta);
            button(&self.fonts, p, id, sign, false)
                .debug_selector(move || id.into())
                .when(next == current, |b| {
                    b.opacity(0.45).cursor_default().tab_index(-1)
                })
                .when(next != current, |b| {
                    b.on_click(cx.listener(move |view, _, _, cx| {
                        if covers {
                            view.preferences.library.cover_workers = next;
                        } else {
                            view.preferences.library.scrape_workers = next;
                        }
                        view.save_preferences();
                        cx.notify();
                    }))
                })
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.tr(label_text))
            .child(step(decrease, "−", -1))
            .child(label(p, current.get().to_string()))
            .child(step(increase, "+", 1))
    }

    pub(super) fn cover_settings(&self, cx: &Context<Self>) -> Div {
        use covers::Provider;
        let p = self.themes.active().palette;
        let provider = self.preferences.library.cover_provider;
        let choices = [
            (Provider::Libretro, Text::LibraryLibretro),
            (Provider::ScreenScraper, Text::LibraryScreenScraper),
            (Provider::Igdb, Text::LibraryIgdb),
            (Provider::Custom, Text::LibraryCustomCoverSource),
        ];
        let mut block = card(&p)
            .p_4()
            .gap_3()
            .child(self.library_button(
                "lib-settings-back",
                Text::LibraryBack,
                false,
                cx,
                |v, _, cx| {
                    v.library.settings = false;
                    v.library.close_editor();
                    cx.notify();
                },
            ))
            .child(self.tr(Text::LibraryCoverSource))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(choices.map(|(choice, name)| {
                        self.library_button(
                            name.id(),
                            name,
                            choice == provider,
                            cx,
                            move |v, w, cx| {
                                if v.preferences.library.cover_provider != choice {
                                    v.library.close_editor();
                                    w.focus(&v.focus, cx);
                                    v.preferences.library.cover_provider = choice;
                                    v.cover_preferences_changed(cx);
                                }
                            },
                        )
                    })),
            );
        block = match provider {
            Provider::Custom => block
                .child(label(&p, self.preferences.library.cover_template.clone()))
                .child(self.library_button(
                    "lib-template",
                    Text::LibraryTemplate,
                    false,
                    cx,
                    |v, w, cx| {
                        v.edit_library(
                            Edit::Source,
                            v.preferences.library.cover_template.clone(),
                            w,
                            cx,
                        );
                    },
                )),
            Provider::ScreenScraper => {
                block.child(label(&p, self.tr(Text::LibraryScreenScraperCredentials)))
            }
            Provider::Igdb => block.child(label(&p, self.tr(Text::LibraryIgdbCredentials))),
            Provider::Libretro => block,
        };
        block = block
            .child(self.scrape_concurrency_control(false, cx))
            .child(self.scrape_concurrency_control(true, cx));
        block = block.child(self.library_button(
            "lib-scrape-new",
            Text::LibraryScrapeNewGames,
            self.preferences.library.scrape_new_games,
            cx,
            |v, _, cx| {
                v.toggle_import_scraping(cx);
            },
        ));
        block = block.child(self.library_button(
            "lib-auto",
            Text::LibraryAutoCovers,
            self.preferences.library.auto_covers,
            cx,
            |v, _, cx| {
                v.preferences.library.auto_covers = !v.preferences.library.auto_covers;
                v.cover_preferences_changed(cx);
            },
        ));
        if self.library.editor.is_some() {
            block = block.child(self.library_editor(cx));
        }
        if self.library.cover.is_some() {
            block = block.child(self.cover_progress(cx));
        }
        block
    }
}

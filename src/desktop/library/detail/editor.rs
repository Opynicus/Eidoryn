use super::super::TextInput;
use super::{Desktop, Entry, Text, div, label};
use eidoryn_psx::library::metadata::{self, Field};
use gpui::{Context, Div, Entity, Focusable, Subscription, Window, prelude::*, rgb};

#[derive(Clone, Copy)]
pub(in crate::desktop::library) enum Edit {
    Match(i64),
    Group { disc: i64, location: i64, game: i64 },
}

const FIELDS: [(Field, Text, &str); 3] = [
    (Field::Title, Text::LibraryTitle, "library-editor"),
    (Field::Region, Text::LibraryRegion, "lib-editor-region"),
    (Field::Date, Text::LibraryDate, "lib-editor-date"),
];

pub(in crate::desktop::library) struct ReleaseForm {
    id: i64,
    originals: [String; 3],
    inputs: [Entity<TextInput>; 3],
    manual: [bool; 3],
    restore: [bool; 3],
    error: Option<Text>,
}

pub(in crate::desktop::library) enum Editor {
    Release(ReleaseForm),
    Query {
        purpose: Edit,
        input: Entity<TextInput>,
        _subscription: Option<Subscription>,
    },
}

impl Editor {
    pub(in crate::desktop::library) fn is_focused(
        &self,
        window: &Window,
        cx: &Context<Desktop>,
    ) -> bool {
        let inputs = match self {
            Self::Release(form) => form.inputs.as_slice(),
            Self::Query { input, .. } => std::slice::from_ref(input),
        };
        inputs
            .iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn title(&self) -> Text {
        match self {
            Self::Release(_) => Text::LibraryEditInfo,
            Self::Query {
                purpose: Edit::Match(_),
                ..
            } => Text::LibraryMatch,
            Self::Query {
                purpose: Edit::Group { .. },
                ..
            } => Text::LibraryManageGroup,
        }
    }
}

impl Desktop {
    pub(in crate::desktop::library) fn edit_library(
        &mut self,
        purpose: Edit,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.close_editor();
        let palette = self.library_palette();
        let input = cx.new(|cx| TextInput::named("library-editor", value, palette, cx));
        let subscription = matches!(purpose, Edit::Group { .. })
            .then(|| cx.observe(&input, |_, _, cx| cx.notify()));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.library.editor = Some(Editor::Query {
            purpose,
            input,
            _subscription: subscription,
        });
        if let Edit::Match(id) = purpose {
            self.find_releases(id, String::new(), cx);
        }
        cx.notify();
    }

    pub(super) fn edit_release(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.library.entries.iter().find(|e| e.game.id == id) else {
            return;
        };
        let Entry {
            title,
            region,
            date,
            manual_fields,
            ..
        } = entry;
        let originals = [
            title.clone(),
            region.clone().unwrap_or_default(),
            date.clone().unwrap_or_default(),
        ];
        let manual = *manual_fields;
        let p = self.library_palette();
        let inputs = std::array::from_fn(|i| {
            cx.new(|cx| TextInput::named(FIELDS[i].2, originals[i].clone(), p, cx))
        });
        self.library.close_editor();
        window.focus(&inputs[0].read(cx).focus_handle(cx), cx);
        self.library.editor = Some(Editor::Release(ReleaseForm {
            id,
            originals,
            inputs,
            manual,
            restore: [false; 3],
            error: None,
        }));
        cx.notify();
    }

    fn dismiss_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.library.close_editor();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn save_release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Editor::Release(form)) = self.library.editor.as_mut() else {
            return;
        };
        let mut changes = Vec::new();
        for (i, &(field, _, _)) in FIELDS.iter().enumerate() {
            if form.restore[i] {
                changes.push((field, None));
                continue;
            }
            let value = form.inputs[i].read(cx).value().trim();
            if value == form.originals[i].trim() {
                continue;
            }
            if !field.accepts(value) {
                form.error = Some(match field {
                    Field::Date => Text::LibraryInvalidDate,
                    _ => Text::LibraryInvalidField,
                });
                window.focus(&form.inputs[i].read(cx).focus_handle(cx), cx);
                cx.notify();
                return;
            }
            changes.push((field, Some(value.to_owned())));
        }
        let id = form.id;
        self.dismiss_editor(window, cx);
        if !changes.is_empty() {
            self.library_write(cx, move |s| {
                let borrowed: Vec<_> = changes.iter().map(|(f, v)| (*f, v.as_deref())).collect();
                metadata::set_overrides(s, id, &borrowed)
            });
        }
    }

    fn release_form(&self, form: &ReleaseForm, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let id = form.id;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(label(&p, self.tr(Text::LibraryManualHint)))
            .children(FIELDS.iter().enumerate().map(|(i, &(field, key, _))| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.tr(key))
                            .when(form.manual[i], |s| {
                                s.child(self.library_button(
                                    ("lib-reset-field", i),
                                    if form.restore[i] {
                                        Text::LibraryUndoRestore
                                    } else {
                                        Text::LibraryRestore
                                    },
                                    false,
                                    cx,
                                    move |v, _, cx| {
                                        if let Some(Editor::Release(form)) = &mut v.library.editor {
                                            form.restore[i] = !form.restore[i];
                                            form.error = None;
                                            cx.notify();
                                        }
                                    },
                                ))
                            }),
                    )
                    .when_else(
                        form.restore[i],
                        |s| s.child(label(&p, self.tr(Text::LibraryRestoringField))),
                        |s| s.child(form.inputs[i].clone()),
                    )
                    .when(matches!(field, Field::Date), |s| {
                        s.child(label(&p, self.tr(Text::LibraryDateFormat)))
                    })
            }))
            .when_some(form.error, |s, error| {
                s.child(label(&p, self.tr(error)).text_color(rgb(p.danger_text())))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(self.library_button(
                        "lib-reset-edit",
                        Text::LibraryRestoreAll,
                        false,
                        cx,
                        move |v, w, cx| {
                            v.dismiss_editor(w, cx);
                            v.library_write(cx, move |s| {
                                metadata::set_overrides(s, id, &FIELDS.map(|(f, _, _)| (f, None)))
                            });
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.library_button(
                                "lib-close-edit",
                                Text::LibraryCancel,
                                false,
                                cx,
                                Self::dismiss_editor,
                            ))
                            .child(self.library_button(
                                "lib-save-edit",
                                Text::LibrarySave,
                                true,
                                cx,
                                Self::save_release,
                            )),
                    ),
            )
    }

    pub(in crate::desktop::library) fn library_editor(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        match self.library.editor.as_ref() {
            None => div(),
            Some(Editor::Release(form)) => self.release_form(form, cx),
            Some(Editor::Query { purpose, input, .. }) => {
                let (key, action) = match purpose {
                    Edit::Match(_) => (Text::LibraryFindRelease, Some(Text::LibrarySearchAction)),
                    Edit::Group { .. } => (Text::LibraryGroupFind, None),
                };
                let panel = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(label(&p, self.tr(key)))
                    .child(input.clone())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .when_some(action, |row, key| {
                                row.child(self.library_button(
                                    "lib-save-edit",
                                    key,
                                    true,
                                    cx,
                                    |v, _, cx| {
                                        if let Some(Editor::Query {
                                            purpose: Edit::Match(id),
                                            input,
                                            ..
                                        }) = &v.library.editor
                                        {
                                            v.find_releases(
                                                *id,
                                                input.read(cx).value().trim().to_owned(),
                                                cx,
                                            );
                                        }
                                    },
                                ))
                            })
                            .child(self.library_button(
                                "lib-close-edit",
                                Text::LibraryCancel,
                                false,
                                cx,
                                Self::dismiss_editor,
                            )),
                    );
                self.match_or_group_choices(panel, *purpose, input, cx)
            }
        }
    }
    fn match_or_group_choices(
        &self,
        panel: Div,
        purpose: Edit,
        input: &Entity<TextInput>,
        cx: &Context<Self>,
    ) -> Div {
        match purpose {
            Edit::Match(id) => panel.child(self.release_choices(id, cx)),
            Edit::Group {
                disc,
                location,
                game,
            } => panel.child(self.group_choices(disc, location, game, input, cx)),
        }
    }

    fn release_choices(&self, id: i64, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        div()
            .flex()
            .flex_col()
            .gap_2()
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
                let summary = [
                    Some(r.title.as_str()),
                    r.product_code(),
                    Some(r.region.as_str()),
                    Some(date),
                ]
                .into_iter()
                .flatten()
                .filter(|field| !field.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
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
                        move |v, w, cx| {
                            w.focus(&v.focus, cx);
                            v.select_release(id, key.clone(), w, cx);
                        },
                    ))
                    .child(
                        self.library_button(
                            ("lib-edition-cover", i),
                            Text::LibraryEditionCover,
                            false,
                            cx,
                            move |v, _, cx| {
                                v.fetch_library_covers(
                                    vec![id],
                                    super::CoverRequest::Edition(release.clone()),
                                    cx,
                                );
                            },
                        )
                        .opacity(if self.library.scraping.busy() {
                            0.5
                        } else {
                            1.0
                        }),
                    )
            }))
    }

    fn group_choices(
        &self,
        disc: i64,
        location: i64,
        game: i64,
        input: &Entity<TextInput>,
        cx: &Context<Self>,
    ) -> Div {
        let p = self.library_palette();
        let query = input.read(cx).value().trim().to_lowercase();
        // The full library is searchable; render a short choice list in the detail pane.
        let candidates = self.library.entries.iter().filter(|entry| {
            entry.game.id != game
                && entry.game.platform_kind() == Some(eidoryn_psx::library::GamePlatform::Ps1)
                && entry.search.contains(&query)
        });
        let multiple_discs = self
            .library
            .entries
            .iter()
            .find(|e| e.game.id == game)
            .is_some_and(|e| e.game.locations.iter().any(|l| l.disc_id != disc));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(multiple_discs, |s| {
                s.child(self.library_button(
                    "lib-separate",
                    Text::LibrarySeparate,
                    false,
                    cx,
                    move |v, w, cx| {
                        v.dismiss_editor(w, cx);
                        v.library_write(cx, move |s| s.separate_disc(disc));
                    },
                ))
            })
            .when(candidates.clone().next().is_none(), |s| {
                s.child(label(&p, self.tr(Text::LibraryNoResults)))
            })
            .children(candidates.take(20).enumerate().map(|(i, e)| {
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
                        move |v, w, cx| {
                            v.dismiss_editor(w, cx);
                            v.library_write(cx, move |s| s.group_disc(disc, id));
                        },
                    ))
                    .when_some(
                        e.game.locations.iter().find(|l| l.available).map(|l| l.id),
                        |s, other| {
                            s.child(self.library_button(
                                ("lib-copy", i),
                                Text::LibraryCopy,
                                false,
                                cx,
                                move |v, w, cx| {
                                    v.dismiss_editor(w, cx);
                                    v.library_write(cx, move |s| s.confirm_copy(location, other));
                                },
                            ))
                        },
                    )
            }))
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use crate::desktop::{
        library::Workspace,
        tests::{click_control, desktop, press_keys},
    };
    use eidoryn_psx::library::{Library, metadata};
    use gpui::{TestAppContext, px, size};
    use std::sync::atomic::AtomicBool;

    #[gpui::test]
    fn release_form_keeps_invalid_drafts_and_restores_fields_independently(
        cx: &mut TestAppContext,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let image = tmp.path().join("Original.iso");
        std::fs::write(&image, vec![0; 2048 * 32]).unwrap();
        let db = Library::open(&tmp.path().join("games.db")).unwrap();
        db.scan_files(&[image], &AtomicBool::default(), |_| {})
            .unwrap();
        let (view, cx) = cx.add_window_view(desktop);
        view.update(cx, |v, _| {
            v.preferences_path = Some(tmp.path().join("preferences.json"));
            v.library = Workspace::new(Some(db.clone()));
        });
        cx.simulate_resize(size(px(1280.), px(900.)));
        cx.run_until_parked();
        for id in [
            "nav-library",
            "library-entry-1",
            "lib-details",
            "lib-edit-info",
        ] {
            click_control(cx, id);
        }
        press_keys(cx, "ctrl-a");
        cx.simulate_input("Edited title");
        click_control(cx, "lib-editor-date");
        cx.simulate_input("1996-02-31");
        click_control(cx, "lib-save-edit");
        assert!(cx.debug_bounds("lib-save-edit").is_some());
        assert_eq!(metadata::list(&db).unwrap()[&1].manual_fields, [false; 3]);

        press_keys(cx, "ctrl-a");
        cx.simulate_input("1996-02");
        click_control(cx, "lib-save-edit");
        let records = metadata::list(&db).unwrap();
        assert_eq!(records[&1].title.as_deref(), Some("Edited title"));
        assert_eq!(records[&1].date.as_deref(), Some("1996-02"));
        assert_eq!(records[&1].manual_fields, [true, false, true]);

        click_control(cx, "lib-edit-info");
        click_control(cx, "lib-editor-region");
        cx.simulate_input("Japan");
        click_control(cx, "lib-reset-field-2");
        // A pending single-field restore leaves the rest of the open draft intact.
        assert!(cx.debug_bounds("lib-editor-date").is_none());
        click_control(cx, "lib-save-edit");
        let updated = metadata::list(&db).unwrap();
        assert_eq!(updated[&1].manual_fields, [true, true, false]);
        assert_eq!(updated[&1].region.as_deref(), Some("Japan"));
    }
}

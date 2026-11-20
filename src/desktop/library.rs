//! Desktop library browsing. File reads, media inspection and database writes run off the UI thread.
mod detail;
mod tasks;
#[cfg(all(test, feature = "ui-tests"))]
mod tests;

use super::{
    Desktop,
    components::{button, card, label},
    icons::icon,
    text_input::TextInput,
};
use eidoryn_psx::{
    i18n::Text,
    library::{
        Game, Library, ScanRoot,
        metadata::{self, Field, Release},
    },
};
use gpui::{Context, Div, Entity, Focusable, Window, div, prelude::*, px, rgb, uniform_list};
use std::{
    cmp::Ordering,
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) fn open() -> Result<Library, String> {
    let path = Library::default_path().ok_or("Cannot locate the game library data directory")?;
    Library::open(&path)
}

struct Entry {
    game: Game,
    title: String,
    search: String,
    sort_title: String,
    pending: bool,
    source: Option<String>,
    refreshed: Option<i64>,
    date: Option<String>,
    work_date: Option<String>,
    region: Option<String>,
    thumbnail: Option<PathBuf>,
    serial: Option<String>,
    release: Option<String>,
    cover_source: Option<String>,
    cover_status: Option<String>,
}

impl Entry {
    fn new(game: Game, metadata: Option<metadata::Metadata>) -> Self {
        let title = metadata
            .as_ref()
            .and_then(|m| m.title.clone())
            .unwrap_or_else(|| game.title.clone());
        let mut terms = vec![title.clone(), game.title.clone()];
        if let Some(meta) = &metadata {
            terms.extend(meta.aliases.iter().cloned());
        }
        terms.extend(game.locations.iter().filter_map(|loc| loc.serial.clone()));
        terms.extend(game.serial.clone());
        terms.extend(metadata.as_ref().and_then(|m| m.serial.clone()));
        Self {
            sort_title: title.to_lowercase(),
            pending: metadata.as_ref().is_some_and(|m| m.pending),
            source: metadata.as_ref().and_then(|m| m.source.clone()),
            refreshed: metadata.as_ref().and_then(|m| m.refreshed),
            title,
            search: terms.join("\n").to_lowercase(),
            work_date: metadata.as_ref().and_then(|m| m.work_date.clone()),
            date: metadata.as_ref().and_then(|m| m.date.clone()),
            region: metadata
                .as_ref()
                .and_then(|m| m.region.clone())
                .or_else(|| game.region.clone()),
            thumbnail: metadata.as_ref().and_then(|m| m.thumbnail.clone()),
            serial: metadata
                .as_ref()
                .and_then(|m| m.serial.clone())
                .or_else(|| game.serial.clone()),
            release: metadata.as_ref().and_then(|m| m.release_id.clone()),
            cover_source: metadata.as_ref().and_then(|m| m.cover_source.clone()),
            cover_status: metadata.and_then(|m| m.status),
            game,
        }
    }

    fn disc_count(&self) -> usize {
        self.game
            .locations
            .iter()
            .map(|loc| loc.disc_number)
            .collect::<HashSet<_>>()
            .len()
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Sort {
    #[default]
    Title,
    Date,
    Played,
    Time,
}

impl Sort {
    const ALL: [(Self, Text); 4] = [
        (Self::Title, Text::LibraryTitle),
        (Self::Date, Text::LibraryDate),
        (Self::Played, Text::LibraryPlayed),
        (Self::Time, Text::LibraryTime),
    ];
}

#[derive(Clone, Copy)]
enum Edit {
    Field(i64, Field),
    Source,
    Match(i64),
    Group { disc: i64, location: i64, game: i64 },
}

struct Editor {
    purpose: Edit,
    input: Entity<TextInput>,
}

#[derive(Clone, Copy)]
enum Confirm {
    Remove(i64),
    Clear(i64),
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent view, scan and asynchronous refresh states"
)]
pub(super) struct Workspace {
    pub(super) store: Option<Library>,
    entries: Vec<Entry>,
    roots: Vec<ScanRoot>,
    order: Vec<usize>,
    sort: Sort,
    search: Option<Entity<TextInput>>,
    search_subscription: Option<gpui::Subscription>,
    query: String,
    #[cfg(all(test, feature = "ui-tests"))]
    rendered_cells: std::ops::Range<usize>,
    loaded: bool,
    last_refresh: std::time::Instant,
    reading: bool,
    reload_pending: bool,
    selected: Option<i64>,
    details: bool,
    location: Option<i64>,
    pending_bios: Option<(PathBuf, bool)>,
    pending_directory: Option<PathBuf>,
    recursive: bool,
    scan: Option<Arc<AtomicBool>>,
    cover: Option<Arc<AtomicBool>>,
    auto_attempted: HashSet<(i64, String)>,
    report: tasks::Report,
    direct: Option<tasks::DirectChoice>,
    choices: Vec<eidoryn_psx::library::TrackChoice>,
    editor: Option<Editor>,
    matches: Vec<Release>,
    match_generation: u64,
    confirmation: Option<Confirm>,
    settings: bool,
}

impl Workspace {
    pub(super) fn new(store: Option<Library>) -> Self {
        Self {
            store,
            entries: vec![],
            roots: vec![],
            order: vec![],
            sort: Sort::Title,
            search: None,
            search_subscription: None,
            query: String::new(),
            #[cfg(all(test, feature = "ui-tests"))]
            rendered_cells: 0..0,
            loaded: false,
            last_refresh: std::time::Instant::now(),
            reading: false,
            reload_pending: false,
            selected: None,
            details: false,
            location: None,
            pending_bios: None,
            pending_directory: None,
            recursive: true,
            scan: None,
            cover: None,
            auto_attempted: HashSet::new(),
            report: tasks::Report::default(),
            direct: None,
            choices: vec![],
            editor: None,
            matches: vec![],
            match_generation: 0,
            confirmation: None,
            settings: false,
        }
    }

    fn reorder(&mut self) {
        self.order = (0..self.entries.len())
            .filter(|&i| self.entries[i].search.contains(&self.query))
            .collect();
        self.order.sort_by(|&a, &b| {
            let (a, b) = (&self.entries[a], &self.entries[b]);
            let order = match self.sort {
                Sort::Date => missing_last(a.date.as_ref(), b.date.as_ref(), Ord::cmp),
                Sort::Played => {
                    missing_last(a.game.last_played, b.game.last_played, |a, b| b.cmp(a))
                }
                Sort::Time => b.game.seconds.cmp(&a.game.seconds).then_with(|| {
                    b.game
                        .last_played
                        .is_some()
                        .cmp(&a.game.last_played.is_some())
                }),
                Sort::Title => Ordering::Equal,
            };
            order
                .then_with(|| a.sort_title.cmp(&b.sort_title))
                .then_with(|| a.game.id.cmp(&b.game.id))
        });
    }

    fn select(&mut self, id: i64) {
        if self.selected != Some(id) {
            self.location = None;
            self.pending_bios = None;
            self.close_editor();
        }
        self.selected = Some(id);
    }

    fn close_editor(&mut self) {
        self.editor = None;
        self.matches.clear();
        self.confirmation = None;
        self.match_generation = self.match_generation.wrapping_add(1);
    }
}

fn missing_last<T>(a: Option<T>, b: Option<T>, cmp: impl FnOnce(&T, &T) -> Ordering) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => cmp(&a, &b),
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

impl Desktop {
    fn library_button(
        &self,
        id: impl Into<gpui::ElementId>,
        key: Text,
        primary: bool,
        cx: &Context<Self>,
        callback: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let id = id.into();
        button(
            &self.fonts,
            &self.themes.active().palette,
            id.clone(),
            self.tr(key),
            primary,
        )
        .debug_selector(move || id.to_string())
        .on_click(cx.listener(move |view, _, window, cx| {
            callback(view, window, cx);
            cx.stop_propagation();
        }))
    }

    pub(super) fn enter_library(&mut self, cx: &mut Context<Self>) {
        // A new visit can retry elapsed download backoffs or an explicitly canceled batch.
        if self.library.cover.is_none() {
            self.library.auto_attempted.clear();
        }
        self.reload_library(cx);
    }

    pub(super) fn library_input_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.library
            .search
            .iter()
            .chain(self.library.editor.iter().map(|e| &e.input))
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn library_search(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some(input) = &self.library.search {
            return input.clone();
        }
        let p = self.themes.active().palette;
        let input = cx.new(|cx| TextInput::named("library-search", String::new(), p, cx));
        self.library.search_subscription = Some(cx.observe(&input, |view, input, cx| {
            let query = input.read(cx).value().trim().to_lowercase();
            if query != view.library.query {
                view.library.query = query;
                view.library.reorder();
                if !view
                    .library
                    .order
                    .iter()
                    .any(|&i| Some(view.library.entries[i].game.id) == view.library.selected)
                {
                    view.library.selected = None;
                }
                cx.notify();
            }
        }));
        self.library.search = Some(input.clone());
        input
    }

    pub(super) fn library_page(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let mut page = div()
            .debug_selector(|| "library-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_3();
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
            return page.child(self.library_details(index, cx));
        }
        if self.library.settings {
            return page.child(self.cover_settings(cx));
        }
        page = page
            .child(self.library_toolbar(cx))
            .child(self.library_scan_status(cx))
            .child(self.library_sort_controls(cx));
        if let Some(id) = self.library.selected {
            page = page.child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.library_button(
                        "lib-start",
                        Text::LibraryStart,
                        true,
                        cx,
                        move |v, _, cx| v.launch_library(id, cx),
                    ))
                    .child(self.library_button(
                        "lib-details",
                        Text::LibraryDetails,
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.details = true;
                            cx.notify();
                        },
                    )),
            );
        } else if !self.library.order.is_empty() {
            // Keep cards under the pointer between the first and second clicks.
            page = page.child(
                div()
                    .h(crate::desktop::components::ACTION_HEIGHT)
                    .flex_shrink_0(),
            );
        }
        if self.library.order.is_empty() {
            let key = if !self.library.loaded {
                Text::LibraryLoading
            } else if self.library.entries.is_empty() {
                Text::LibraryEmpty
            } else {
                Text::LibraryNoResults
            };
            return page.child(card(&p).p_6().child(self.tr(key)));
        }
        page.child(self.library_results(window, cx))
    }

    fn library_toolbar(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let input = self.library_search(cx);
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(180.))
                    .child(label(&p, self.tr(Text::LibrarySearch)))
                    .child(input),
            )
            .child(
                self.library_button("lib-files", Text::LibraryAddImages, true, cx, |v, _, cx| {
                    v.library_picker(tasks::Picker::Images, cx);
                })
                .debug_selector(|| "library-add-images".into()),
            )
            .child(
                self.library_button(
                    "lib-folder",
                    Text::LibraryAddFolder,
                    false,
                    cx,
                    |v, _, cx| v.library_picker(tasks::Picker::Folder, cx),
                )
                .debug_selector(|| "library-add-folder".into()),
            )
            .when(!self.library.roots.is_empty(), |s| {
                s.child(self.library_button(
                    "lib-rescan",
                    Text::LibraryRescan,
                    false,
                    cx,
                    |v, _, cx| v.scan_roots(cx),
                ))
            })
            .child(self.library_button(
                "lib-settings",
                Text::LibrarySettings,
                false,
                cx,
                |v, _, cx| {
                    v.library.settings = true;
                    v.library.close_editor();
                    cx.notify();
                },
            ))
    }

    fn library_sort_controls(&self, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(label(&p, self.tr(Text::LibrarySort)))
            .children(Sort::ALL.map(|(sort, key)| {
                self.library_button(
                    key.id(),
                    key,
                    self.library.sort == sort,
                    cx,
                    move |v, _, cx| {
                        v.library.sort = sort;
                        v.library.reorder();
                        cx.notify();
                    },
                )
            }))
            .child(div().flex_1())
            .child(self.library_button(
                "lib-layout",
                if self.preferences.library.list {
                    Text::LibraryGrid
                } else {
                    Text::LibraryList
                },
                false,
                cx,
                |v, _, cx| {
                    v.preferences.library.list = !v.preferences.library.list;
                    v.save_preferences();
                    cx.notify();
                },
            ))
    }

    fn library_results(&self, window: &Window, cx: &Context<Self>) -> gpui::UniformList {
        let columns = if self.preferences.library.list {
            1
        } else if self.content_width(window) >= px(1150.) {
            6
        } else if self.content_width(window) >= px(850.) {
            5
        } else {
            4
        };
        let rows = self.library.order.len().div_ceil(columns);
        uniform_list(
            "library-results",
            rows,
            cx.processor(move |v, range: std::ops::Range<usize>, _, cx| {
                #[cfg(all(test, feature = "ui-tests"))]
                {
                    v.library.rendered_cells =
                        (range.start * columns)..(range.end * columns).min(v.library.order.len());
                }
                range
                    .map(|row| {
                        let mut line = div().flex().items_stretch().gap_2().pb_2().h(
                            if v.preferences.library.list {
                                px(61.)
                            } else {
                                px(182.)
                            },
                        );
                        for cell in row * columns..(row + 1) * columns {
                            if let Some(&i) = v.library.order.get(cell) {
                                line = line.child(v.library_cell(&v.library.entries[i], cx));
                            } else {
                                line = line.child(div().flex_1().min_w_0());
                            }
                        }
                        line
                    })
                    .collect()
            }),
        )
        .flex_1()
        .min_h_0()
        .w_full()
    }

    fn library_cell(&self, entry: &Entry, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let p = self.themes.active().palette;
        let id = entry.game.id;
        let title = div()
            .truncate()
            .font(self.fonts.medium())
            .text_sm()
            .child(entry.title.clone());
        let discs = entry.disc_count();
        let mut tags = entry.region.iter().cloned().collect::<Vec<_>>();
        if discs > 1 {
            tags.push(format!("{} {discs}", self.tr(Text::LibraryDiscs)));
        }
        if entry.pending {
            tags.push(self.tr(Text::LibraryPending).into());
        }
        let subtitle = label(&p, tags.join(" · ")).truncate();
        let mut cell = div()
            .id(("library-entry", id as u64))
            .debug_selector(move || format!("library-entry-{id}"))
            .tab_index(0)
            .flex_1()
            .min_w_0()
            .flex()
            .rounded_md()
            .border_1()
            .bg(rgb(p.surface))
            .border_color(rgb(if self.library.selected == Some(id) {
                p.primary
            } else {
                p.border
            }))
            .focus(move |s| s.border_color(rgb(p.primary)))
            .hover(move |s| s.bg(rgb(p.tonal_hover)))
            .cursor_pointer()
            .on_click(cx.listener(move |v, event: &gpui::ClickEvent, _, cx| {
                v.library.select(id);
                if event.is_keyboard() || event.click_count() >= 2 {
                    v.launch_library(id, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }));
        if self.preferences.library.list {
            cell = cell
                .items_center()
                .gap_3()
                .px_3()
                .child(div().flex_1().min_w_0().child(title).child(subtitle))
                .child(
                    div()
                        .w(px(100.))
                        .child(label(&p, self.tr(Text::LibraryDate)))
                        .child(
                            entry
                                .date
                                .clone()
                                .unwrap_or_else(|| self.tr(Text::LibraryUnknown).into()),
                        ),
                )
                .child(
                    div()
                        .w(px(145.))
                        .child(label(&p, self.tr(Text::LibraryPlayed)))
                        .child(self.played_at(entry.game.last_played)),
                )
                .child(
                    div()
                        .w(px(90.))
                        .child(label(&p, self.tr(Text::LibraryTime)))
                        .child(self.play_minutes(entry.game.seconds)),
                );
        } else {
            let cover = div()
                .flex_1()
                .min_h_0()
                .flex()
                .justify_center()
                .items_center()
                .rounded_md()
                .overflow_hidden()
                .bg(rgb(p.inset_surface()));
            let cover = if let Some(path) = &entry.thumbnail {
                // The cell owns its decoded image and atlas entry for its visible lifetime.
                cover.image_cache(gpui::retain_all("thumbnail")).child(
                    gpui::img(path.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
            } else {
                cover.child(icon("disc", p.muted).size(px(38.)))
            };
            cell = cell
                .flex_col()
                .p_2()
                .gap_1()
                .child(cover)
                .child(title)
                .child(subtitle);
        }
        cell
    }

    fn played_at(&self, timestamp: Option<i64>) -> String {
        timestamp
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map_or_else(
                || self.tr(Text::LibraryNever).to_owned(),
                |t| {
                    t.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                },
            )
    }

    fn play_minutes(&self, seconds: u64) -> String {
        format!("{} {}", seconds / 60, self.tr(Text::LibraryMinutes))
    }
}

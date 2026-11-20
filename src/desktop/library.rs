//! Desktop library browsing. File reads, media inspection and database writes run off the UI thread.
mod detail;
mod interactions;
mod ordering;
mod overview;
mod scraping;
mod selection;
mod settings;
mod tasks;
#[cfg(all(test, feature = "ui-tests"))]
mod tests;
mod tiles;

use self::detail::editor::Editor;
use self::overview::OverviewMenu;
use super::{
    Desktop,
    components::{action, button, card, label},
    icons::icon,
    text_input::TextInput,
};
use eidoryn_psx::{
    i18n::Text,
    library::{
        Game, GameMove, Library, ScanRoot,
        metadata::{self, Release},
    },
    preferences::LibrarySort as Sort,
};
use gpui::{Context, Div, Entity, Focusable, Window, div, prelude::*, px, rgb};
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) fn open() -> Result<Library, String> {
    let path = Library::default_path().ok_or("Cannot locate the game library data directory")?;
    Ok(Library::deferred(&path))
}

struct Entry {
    game: Game,
    title: String,
    search: String,
    sort_title: String,
    pending: bool,
    scrape_active: bool,
    source: Option<String>,
    refreshed: Option<i64>,
    date: Option<String>,
    work_date: Option<String>,
    region: Option<String>,
    thumbnail: Option<PathBuf>,
    serial: Option<String>,
    release_title: Option<String>,
    manual_fields: [bool; 3],
    manual_cover: bool,
    cover_source: Option<String>,
    cover_status: Option<String>,
    discs: usize,
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
            terms.extend(meta.region.clone());
        }
        terms.extend(game.locations.iter().filter_map(|loc| loc.serial.clone()));
        terms.extend(game.serial.clone());
        terms.extend(game.region.clone());
        terms.extend(metadata.as_ref().and_then(|m| m.serial.clone()));
        let discs = game
            .locations
            .iter()
            .map(|loc| loc.disc_number)
            .collect::<HashSet<_>>()
            .len();
        Self {
            discs,
            sort_title: title.to_lowercase(),
            pending: metadata.as_ref().is_some_and(|m| m.pending),
            scrape_active: metadata.as_ref().is_some_and(|m| m.scrape_active),
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
            release_title: metadata.as_ref().and_then(|m| m.release_title.clone()),
            manual_fields: metadata.as_ref().map_or([false; 3], |m| m.manual_fields),
            manual_cover: metadata.as_ref().is_some_and(|m| m.manual_cover),
            cover_source: metadata.as_ref().and_then(|m| m.cover_source.clone()),
            cover_status: metadata.and_then(|m| m.status),
            game,
        }
    }

    fn publication_date(&self) -> (Text, Option<&str>) {
        match (self.date.as_deref(), self.work_date.as_deref()) {
            (None, Some(date)) => (Text::LibraryWorkDate, Some(date)),
            (edition, _) => (Text::LibraryDate, edition),
        }
    }

    fn matched_release<'a>(&'a self, pending_label: &'a str) -> Option<&'a str> {
        self.pending
            .then_some(pending_label)
            .or(self.release_title.as_deref())
    }

    fn disc_count(&self) -> usize {
        self.discs
    }
}

#[derive(Clone, Copy)]
enum Confirm {
    Remove(i64),
    Clear(i64),
}

#[derive(Debug, PartialEq, Eq)]
enum MetadataLookup {
    Fetching,
    Finished(Text),
    Failed(String),
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
    pending_moves: VecDeque<GameMove>,
    order_revision: u64,
    checked_games: Option<BTreeSet<i64>>,
    card_menu: Option<interactions::CardMenu>,
    menu: Option<OverviewMenu>,
    source_menu_open: bool,
    template_field: Option<settings::TemplateField>,
    task_history_open: bool,
    search: Option<Entity<TextInput>>,
    search_subscription: Option<gpui::Subscription>,
    query: String,
    #[cfg(all(test, feature = "ui-tests"))]
    rendered_cells: std::ops::Range<usize>,
    loaded: bool,
    last_refresh: std::time::Instant,
    reading: bool,
    reload_pending: bool,
    dirty_games: BTreeSet<i64>,
    thumbnails: VecDeque<(PathBuf, Entity<gpui::RetainAllImageCache>)>,
    selected: Option<i64>,
    details: bool,
    location: Option<i64>,
    pending_bios: Option<(PathBuf, bool)>,
    pending_directory: Option<tasks::FolderImport>,
    recursive: bool,
    scan: Option<Arc<AtomicBool>>,
    cover: Option<tasks::CoverTask>,
    scraping: scraping::Scraping,
    auto_attempted: HashSet<(i64, String)>,
    report: tasks::Report,
    direct: Option<tasks::DirectChoice>,
    choices: Vec<eidoryn_psx::library::TrackChoice>,
    editor: Option<Editor>,
    matches: Vec<Release>,
    match_generation: u64,
    launch_generation: u64,
    metadata_lookups: HashMap<i64, MetadataLookup>,
    confirmation: Option<Confirm>,
    settings: bool,
}

impl Workspace {
    pub(super) fn new(mut store: Option<Library>) -> Self {
        Self {
            scraping: scraping::Scraping::new(store.as_mut()),
            store,
            entries: vec![],
            roots: vec![],
            order: vec![],
            pending_moves: VecDeque::new(),
            order_revision: 0,
            checked_games: None,
            card_menu: None,
            menu: None,
            source_menu_open: false,
            template_field: None,
            task_history_open: false,
            search: None,
            search_subscription: None,
            query: String::new(),
            #[cfg(all(test, feature = "ui-tests"))]
            rendered_cells: 0..0,
            loaded: false,
            last_refresh: std::time::Instant::now(),
            reading: false,
            reload_pending: false,
            dirty_games: BTreeSet::new(),
            thumbnails: VecDeque::new(),
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
            launch_generation: 0,
            metadata_lookups: HashMap::new(),
            confirmation: None,
            settings: false,
        }
    }

    pub(super) fn subpage_title(&self) -> Option<Text> {
        if self.settings {
            Some(Text::LibrarySettings)
        } else {
            self.details.then_some(Text::LibraryDetails)
        }
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(store) = &self.store {
            store.request_scrape_shutdown();
        }
        // Scans and individual cover requests have no durable leases. Stop their local I/O.
        if let Some(scan) = &self.scan {
            scan.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(cover) = &self.cover {
            cover
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // Each GPUI cache owns one decoded thumbnail and its texture. Retain recent
    // viewports across scrolling, and release the oldest entry at this bound.
    fn thumbnail_cache(
        &mut self,
        index: usize,
        cx: &mut Context<Desktop>,
    ) -> Option<Entity<gpui::RetainAllImageCache>> {
        let path = self.entries[index].thumbnail.as_ref()?;
        let entry = if let Some(old) = self.thumbnails.iter().position(|(key, _)| key == path) {
            self.thumbnails.remove(old)?
        } else {
            if self.thumbnails.len() >= 64 {
                self.thumbnails.pop_front();
            }
            (path.clone(), gpui::RetainAllImageCache::new(cx))
        };
        let cache = entry.1.clone();
        self.thumbnails.push_back(entry);
        Some(cache)
    }

    fn visible_selection(&mut self) -> Option<usize> {
        let id = self.selected?;
        let index = self
            .order
            .iter()
            .copied()
            .find(|&index| self.entries[index].game.id == id);
        if index.is_none() {
            self.selected = None;
            self.location = None;
            self.pending_bios = None;
        }
        index
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
        self.library_action(id, key, None, primary, cx, callback)
    }

    fn library_action(
        &self,
        id: impl Into<gpui::ElementId>,
        key: Text,
        symbol: Option<&'static str>,
        primary: bool,
        cx: &Context<Self>,
        callback: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let id = id.into();
        action(
            &self.fonts,
            &self.library_palette(),
            id.clone(),
            self.tr(key),
            symbol,
            primary,
        )
        .role(gpui::Role::Button)
        .aria_label(self.tr(key))
        .debug_selector(move || id.to_string())
        .on_click(cx.listener(move |view, _, window, cx| {
            callback(view, window, cx);
            cx.stop_propagation();
        }))
    }

    pub(super) fn dismiss_library_context_menu(&mut self) -> bool {
        self.library.card_menu.take().is_some()
    }

    pub(super) fn enter_library(&mut self, cx: &mut Context<Self>) {
        self.library.details = false;
        self.library.settings = false;
        self.library.checked_games = None;
        self.library.card_menu = None;
        self.library.menu = None;
        self.library.task_history_open = false;
        self.library.source_menu_open = false;
        self.library.template_field = None;
        self.library.close_editor();
        // A new visit can retry elapsed download backoffs or an explicitly canceled batch.
        if self.library.cover.is_none() && !self.library.scraping.busy() {
            self.library.auto_attempted.clear();
        }
        self.reload_library(cx);
    }

    pub(super) fn library_input_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.library
            .search
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
            || self.library_editor_focused(window, cx)
            || self.library.template_field.as_ref().is_some_and(|field| {
                self.library.settings && field.input.read(cx).focus_handle(cx).is_focused(window)
            })
    }

    pub(super) fn library_editor_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.library
            .editor
            .as_ref()
            .is_some_and(|editor| editor.is_focused(window, cx))
    }

    fn library_search(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        let p = self.library_palette();
        let hint = self.tr(Text::LibrarySearch);
        if let Some(input) = &self.library.search {
            if !input.read(cx).matches_search_style(p, hint) {
                input.update(cx, |input, cx| input.set_search_style(p, hint, cx));
            }
            return input.clone();
        }
        let input = cx.new(|cx| {
            TextInput::named("library-search", String::new(), p, cx).with_search_hint(hint)
        });
        self.library.search_subscription = Some(cx.observe(&input, |view, input, cx| {
            let query = input.read(cx).value().trim().to_lowercase();
            if query != view.library.query {
                view.library.query = query;
                view.library.card_menu = None;
                view.reorder_library();
                cx.notify();
            }
        }));
        self.library.search = Some(input.clone());
        input
    }

    fn library_launch_controls(
        &self,
        button_id: &'static str,
        game_id: i64,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(self.library_action(
                button_id,
                Text::LibraryStart,
                Some("play"),
                true,
                cx,
                move |v, _, cx| {
                    v.launch_library(game_id, cx);
                },
            ))
            .when_some(
                self.library.pending_bios.clone(),
                |row, (path, fast_boot)| {
                    row.child(self.library_button(
                        "lib-bios-continue",
                        Text::LibraryBiosContinue,
                        true,
                        cx,
                        move |v, _, cx| {
                            v.library_picker(tasks::Picker::Bios(path.clone(), fast_boot), cx);
                        },
                    ))
                },
            )
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

    fn library_duration(&self, entry: &Entry) -> String {
        if entry.game.last_played.is_none() && entry.game.seconds == 0 {
            return self.tr(Text::LibraryNever).to_owned();
        }
        let minutes = entry.game.seconds / 60;
        if minutes < 60 {
            format!("{minutes} {}", self.tr(Text::LibraryMinutes))
        } else {
            format!(
                "{} {} {} {}",
                minutes / 60,
                self.tr(Text::LibraryHours),
                minutes % 60,
                self.tr(Text::LibraryMinutes)
            )
        }
    }
}

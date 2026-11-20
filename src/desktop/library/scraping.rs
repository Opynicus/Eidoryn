//! Schedule manual refreshes and newly committed imports on the background executor.
use super::{Desktop, Text, div, label};
use eidoryn_psx::library::{
    Library,
    scrape::{self, Mode, Report},
};
use gpui::{Context, Div, prelude::*};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

struct Active {
    mode: Mode,
    ids: BTreeSet<i64>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    displayed: usize,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(super) struct Scraping {
    additions: Option<Receiver<i64>>,
    incremental: BTreeSet<i64>,
    full: BTreeSet<i64>,
    pub(super) preparing_full: bool,
    active: Option<Active>,
    pub(super) last: Option<(Mode, Report, bool)>,
    next_poll: Instant,
}

impl Scraping {
    pub(super) fn new(store: Option<&mut Library>) -> Self {
        Self {
            additions: store.map(Library::watch_additions),
            incremental: BTreeSet::new(),
            full: BTreeSet::new(),
            preparing_full: false,
            active: None,
            last: None,
            next_poll: Instant::now(),
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.preparing_full
            || self.active.is_some()
            || !self.full.is_empty()
            || !self.incremental.is_empty()
    }

    pub(super) fn full_requested(&self) -> bool {
        self.preparing_full
            || !self.full.is_empty()
            || self.active.as_ref().is_some_and(|a| a.mode == Mode::Full)
    }

    pub(super) fn include_full_library(&mut self, entries: &[super::Entry]) {
        self.full.extend(entries.iter().map(|e| e.game.id));
    }

    fn collect_additions(&mut self, enabled: bool) {
        if let Some(rx) = &self.additions {
            for id in rx.try_iter() {
                if enabled
                    && !self.full.contains(&id)
                    && !self.active.as_ref().is_some_and(|a| a.ids.contains(&id))
                {
                    self.incremental.insert(id);
                }
            }
        }
        if !enabled {
            self.incremental.clear();
            if let Some(active) = &self.active
                && active.mode == Mode::Incremental
            {
                active.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

impl Desktop {
    pub(super) fn refresh_library_data(&mut self, cx: &mut Context<Self>) {
        if self.library.scraping.full_requested() {
            return;
        }
        if let Some(task) = &self.library.cover
            && !task.chosen_edition
        {
            task.cancel.store(true, Ordering::Relaxed);
        }
        self.library.scraping.preparing_full = true;
        self.reload_library(cx);
        cx.notify();
    }

    pub(in crate::desktop) fn tick_library_scrape(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if now >= self.library.scraping.next_poll {
            self.library.scraping.next_poll = now + Duration::from_millis(200);
            self.poll_library_scrape(cx);
        }
    }

    pub(super) fn poll_library_scrape(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.library.scraping;
        state.collect_additions(self.preferences.library.scrape_new_games);
        if let Some(active) = &mut state.active {
            let done = active.done.load(Ordering::Relaxed);
            if active.displayed != done {
                active.displayed = done;
                if self.page == crate::desktop::WorkspacePage::Library {
                    cx.notify();
                }
            }
            return;
        }
        if state.preparing_full || self.library.cover.is_some() {
            return;
        }
        let (mode, ids) = if !state.full.is_empty() {
            state.incremental.retain(|id| !state.full.contains(id));
            (Mode::Full, std::mem::take(&mut state.full))
        } else if !state.incremental.is_empty() && self.library.scan.is_none() {
            (Mode::Incremental, std::mem::take(&mut state.incremental))
        } else {
            return;
        };
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let ids_to_run = ids.iter().copied().collect::<Vec<_>>();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicUsize::new(0));
        state.active = Some(Active {
            mode,
            ids,
            cancel: cancel.clone(),
            done: done.clone(),
            displayed: 0,
        });
        state.last = None;
        let prefs = &self.preferences.library;
        let provider = prefs.cover_provider;
        let template = prefs.cover_template.clone();
        cx.spawn(async move |this, cx| {
            let (report, cancelled) = cx
                .background_executor()
                .spawn(async move {
                    let report = scrape::run(
                        &store,
                        &ids_to_run,
                        mode,
                        provider,
                        &template,
                        &cancel,
                        |finished, _| {
                            done.store(finished, Ordering::Relaxed);
                        },
                    );
                    (report, cancel.load(Ordering::Relaxed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(active) = view.library.scraping.active.take() {
                    // Browsing can backfill missing covers separately. It should not repeat
                    // the provider requests that this batch has already made.
                    if !cancelled {
                        for entry in &view.library.entries {
                            if active.ids.contains(&entry.game.id)
                                && let Some(serial) = &entry.serial
                            {
                                view.library
                                    .auto_attempted
                                    .insert((entry.game.id, serial.clone()));
                            }
                        }
                    }
                }
                view.library.scraping.last = Some((mode, report, cancelled));
                view.poll_library_scrape(cx);
                view.reload_library(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn scrape_status(&self, cx: &Context<Self>) -> Div {
        let state = &self.library.scraping;
        let palette = self.themes.active().palette;
        let title = |mode| {
            self.tr(match mode {
                Mode::Full => Text::LibraryRefreshAll,
                Mode::Incremental => Text::LibraryScrapeNew,
            })
        };
        let row = div().flex().items_center().gap_2();
        if let Some(active) = &state.active {
            let cancel = active.cancel.clone();
            return row
                .child(label(
                    &palette,
                    format!(
                        "{} · {}/{}",
                        title(active.mode),
                        active.displayed,
                        active.ids.len()
                    ),
                ))
                .child(self.library_button(
                    "lib-cancel-scrape",
                    Text::LibraryCancel,
                    false,
                    cx,
                    move |_, _, _| cancel.store(true, Ordering::Relaxed),
                ));
        }
        if state.full_requested() {
            return row.child(label(&palette, self.tr(Text::LibraryRefreshQueued)));
        }
        if let Some((mode, report, cancelled)) = &state.last {
            let status = self.tr(if *cancelled {
                Text::LibraryScrapeCancelled
            } else {
                Text::LibraryScrapeFinished
            });
            return div()
                .flex()
                .flex_col()
                .child(label(
                    &palette,
                    format!(
                        "{} · {status} {}/{} · {} {} · {} {}",
                        title(*mode),
                        report.finished,
                        report.total,
                        self.tr(Text::LibrarySkipped),
                        report.skipped,
                        self.tr(Text::LibraryFailed),
                        report.failed
                    ),
                ))
                .children(report.errors.iter().map(|e| label(&palette, e.clone())));
        }
        row
    }
}

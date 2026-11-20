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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

/// Coalesce bursts while the UI is busy. A lagged view can always read SQLite.
#[derive(Default)]
struct Updates {
    ids: BTreeSet<i64>,
    overflow: bool,
    finished: usize,
    failed: usize,
    current: Option<i64>,
}

impl Updates {
    fn record(&mut self, event: &scrape::Event) {
        self.finished = event.finished;
        self.failed = event.failed;
        if let Some(id) = event.started {
            self.current = Some(id);
        }
        if let Some(id) = event.game_id {
            if self.ids.len() < 256 {
                self.ids.insert(id);
            } else {
                self.overflow = true;
            }
        }
    }
}

struct Active {
    mode: Mode,
    ids: BTreeSet<i64>,
    cancel: Arc<AtomicBool>,
    updates: Arc<Mutex<Updates>>,
    displayed: usize,
    failed: usize,
    current: Option<i64>,
}

pub(super) struct Scraping {
    additions: Option<Receiver<i64>>,
    incremental: BTreeSet<i64>,
    full: BTreeSet<i64>,
    diagnostics: Vec<String>,
    failed: BTreeSet<i64>,
    reading_queue: bool,
    queue_revision: u64,
    next_recovery: Instant,
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
            diagnostics: Vec::new(),
            failed: BTreeSet::new(),
            reading_queue: false,
            queue_revision: 0,
            next_recovery: Instant::now(),
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

    pub(super) fn is_queued(&self, id: i64) -> bool {
        self.full.contains(&id)
            || self.incremental.contains(&id)
            || self.active.as_ref().is_some_and(|a| a.ids.contains(&id))
    }

    pub(super) fn stop_automatic(&mut self) {
        self.incremental.clear();
        if let Some(active) = &self.active
            && active.mode == Mode::Incremental
        {
            active.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn has_failures(&self) -> bool {
        !self.failed.is_empty()
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
        self.enqueue_library_jobs(None, cx);
    }

    fn enqueue_library_jobs(&mut self, ids: Option<Vec<i64>>, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let provider = self.preferences.library.cover_provider;
        let all = ids.is_none();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let ids = match ids {
                        Some(ids) => ids,
                        None => store.visible_game_ids()?,
                    };
                    scrape::enqueue_games(&store, &ids, Mode::Full, provider)?;
                    Ok::<_, String>(ids)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.library.scraping;
                if all {
                    state.preparing_full = false;
                }
                match result {
                    Ok(ids) => {
                        state.full.extend(ids);
                        view.poll_library_scrape(cx);
                    }
                    Err(e) => view.notice = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::desktop) fn tick_library_scrape(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if now >= self.library.scraping.next_poll {
            self.library.scraping.next_poll = now + Duration::from_millis(200);
            self.poll_library_scrape(cx);
        }
    }

    fn read_scrape_queue(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let state = &mut self.library.scraping;
        if state.reading_queue || Instant::now() < state.next_recovery {
            return;
        }
        state.reading_queue = true;
        let revision = state.queue_revision;
        state.next_recovery = Instant::now() + Duration::from_secs(3);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Ok::<_, String>((
                        scrape::due_games(&store)?,
                        scrape::failed_games(&store)?,
                        scrape::recent_failures(&store)?,
                    ))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                let state = &mut v.library.scraping;
                state.reading_queue = false;
                if revision != state.queue_revision {
                    // A batch that started after this read may already have consumed these jobs.
                    return;
                }
                match result {
                    Ok((due, failed, diagnostics)) => {
                        let diagnostics = diagnostics
                            .into_iter()
                            .map(|(id, error)| {
                                let title = v
                                    .library
                                    .entries
                                    .iter()
                                    .find(|e| e.game.id == id)
                                    .map_or_else(|| id.to_string(), |e| e.title.clone());
                                format!("{title} · {error}")
                            })
                            .collect();
                        let failed = failed.into_iter().collect();
                        if state.failed != failed || state.diagnostics != diagnostics {
                            state.diagnostics = diagnostics;
                            state.failed = failed;
                            if v.page == crate::desktop::WorkspacePage::Library {
                                cx.notify();
                            }
                        }
                        for (id, mode) in due {
                            if state.is_queued(id) {
                                continue;
                            }
                            match mode {
                                Mode::Full => {
                                    state.full.insert(id);
                                }
                                Mode::Incremental if v.preferences.library.scrape_new_games => {
                                    state.incremental.insert(id);
                                }
                                Mode::Incremental => {}
                            }
                        }
                        v.poll_library_scrape(cx);
                    }
                    Err(error) if v.notice.as_ref() != Some(&error) => {
                        v.notice = Some(error);
                        cx.notify();
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    fn consume_scrape_updates(&mut self, cx: &mut Context<Self>) {
        let Some(active) = &mut self.library.scraping.active else {
            return;
        };
        let mut pulse = active
            .updates
            .lock()
            .expect("scrape progress lock poisoned");
        let changed = active.displayed != pulse.finished
            || active.failed != pulse.failed
            || active.current != pulse.current;
        active.current = pulse.current;
        active.displayed = pulse.finished;
        active.failed = pulse.failed;
        let dirty = std::mem::take(&mut pulse.ids);
        let lagged = std::mem::take(&mut pulse.overflow);
        drop(pulse);
        if lagged {
            self.reload_library(cx);
        } else if !dirty.is_empty() {
            self.update_library_games(dirty, cx);
        }
        if changed && self.page == crate::desktop::WorkspacePage::Library {
            cx.notify();
        }
    }

    fn collect_library_additions(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.library.scraping;
        let added: Vec<_> = state
            .additions
            .iter()
            .flat_map(Receiver::try_iter)
            .collect();
        if self.preferences.library.scrape_new_games {
            for &id in &added {
                if !state.is_queued(id) {
                    state.incremental.insert(id);
                }
            }
        } else {
            state.incremental.clear();
        }
        if self
            .library
            .store
            .as_ref()
            .is_some_and(Library::take_additions_overflow)
        {
            self.library.scraping.next_recovery = Instant::now();
            self.reload_library(cx);
        } else if !added.is_empty() {
            self.update_library_games(added, cx);
        }
    }

    pub(in crate::desktop) fn poll_library_scrape(&mut self, cx: &mut Context<Self>) {
        self.collect_library_additions(cx);
        self.consume_scrape_updates(cx);
        let state = &mut self.library.scraping;
        if state.active.is_some() || state.preparing_full || self.library.cover.is_some() {
            return;
        }
        let (mode, ids) = if !state.full.is_empty() {
            state.incremental.retain(|id| !state.full.contains(id));
            (Mode::Full, std::mem::take(&mut state.full))
        } else if !state.incremental.is_empty() {
            (Mode::Incremental, std::mem::take(&mut state.incremental))
        } else {
            self.read_scrape_queue(cx);
            return;
        };
        let Some(store) = self.library.store.clone() else {
            return;
        };
        for entry in &mut self.library.entries {
            if ids.contains(&entry.game.id) {
                entry.scrape_active = true;
            }
        }
        let ids_to_run: Vec<_> = ids.iter().copied().collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(Mutex::new(Updates::default()));
        state.queue_revision = state.queue_revision.wrapping_add(1);
        state.active = Some(Active {
            mode,
            ids,
            cancel: cancel.clone(),
            updates: updates.clone(),
            displayed: 0,
            failed: 0,
            current: None,
        });
        state.last = None;
        let prefs = &self.preferences.library;
        let (provider, workers, cover_workers) = (
            prefs.cover_provider,
            prefs.scrape_workers,
            prefs.cover_workers,
        );
        let template = prefs.cover_template.clone();
        cx.spawn(async move |this, cx| {
            let (report, cancelled) = cx
                .background_executor()
                .spawn(async move {
                    let opts = scrape::Options {
                        mode,
                        provider,
                        template: &template,
                        workers,
                        cover_workers,
                    };
                    let progress = |e| {
                        updates
                            .lock()
                            .expect("scrape progress lock poisoned")
                            .record(&e);
                    };
                    let report =
                        scrape::resume_events(&store, &ids_to_run, opts, &cancel, progress);
                    (report, cancel.load(Ordering::Relaxed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.consume_scrape_updates(cx);
                if let Some(active) = view.library.scraping.active.take() {
                    if cancelled {
                        view.update_library_games(active.ids, cx);
                    } else {
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
                if report.failed > 0 || cancelled {
                    view.library.scraping.next_recovery = Instant::now();
                }
                view.library.scraping.last = Some((mode, report, cancelled));
                view.poll_library_scrape(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn retry_game(&mut self, id: i64, cx: &mut Context<Self>) {
        self.enqueue_library_jobs(Some(vec![id]), cx);
    }

    pub(super) fn cancel_game_scrape(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { scrape::cancel_games(&store, &[id]) })
                .await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(()) => {
                        v.library.scraping.full.remove(&id);
                        v.library.scraping.incremental.remove(&id);
                        v.library.scraping.next_recovery = Instant::now();
                        v.update_library_games([id], cx);
                        v.poll_library_scrape(cx);
                    }
                    Err(e) => v.notice = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn scrape_summary(&self, title: &str, report: &Report, cancelled: bool) -> String {
        let mut items = vec![
            title.to_owned(),
            format!(
                "{} {}/{}",
                self.tr(Text::LibraryScrapeFinished),
                report.finished,
                report.total
            ),
        ];
        for (key, count) in [
            (Text::LibrarySkipped, report.skipped),
            (Text::LibraryFailed, report.failed),
            (Text::LibraryRetryWait, report.retry_waiting),
            (Text::LibraryProcessing, report.pending),
            (Text::LibraryScrapeCancelled, report.cancelled),
        ] {
            if count > 0 {
                items.push(format!("{} {count}", self.tr(key)));
            }
        }
        if cancelled && report.cancelled == 0 {
            items.push(self.tr(Text::LibraryScrapeCancelled).to_owned());
        }
        items.join(" · ")
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
        let row = div().flex().flex_wrap().items_center().gap_2();
        let row = row.when(state.has_failures(), |row| {
            row.child(self.library_button(
                "lib-retry-failed",
                Text::LibraryRetryFailed,
                false,
                cx,
                |v, _, cx| {
                    let ids = v.library.scraping.failed.iter().copied().collect();
                    v.enqueue_library_jobs(Some(ids), cx);
                },
            ))
        });
        let mut errors = state.diagnostics.as_slice();
        let row = if let Some(active) = &state.active {
            let cancel = active.cancel.clone();
            let current = active
                .current
                .and_then(|id| self.library.entries.iter().find(|e| e.game.id == id));
            row.when_some(current, |row, entry| {
                row.child(label(&palette, entry.title.clone()).truncate())
            })
            .child(label(
                &palette,
                format!(
                    "{} · {}/{} · {} {}",
                    title(active.mode),
                    active.displayed,
                    active.ids.len(),
                    self.tr(Text::LibraryFailed),
                    active.failed
                ),
            ))
            .child(self.library_button(
                "lib-cancel-scrape",
                Text::LibraryCancel,
                false,
                cx,
                move |_, _, _| cancel.store(true, Ordering::Relaxed),
            ))
        } else if state.full_requested() {
            row.child(label(&palette, self.tr(Text::LibraryRefreshQueued)))
        } else if let Some((mode, report, cancelled)) = &state.last {
            if errors.is_empty() {
                errors = &report.errors;
            }
            row.child(label(
                &palette,
                self.scrape_summary(title(*mode), report, *cancelled),
            ))
        } else {
            row
        };
        div()
            .flex()
            .flex_col()
            .child(row)
            .when(!errors.is_empty(), |panel| {
                panel.child(
                    div()
                        .id("library-scrape-issues")
                        .max_h(gpui::px(84.))
                        .overflow_y_scroll()
                        .children(errors.iter().map(|e| label(&palette, e.clone()))),
                )
            })
    }
}

//! Schedule manual refreshes and newly committed imports on the background executor.
use super::{Desktop, Text, div, label};
use crate::desktop::components::ghost_button;
use eidoryn_psx::library::{
    Library,
    scrape::{self, Mode, Report},
};
use gpui::{Context, Div, prelude::*, px, rgb};
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
    history: Option<Result<Vec<scrape::JobRecord>, String>>,
    failed: BTreeSet<i64>,
    reading_queue: bool,
    queue_revision: u64,
    /// Enqueues and manual cache clears must commit in the order the user requested them.
    pub(super) queue_write: Option<gpui::Task<()>>,
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
            history: None,
            failed: BTreeSet::new(),
            reading_queue: false,
            queue_revision: 0,
            queue_write: None,
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
        if self.is_gba() || self.library.scraping.full_requested() {
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
        let previous = self.library.scraping.queue_write.take();
        self.library.scraping.queue_write = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let ids = match ids {
                        Some(ids) => ids,
                        None => store.visible_games_for_scraping()?,
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
        }));
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
        let load_history = self.library.task_history_open;
        state.next_recovery = Instant::now() + Duration::from_secs(3);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Ok::<_, String>((
                        scrape::due_games(&store)?,
                        scrape::failed_games(&store)?,
                        scrape::recent_failures(&store)?,
                        load_history.then(|| scrape::recent_jobs(&store)),
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
                    Ok((due, failed, diagnostics, history)) => {
                        if history.is_some() && state.history != history {
                            state.history = history;
                            cx.notify();
                        }
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
                        if load_history {
                            state.history = Some(Err(error.clone()));
                        }
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
        if !added.is_empty() {
            // Import persists eligible jobs in the same transaction as the game.
            // Waking the scheduler also lets GBA cards appear without queuing a scraper.
            state.next_recovery = Instant::now();
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

    fn scrape_title(&self, mode: Mode) -> &'static str {
        self.tr(match mode {
            Mode::Full => Text::LibraryRefreshAll,
            Mode::Incremental => Text::LibraryScrapeNew,
        })
    }

    pub(super) fn scrape_status(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let state = &self.library.scraping;
        if let Some(active) = &state.active {
            let cancel = active.cancel.clone();
            let mut detail = format!(
                "{} · {} {}/{} · {} {}",
                self.scrape_title(active.mode),
                self.tr(Text::LibraryScrapeFinished),
                active.displayed,
                active.ids.len(),
                self.tr(Text::LibraryFailed),
                active.failed
            );
            if let Some(game) = active
                .current
                .and_then(|id| self.library.entries.iter().find(|e| e.game.id == id))
            {
                detail.push_str(" · ");
                detail.push_str(&game.title);
            }
            let progress = active.displayed as f32 / active.ids.len().max(1) as f32;
            return self
                .task_banner("reset", Text::LibraryRefreshRunning, detail)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(progress_bar(&p, progress))
                        .child(self.library_button(
                            "lib-cancel-scrape",
                            Text::LibraryCancelRefresh,
                            false,
                            cx,
                            move |_, _, _| cancel.store(true, Ordering::Relaxed),
                        )),
                );
        }
        let (icon, title, description) = if state.full_requested() {
            (
                "reset",
                Text::LibraryRefreshRunning,
                self.tr(Text::LibraryRefreshQueued).to_owned(),
            )
        } else if !state.incremental.is_empty() {
            (
                "reset",
                Text::LibraryScrapeNew,
                format!(
                    "{} · {}",
                    self.tr(Text::LibraryScrapePending),
                    state.incremental.len()
                ),
            )
        } else {
            (
                "check",
                Text::LibraryStatusReady,
                self.tr(Text::LibraryStatusReadyHint).to_owned(),
            )
        };
        self.task_banner(icon, title, description).child(
            ghost_button(
                &self.fonts,
                &p,
                "lib-task-history",
                self.tr(Text::LibraryViewHistory),
                None,
            )
            .role(gpui::Role::Button)
            .aria_label(self.tr(Text::LibraryTaskHistory))
            .debug_selector(|| "lib-task-history".into())
            .on_click(cx.listener(|v, _, _, cx| {
                v.library.task_history_open = true;
                v.library.scraping.next_recovery = Instant::now();
                v.read_scrape_queue(cx);
                cx.notify();
            })),
        )
    }

    pub(super) fn task_history_dialog(&self, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let state = &self.library.scraping;
        let mut content = div()
            .id("lib-task-history-body")
            .max_h(px(388.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        if let Some((mode, report, cancelled)) = &state.last {
            content = content.child(label(
                &p,
                self.scrape_summary(self.scrape_title(*mode), report, *cancelled),
            ));
        }
        content = content.child(self.scan_report_panel());
        let errors = if state.diagnostics.is_empty() {
            state
                .last
                .as_ref()
                .map_or(&[][..], |(_, report, _)| report.errors.as_slice())
        } else {
            &state.diagnostics
        };
        content = content.children(
            errors
                .iter()
                .map(|error| label(&p, error.clone()).text_color(rgb(p.on_error))),
        );
        if state.has_failures() {
            content = content.child(div().flex().child(self.library_button(
                "lib-retry-failed",
                Text::LibraryRetryFailed,
                false,
                cx,
                |v, _, cx| {
                    let ids = v.library.scraping.failed.iter().copied().collect();
                    v.library.task_history_open = false;
                    v.enqueue_library_jobs(Some(ids), cx);
                },
            )));
        }
        content = content.child(
            div()
                .font(self.fonts.semibold())
                .text_sm()
                .child(self.tr(Text::LibraryRecentJobs)),
        );
        content = match &state.history {
            Some(Err(error)) => content.child(label(&p, error.clone()).text_color(rgb(p.on_error))),
            None => content.child(label(&p, self.tr(Text::LibraryLoading))),
            Some(Ok(rows)) if rows.is_empty() => {
                content.child(label(&p, self.tr(Text::LibraryTaskEmpty)))
            }
            Some(Ok(rows)) => content.children(rows.iter().map(|row| self.job_row(row))),
        };
        self.library_modal(self.tr(Text::LibraryTaskHistory), div().child(content), cx)
    }

    fn job_row(&self, row: &scrape::JobRecord) -> Div {
        use scrape::JobStatus;
        let p = self.library_palette();
        let stage = if row.cover {
            format!("{} · {}", self.tr(Text::LibraryCover), row.provider)
        } else {
            self.tr(Text::LibraryTaskMetadata).to_owned()
        };
        let status = match row.status {
            JobStatus::Pending => Text::LibraryScrapePending,
            JobStatus::Running => Text::LibraryProcessing,
            JobStatus::Succeeded if row.skipped => Text::LibrarySkipped,
            JobStatus::Succeeded => Text::LibraryScrapeFinished,
            JobStatus::RetryWait => Text::LibraryRetryWait,
            JobStatus::Failed => Text::LibraryFailed,
            JobStatus::Cancelled => Text::LibraryScrapeCancelled,
        };
        div()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(p.border))
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font(self.fonts.semibold())
                            .child(row.title.clone()),
                    )
                    .child(label(&p, self.tr(status)).flex_shrink_0()),
            )
            .child(label(
                &p,
                format!("{stage} · {}", self.played_at(Some(row.updated_at))),
            ))
    }
}

pub(super) fn progress_bar(p: &eidoryn_psx::theme::Palette, fraction: f32) -> Div {
    div()
        .w(px(112.))
        .h(px(5.))
        .rounded_full()
        .bg(rgb(p.border))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(gpui::relative(fraction.clamp(0., 1.)))
                .bg(rgb(p.primary)),
        )
}

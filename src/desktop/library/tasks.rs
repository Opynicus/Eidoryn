use super::{Desktop, Entry, Text, div, label};
use eidoryn_psx::{
    library::{Library, ScanPhase, ScanReport, TrackChoice, covers, metadata},
    systems::ps1::{media::validate_bios_path, runner::Command},
};
use gpui::{Context, Div, PathPromptOptions, prelude::*, px};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Report {
    counts: [usize; 4],
    processed: usize,
    total: usize,
    current: Option<String>,
    status: Option<Text>,
    errors: Vec<String>,
}

pub(super) enum Picker {
    Images,
    Folder,
    Cover(i64),
    Relocate(i64),
    Bios(PathBuf, bool),
}

pub(super) struct CoverTask {
    pub(super) cancel: Arc<AtomicBool>,
    // An explicit cover from another edition is a user override. A full refresh
    // can replace automatic downloads but must let this selection finish.
    pub(super) chosen_edition: bool,
}

pub(super) struct DirectChoice {
    cues: Vec<PathBuf>,
    fast_boot: bool,
    recent: bool,
}

enum Source {
    Files(Vec<PathBuf>),
    Directory(PathBuf, bool),
}

enum Update {
    Counts {
        phase: ScanPhase,
        counts: [usize; 4],
        processed: usize,
        total: usize,
        current: Option<String>,
    },
    Done(Report, Vec<TrackChoice>),
}

impl Desktop {
    pub(in crate::desktop) fn direct_disc(
        &mut self,
        path: PathBuf,
        fast_boot: bool,
        recent: bool,
        cx: &mut Context<Self>,
    ) {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
            && let Some(store) = self.library.store.clone()
        {
            cx.spawn(async move |this, cx| {
                let (path, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = store.track_choices(&path);
                        (path, result)
                    })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    match result {
                        Ok(cues) if !cues.is_empty() => {
                            v.library.direct = Some(DirectChoice {
                                cues,
                                fast_boot,
                                recent,
                            });
                            v.library.details = false;
                            v.library.settings = false;
                            v.presentation.windowed_video = false;
                            v.page = crate::desktop::WorkspacePage::Library;
                            v.reload_library(cx);
                        }
                        Ok(_) => v.send_direct_disc(path, fast_boot, recent, cx),
                        Err(error) => v.notice = Some(error),
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.send_direct_disc(path, fast_boot, recent, cx);
        }
    }

    fn send_direct_disc(
        &mut self,
        path: PathBuf,
        fast_boot: bool,
        recent: bool,
        cx: &mut Context<Self>,
    ) {
        if self.validate_disc_bios(fast_boot, cx) {
            let bios = self.bios.clone();
            let command = if recent {
                Command::ReopenDisc {
                    path,
                    bios,
                    fast_boot,
                }
            } else {
                Command::LoadDisc {
                    path,
                    bios,
                    fast_boot,
                }
            };
            self.command(command, cx);
        }
    }

    pub(in crate::desktop) fn drop_discs(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match paths.paths() {
            [path] if path.is_dir() => {
                self.show_page(crate::desktop::WorkspacePage::Library, window, cx);
                self.library.details = false;
                self.library.settings = false;
                self.library.pending_directory = Some(path.clone());
            }
            [path] => self.load_selected_disc(path.clone(), false, cx),
            paths if !paths.is_empty() => {
                self.show_page(crate::desktop::WorkspacePage::Library, window, cx);
                self.library.details = false;
                self.library.settings = false;
                self.scan_sources(vec![Source::Files(paths.to_vec())], cx);
            }
            _ => {}
        }
    }

    pub(super) fn clear_playtime(&mut self, id: i64, cx: &mut Context<Self>) {
        let (completed, rx) = mpsc::channel();
        if let Err(error) = self.runner.send(Command::ClearLibraryStatistics {
            game_id: id,
            completed,
        }) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { rx.recv().map_err(|e| e.to_string()).and_then(|r| r) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(error) = result {
                    v.notice = Some(error);
                }
                v.update_library_games([id], cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::desktop) fn refresh_playing_library(&mut self, cx: &mut Context<Self>) {
        if self.page == crate::desktop::WorkspacePage::Library
            && self.snapshot.disc_mounted
            && self.library.last_refresh.elapsed() >= Duration::from_secs(30)
        {
            self.library.last_refresh = Instant::now();
            if let Some(path) = &self.snapshot.disc_path
                && let Some(entry) = self
                    .library
                    .entries
                    .iter()
                    .find(|entry| entry.game.locations.iter().any(|l| &l.path == path))
            {
                self.update_library_games([entry.game.id], cx);
            }
        }
    }

    pub(in crate::desktop) fn reload_library(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        if self.library.reading {
            self.library.reload_pending = true;
            return;
        }
        self.library.last_refresh = Instant::now();
        self.library.dirty_games.clear();
        self.library.reading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut meta = metadata::list(&store)?;
                    let entries = store
                        .games()?
                        .into_iter()
                        .map(|game| {
                            let m = meta.remove(&game.id);
                            Entry::new(game, m)
                        })
                        .collect::<Vec<_>>();
                    Ok::<_, String>((entries, store.roots()?))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.library.reading = false;
                match result {
                    Ok((entries, roots)) => {
                        v.library.loaded = true;
                        v.library.entries = entries;
                        v.library.roots = roots;
                        if let Some(selected) = v.library.selected
                            && !v.library.entries.iter().any(|e| e.game.id == selected)
                        {
                            v.library.selected = None;
                            v.library.details = false;
                            v.library.location = None;
                            v.library.close_editor();
                        }
                        v.library.reorder();
                        v.poll_library_scrape(cx);
                        v.auto_covers(cx);
                    }
                    Err(error) => v.notice = Some(error),
                }
                v.read_pending_games(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn update_library_games(
        &mut self,
        ids: impl IntoIterator<Item = i64>,
        cx: &mut Context<Self>,
    ) {
        if self.library.reload_pending {
            return;
        }
        for id in ids {
            if self.library.dirty_games.len() == 512 && !self.library.dirty_games.contains(&id) {
                self.library.dirty_games.clear();
                self.reload_library(cx);
                return;
            }
            self.library.dirty_games.insert(id);
        }
        self.read_pending_games(cx);
    }

    fn read_pending_games(&mut self, cx: &mut Context<Self>) {
        if self.library.reading {
            return;
        }
        if std::mem::take(&mut self.library.reload_pending) {
            self.reload_library(cx);
            return;
        }
        let Some(store) = self.library.store.clone() else {
            return;
        };
        if self.library.dirty_games.is_empty() {
            return;
        }
        let ids: Vec<_> = (0..128)
            .map_while(|_| self.library.dirty_games.pop_first())
            .collect();
        self.library.reading = true;
        cx.spawn(async move |this, cx| {
            let (ids, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = (|| {
                        let mut meta = metadata::list_by_id(&store, &ids)?;
                        Ok::<_, String>(
                            store
                                .games_by_id(&ids)?
                                .into_iter()
                                .map(|game| {
                                    let data = meta.remove(&game.id);
                                    Entry::new(game, data)
                                })
                                .collect::<Vec<_>>(),
                        )
                    })();
                    (ids, result)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.library.reading = false;
                match result {
                    Ok(rows) => {
                        // The bounded list is sorted. Missing rows were removed or grouped.
                        v.library
                            .entries
                            .retain(|e| ids.binary_search(&e.game.id).is_err());
                        v.library.entries.extend(rows);
                        v.library.reorder();
                        if let Some(id) = v.library.selected
                            && ids.binary_search(&id).is_ok()
                            && !v.library.entries.iter().any(|e| e.game.id == id)
                        {
                            v.library.selected = None;
                            v.library.details = false;
                            v.library.close_editor();
                        }
                    }
                    Err(e) => v.notice = Some(e),
                }
                v.read_pending_games(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn library_write(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Library) -> Result<(), String> + Send + 'static,
    ) {
        let Some(store) = self.library.store.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&store) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(error) = result {
                    v.notice = Some(error);
                }
                v.reload_library(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn library_picker(&mut self, picker: Picker, cx: &mut Context<Self>) {
        let prompt = self.tr(match picker {
            Picker::Images => Text::LibraryAddImages,
            Picker::Folder => Text::LibraryAddFolder,
            Picker::Cover(_) => Text::LibraryLocalCover,
            Picker::Relocate(_) => Text::LibraryConfirmRelocate,
            Picker::Bios(..) => Text::LibraryBiosContinue,
        });
        let folder = matches!(picker, Picker::Folder);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            directories: folder,
            files: !folder,
            multiple: matches!(picker, Picker::Images),
            prompt: Some(prompt.into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match paths.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |v, cx| {
                        v.notice = Some(error.to_string());
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |v, cx| {
                        v.notice = Some(error.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(first) = paths.first().cloned() else {
                return;
            };
            if let Picker::Bios(disc, fast_boot) = picker {
                let result = cx
                    .background_executor()
                    .spawn(async move { validate_bios_path(&first) })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    match result {
                        Ok(bios)
                            if v.library.pending_bios.as_ref()
                                == Some(&(disc.clone(), fast_boot)) =>
                        {
                            v.notice = None;
                            v.bios = Some(bios);
                            v.library.pending_bios = None;
                            v.command(
                                Command::LaunchLibraryDisc {
                                    path: disc,
                                    bios: v.bios.clone(),
                                    fast_boot,
                                },
                                cx,
                            );
                        }
                        Ok(_) => {}
                        Err(error) => v.notice = Some(error),
                    }
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |v, cx| {
                match picker {
                    Picker::Images => v.scan_sources(vec![Source::Files(paths)], cx),
                    Picker::Folder => v.library.pending_directory = Some(first),
                    Picker::Cover(id) => {
                        v.library_write(cx, move |s| covers::set_local(s, id, &first));
                    }
                    Picker::Relocate(id) => v.library_write(cx, move |s| s.relocate(id, &first)),
                    Picker::Bios(..) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn launch_library(&mut self, id: i64, cx: &mut Context<Self>) {
        self.library.select(id);
        let Some(game) = self
            .library
            .entries
            .iter()
            .find(|e| e.game.id == id)
            .map(|e| &e.game)
        else {
            return;
        };
        let Some(loc) = game
            .locations
            .iter()
            .find(|l| Some(l.id) == self.library.location)
            .or_else(|| game.locations.iter().find(|l| l.available))
            .or_else(|| game.locations.first())
        else {
            return;
        };
        let path = loc.path.clone();
        let fast_boot = loc.fast_boot;
        self.library.location = Some(loc.id);
        self.library.pending_bios = None;
        let location = loc.id;
        let available = loc.available;
        let bios = self.bios.clone();
        self.library.launch_generation = self.library.launch_generation.wrapping_add(1);
        let generation = self.library.launch_generation;
        cx.spawn(async move |this, cx| {
            let (path, bios, disc_available, choice, validation) = cx
                .background_executor()
                .spawn(async move {
                    let disc_available = available && path.is_file();
                    let mut choice = bios.clone();
                    let validation = if disc_available {
                        eidoryn_psx::preferences::validate_bios_choice(&mut choice)
                    } else {
                        Ok(None)
                    };
                    (path, bios, disc_available, choice, validation)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.library.launch_generation != generation
                    || v.library.selected != Some(id)
                    || v.library.location != Some(location)
                    || v.bios != bios
                    || v.page != crate::desktop::WorkspacePage::Library
                {
                    return;
                }
                if !disc_available {
                    v.library_picker(Picker::Relocate(location), cx);
                    v.notice = Some(v.tr(Text::LibraryMissing).to_owned());
                } else if bios.is_some() && choice.is_none() {
                    v.bios = None;
                    v.notice = Some(v.tr(Text::MissingBios).into());
                    if v.preferences.bios_path == bios {
                        v.preferences.bios_path = None;
                        v.save_preferences();
                    }
                    v.library.pending_bios = Some((path, fast_boot));
                } else if let Err(error) = validation {
                    v.notice = Some(error);
                    v.library.pending_bios = Some((path, fast_boot));
                } else if !fast_boot && choice.is_none() {
                    v.notice = Some(v.tr(Text::BiosRequired).into());
                    v.library.pending_bios = Some((path, fast_boot));
                } else {
                    v.command(
                        Command::LaunchLibraryDisc {
                            path,
                            bios: choice,
                            fast_boot,
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn scan_roots(&mut self, cx: &mut Context<Self>) {
        let sources = self
            .library
            .roots
            .iter()
            .map(|r| Source::Directory(r.path.clone(), r.recursive))
            .collect();
        self.scan_sources(sources, cx);
    }

    fn scan_sources(&mut self, sources: Vec<Source>, cx: &mut Context<Self>) {
        if self.library.scan.is_some() || sources.is_empty() {
            return;
        }
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.library.scan = Some(cancel.clone());
        self.library.report = Report {
            status: Some(Text::LibraryScanning),
            ..Report::default()
        };
        if !sources
            .iter()
            .all(|source| matches!(source, Source::Files(_)))
        {
            self.library.choices.clear();
        }
        let (tx, rx) = mpsc::sync_channel(8);
        cx.background_executor()
            .spawn(async move {
                scan(&store, sources, &cancel, &tx);
            })
            .detach();
        cx.spawn(async move |this, cx| {
            loop {
                let mut finished = false;
                for update in rx.try_iter() {
                    if this
                        .update(cx, |v, cx| {
                            match update {
                                Update::Counts {
                                    phase,
                                    counts,
                                    processed,
                                    total,
                                    current,
                                } => {
                                    v.library.report.status = Some(match phase {
                                        ScanPhase::Discovering => Text::LibraryDiscovering,
                                        ScanPhase::Indexing => Text::LibraryScanning,
                                        ScanPhase::CheckingAvailability => {
                                            Text::LibraryCheckingPaths
                                        }
                                    });
                                    v.library.report.counts = counts;
                                    v.library.report.processed = processed;
                                    v.library.report.total = total;
                                    v.library.report.current = current;
                                }
                                Update::Done(report, choices) => {
                                    v.library.report = report;
                                    v.library.choices.extend(choices);
                                    v.library.scan = None;
                                    finished = true;
                                }
                            }
                            if finished {
                                // Reconcile missing locations, tracks absorbed by CUEs, and roots.
                                v.reload_library(cx);
                            } else {
                                v.poll_library_scrape(cx);
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                if finished || this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn auto_covers(&mut self, cx: &mut Context<Self>) {
        if self.page != crate::desktop::WorkspacePage::Library
            || !self.preferences.library.auto_covers
            || self.library.cover.is_some()
            || self.library.scan.is_some()
            || self.library.scraping.busy()
        {
            return;
        }
        let ids = self
            .library
            .entries
            .iter()
            .filter(|e| {
                e.thumbnail.is_none()
                    && e.serial.as_ref().is_some_and(|serial| {
                        self.library
                            .auto_attempted
                            .insert((e.game.id, serial.clone()))
                    })
            })
            .map(|e| e.game.id)
            .collect();
        self.fetch_covers(ids, cx);
    }

    pub(super) fn fetch_covers(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        self.fetch_cover_editions(ids, None, cx);
    }

    pub(super) fn fetch_cover_editions(
        &mut self,
        ids: Vec<i64>,
        edition: Option<metadata::Release>,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() || self.library.cover.is_some() || self.library.scraping.busy() {
            return;
        }
        let Some(store) = self.library.store.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.library.cover = Some(CoverTask {
            cancel: cancel.clone(),
            chosen_edition: edition.is_some(),
        });
        let provider = self.preferences.library.cover_provider;
        let template: Arc<str> = self.preferences.library.cover_template.as_str().into();
        let edition = edition.map(Arc::new);
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            let mut changed = Vec::new();
            let mut last_update = Instant::now();
            for id in ids {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let store = store.clone();
                let cancel_fetch = cancel.clone();
                let template = template.clone();
                let edition = edition.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        match edition.as_deref() {
                            Some(release) => covers::fetch_release(
                                &store,
                                id,
                                provider,
                                &template,
                                release,
                                &cancel_fetch,
                            ),
                            None => covers::fetch(&store, id, provider, &template, &cancel_fetch),
                        }
                    })
                    .await;
                changed.push(id);
                if !cancel.load(Ordering::Relaxed)
                    && let Err(error) = result
                    && errors.len() < 8
                    && !errors.contains(&error)
                {
                    errors.push(error);
                }
                if changed.len() >= 64 || last_update.elapsed() >= Duration::from_millis(100) {
                    if this
                        .update(cx, |v, cx| v.update_library_games(changed.drain(..), cx))
                        .is_err()
                    {
                        return;
                    }
                    last_update = Instant::now();
                }
            }
            let _ = this.update(cx, |v, cx| {
                v.library.cover = None;
                if !errors.is_empty() {
                    v.notice = Some(errors.join("\n"));
                }
                v.update_library_games(changed, cx);
                v.poll_library_scrape(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn library_scan_status(&self, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        let mut panel = div().flex().flex_col().gap_1();
        if let Some(path) = &self.library.pending_directory {
            let path = path.clone();
            panel = panel.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(label(
                        &p,
                        format!("{} · {}", self.tr(Text::LibraryFolder), path.display()),
                    ))
                    .child(self.library_button(
                        "lib-recursive",
                        Text::LibraryRecursive,
                        self.library.recursive,
                        cx,
                        |v, _, cx| {
                            v.library.recursive = !v.library.recursive;
                            cx.notify();
                        },
                    ))
                    .child(self.library_button(
                        "lib-confirm-folder",
                        Text::LibraryScan,
                        true,
                        cx,
                        move |v, _, cx| {
                            if v.library.scan.is_none() {
                                v.library.pending_directory = None;
                                v.scan_sources(
                                    vec![Source::Directory(path.clone(), v.library.recursive)],
                                    cx,
                                );
                            }
                        },
                    ))
                    .child(self.library_button(
                        "lib-cancel-folder",
                        Text::LibraryCancel,
                        false,
                        cx,
                        |v, _, cx| {
                            v.library.pending_directory = None;
                            cx.notify();
                        },
                    )),
            );
        }
        if let Some(choice) = &self.library.direct {
            let (recent, fast) = (choice.recent, choice.fast_boot);
            panel = panel
                .child(label(&p, self.tr(Text::LibraryTrackChoice)))
                .children(choice.cues.iter().enumerate().map(|(i, path)| {
                    let path = path.clone();
                    super::button(
                        &self.fonts,
                        &p,
                        ("lib-open-cue", i),
                        path.display().to_string(),
                        false,
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.library.direct = None;
                        v.direct_disc(path.clone(), fast, recent, cx);
                    }))
                }))
                .child(self.library_button(
                    "lib-cancel-track",
                    Text::LibraryCancel,
                    false,
                    cx,
                    |v, _, cx| {
                        v.library.direct = None;
                        cx.notify();
                    },
                ));
        }
        let panel = self.scan_report_panel(panel, cx);
        if self.library.scraping.busy()
            || self.library.scraping.last.is_some()
            || self.library.scraping.has_failures()
        {
            panel.child(self.scrape_status(cx))
        } else {
            panel
        }
    }
    fn scan_report_panel(&self, mut panel: Div, cx: &Context<Self>) -> Div {
        let p = self.themes.active().palette;
        if let Some(status) = self.library.report.status {
            let titles = [
                Text::LibraryAdded,
                Text::LibraryExisting,
                Text::LibraryPending,
                Text::LibraryFailed,
            ];
            let counts = titles
                .into_iter()
                .zip(self.library.report.counts)
                .map(|(key, n)| format!("{} {n}", self.tr(key)))
                .collect::<Vec<_>>()
                .join(" · ");
            let scan = &self.library.report;
            let current = scan.current.as_deref().unwrap_or("");
            panel = panel.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(label(
                        &p,
                        format!(
                            "{} {}/{} · {counts}  {current}",
                            self.tr(status),
                            scan.processed,
                            scan.total
                        ),
                    ))
                    .when_some(self.library.scan.clone(), |s, cancel| {
                        s.child(self.library_button(
                            "lib-cancel-scan",
                            Text::LibraryCancel,
                            false,
                            cx,
                            move |_, _, _| cancel.store(true, Ordering::Relaxed),
                        ))
                    }),
            );
        }
        if self.library.cover.is_some() {
            panel = panel.child(self.cover_progress(cx));
        }
        if !self.library.report.errors.is_empty()
            || !self.library.choices.is_empty()
            || self.library.roots.iter().any(|r| !r.available)
        {
            panel = panel.child(
                div()
                    .id("library-scan-issues")
                    .max_h(px(100.))
                    .overflow_y_scroll()
                    .children(
                        self.library
                            .report
                            .errors
                            .iter()
                            .map(|e| label(&p, e.clone())),
                    )
                    .children(self.library.roots.iter().filter(|r| !r.available).map(|r| {
                        label(
                            &p,
                            format!("{} · {}", self.tr(Text::LibraryOffline), r.path.display()),
                        )
                    }))
                    .children(self.library.choices.iter().enumerate().map(|(i, choice)| {
                        div()
                            .flex()
                            .flex_col()
                            .child(label(
                                &p,
                                format!(
                                    "{} · {}",
                                    self.tr(Text::LibraryTrackChoice),
                                    choice.track.display()
                                ),
                            ))
                            .children(choice.cues.iter().enumerate().map(|(j, path)| {
                                let path = path.clone();
                                let track = choice.track.clone();
                                super::button(
                                    &self.fonts,
                                    &p,
                                    (gpui::SharedString::from(format!("library-cue-{i}")), j),
                                    path.display().to_string(),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |v, _, _, cx| {
                                        v.library.choices.retain(|choice| choice.track != track);
                                        v.scan_sources(vec![Source::Files(vec![path.clone()])], cx);
                                    },
                                ))
                            }))
                    })),
            );
        }
        panel
    }

    pub(super) fn cover_progress(&self, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(label(
                &self.themes.active().palette,
                self.tr(Text::LibraryDownloading),
            ))
            .when_some(
                self.library.cover.as_ref().map(|task| task.cancel.clone()),
                |s, flag| {
                    s.child(self.library_button(
                        "lib-stop-cover",
                        Text::LibraryCancel,
                        false,
                        cx,
                        move |_, _, _| flag.store(true, Ordering::Relaxed),
                    ))
                },
            )
    }
}

fn scan(store: &Library, sources: Vec<Source>, cancel: &AtomicBool, tx: &mpsc::SyncSender<Update>) {
    let mut report = Report::default();
    let mut choices = vec![];
    let mut last: Option<Instant> = None;
    let mut phase = None;
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut progress = |p: &eidoryn_psx::library::ScanProgress| {
            if phase != Some(p.phase)
                || last.is_none_or(|at| at.elapsed() >= Duration::from_millis(300))
            {
                let now = [p.added, p.existing, p.pending, p.failed];
                let _ = tx.try_send(Update::Counts {
                    phase: p.phase,
                    counts: std::array::from_fn(|i| report.counts[i] + now[i]),
                    processed: report.processed + p.processed,
                    total: report.total + p.total,
                    current: p
                        .current_file
                        .as_ref()
                        .and_then(|f| f.file_name())
                        .map(|n| n.to_string_lossy().into_owned()),
                });
                last = Some(Instant::now());
                phase = Some(p.phase);
            }
        };
        let result = match source {
            Source::Files(paths) => store.scan_files(&paths, cancel, &mut progress),
            Source::Directory(path, recursive) => {
                store.scan_directory(&path, recursive, cancel, &mut progress)
            }
        };
        match result {
            Ok(ScanReport {
                processed,
                total,
                added,
                existing,
                pending,
                failed,
                errors,
                choices: more,
                ..
            }) => {
                for (i, n) in [added, existing, pending, failed].into_iter().enumerate() {
                    report.counts[i] += n;
                }
                report.processed += processed;
                report.total += total;
                report.errors.extend(errors);
                choices.extend(more);
            }
            Err(error) => {
                report.counts[3] += 1;
                report.errors.push(error);
            }
        }
    }
    report.status = Some(if cancel.load(Ordering::Relaxed) {
        Text::LibraryCancelled
    } else {
        Text::LibraryFinished
    });
    let _ = tx.send(Update::Done(report, choices));
}

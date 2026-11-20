use super::{Desktop, Entry, Text, div, label, scraping::progress_bar};
use crate::desktop::{WorkspacePage, graphics::switch, icons::icon};
use eidoryn_psx::{
    library::{Library, ScanPhase, ScanReport, TrackChoice, covers, metadata},
    systems::ps1::{media::validate_bios_path, runner::Command},
};
use gpui::{Context, Div, PathPromptOptions, prelude::*, px, rgb};
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

pub(super) enum CoverRequest {
    Missing,
    Game,
    Edition(metadata::Release),
}

pub(super) struct DirectChoice {
    cues: Vec<PathBuf>,
    fast_boot: bool,
    recent: bool,
}

pub(super) struct FolderImport {
    pub(super) directories: Vec<PathBuf>,
    images: Vec<PathBuf>,
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
        let paths = paths.paths();
        if paths.is_empty() {
            return;
        }
        if let [path] = paths
            && self.page != WorkspacePage::Library
            && !path.is_dir()
        {
            self.load_selected_disc(path.clone(), false, cx);
            return;
        }
        if self.library.scan.is_some() {
            self.notice = Some(self.tr(Text::LibraryScanning).to_owned());
            cx.notify();
            return;
        }
        self.show_page(WorkspacePage::Library, window, cx);
        self.library.details = false;
        self.library.settings = false;
        let (directories, images): (Vec<_>, _) =
            paths.iter().cloned().partition(|path| path.is_dir());
        if directories.is_empty() {
            self.scan_sources(vec![Source::Files(images)], cx);
        } else {
            self.library.pending_directory = Some(FolderImport {
                directories,
                images,
            });
            cx.notify();
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

    pub(in crate::desktop) fn saving_library_changes(&self) -> bool {
        !self.library.pending_moves.is_empty()
            || self.removal_in_progress()
            || matches!(
                self.library.cache_clear,
                super::settings::CacheClearState::Running
            )
    }

    pub(super) fn reload_after_library_change(&mut self, cx: &mut Context<Self>) {
        self.library.entries_revision = self.library.entries_revision.wrapping_add(1);
        self.reload_library(cx);
    }

    pub(super) fn clear_library_metadata_cache(
        &mut self,
        scope: metadata::CacheScope,
        cx: &mut Context<Self>,
    ) {
        use super::settings::CacheClearState;
        let Some(store) = self.library.store.clone() else {
            return;
        };
        if matches!(self.library.cache_clear, CacheClearState::Running) {
            return;
        }
        if scope == metadata::CacheScope::All {
            for lookup in self.library.metadata_lookups.values() {
                lookup.cancel();
            }
            if let Some(cover) = &self.library.cover {
                cover.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.library.cache_clear = CacheClearState::Running;
        cx.notify();
        let previous = self.library.scraping.queue_write.take();
        self.library.scraping.queue_write = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_executor()
                .spawn(async move { store.clear_metadata_cache(scope) })
                .await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(()) => {
                        v.library.cache_clear = CacheClearState::Finished;
                        if scope == metadata::CacheScope::All {
                            v.library.thumbnails.clear();
                            v.library.auto_attempted.clear();
                            v.library.matches.clear();
                            v.library.match_generation = v.library.match_generation.wrapping_add(1);
                            v.library.metadata_lookups.clear();
                        }
                        v.reload_after_library_change(cx);
                    }
                    Err(error) => v.library.cache_clear = CacheClearState::Failed(error),
                }
                cx.notify();
            });
        }));
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
        let entries_revision = self.library.entries_revision;
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
                if !v.finish_library_read(entries_revision, cx) {
                    return;
                }
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
                        v.reorder_library();
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

    // A read started before a committed edit may contain removed entries or obsolete metadata.
    fn finish_library_read(&mut self, entries_revision: u64, cx: &mut Context<Self>) -> bool {
        self.library.reading = false;
        if self.library.entries_revision == entries_revision {
            return true;
        }
        self.library.reload_pending = true;
        self.read_pending_games(cx);
        false
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
        let entries_revision = self.library.entries_revision;
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
                if !v.finish_library_read(entries_revision, cx) {
                    return;
                }
                match result {
                    Ok(rows) => {
                        let missing_art = rows.iter().any(|e| {
                            !e.cache_cleared && e.thumbnail.is_none() && e.serial.is_some()
                        });
                        // The bounded list is sorted. Missing rows were removed or grouped.
                        v.library
                            .entries
                            .retain(|e| ids.binary_search(&e.game.id).is_err());
                        v.library.entries.extend(rows);
                        v.reorder_library();
                        if let Some(id) = v.library.selected
                            && ids.binary_search(&id).is_ok()
                            && !v.library.entries.iter().any(|e| e.game.id == id)
                        {
                            v.library.selected = None;
                            v.library.details = false;
                            v.library.close_editor();
                        }
                        if missing_art {
                            v.auto_covers(cx);
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
                    Picker::Folder => {
                        v.library.pending_directory = Some(FolderImport {
                            directories: vec![first],
                            images: Vec::new(),
                        });
                    }
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
            || self.library.cache_clear == super::settings::CacheClearState::Running
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
                !e.cache_cleared
                    && e.thumbnail.is_none()
                    && e.serial.as_ref().is_some_and(|serial| {
                        self.library
                            .auto_attempted
                            .insert((e.game.id, serial.clone()))
                    })
            })
            .map(|e| e.game.id)
            .collect();
        self.fetch_library_covers(ids, CoverRequest::Missing, cx);
    }

    pub(super) fn fetch_library_covers(
        &mut self,
        ids: Vec<i64>,
        request: CoverRequest,
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
            chosen_edition: matches!(request, CoverRequest::Edition(_)),
        });
        let provider = self.preferences.library.cover_provider;
        let template: Arc<str> = self.preferences.library.cover_template.as_str().into();
        let request = Arc::new(request);
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
                let request = Arc::clone(&request);
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        match &*request {
                            CoverRequest::Edition(release) => covers::fetch_release(
                                &store,
                                id,
                                provider,
                                &template,
                                release,
                                &cancel_fetch,
                            ),
                            CoverRequest::Game => {
                                covers::fetch(&store, id, provider, &template, &cancel_fetch)
                            }
                            CoverRequest::Missing => covers::fetch_missing(
                                &store,
                                id,
                                provider,
                                &template,
                                &cancel_fetch,
                            ),
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
                v.auto_covers(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn task_banner(
        &self,
        symbol: &'static str,
        title: Text,
        description: String,
    ) -> Div {
        let p = self.library_palette();
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .px_4()
            .py_3()
            .bg(rgb(p.success))
            .border_1()
            .border_color(rgb(p.success_border()))
            .rounded(px(9.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(34.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(rgb(p.tonal))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(symbol, p.on_success)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font(self.fonts.semibold())
                                    .child(self.tr(title)),
                            )
                            .child(label(&p, description).text_size(px(12.))),
                    ),
            )
    }

    pub(super) fn library_scan_status(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let mut panel = div().flex().flex_col().gap_2();
        if let Some(cancel) = self.library.scan.clone() {
            let scan = &self.library.report;
            let mut detail = self.scan_counts();
            if let Some(current) = &scan.current {
                detail.push_str(" · ");
                detail.push_str(current);
            }
            let row = self
                .task_banner(
                    "folder",
                    scan.status.unwrap_or(Text::LibraryScanning),
                    detail,
                )
                .when(scan.total > 0, |row| {
                    row.child(progress_bar(&p, scan.processed as f32 / scan.total as f32))
                })
                .child(self.library_button(
                    "lib-cancel-scan",
                    Text::LibraryCancel,
                    false,
                    cx,
                    move |_, _, _| cancel.store(true, Ordering::Relaxed),
                ));
            panel = panel.child(row);
        }
        if self.library.cover.is_some() {
            panel = panel.child(self.cover_progress(cx));
        }
        if self.library.scraping.busy()
            || (self.library.scan.is_none() && self.library.cover.is_none())
        {
            panel = panel.child(self.scrape_status(cx));
        }
        panel
    }

    pub(in crate::desktop) fn close_task_dialog(&mut self) -> bool {
        if self.library.pending_directory.take().is_some() || self.library.direct.take().is_some() {
            return true;
        }
        if !self.library.choices.is_empty() && self.library.scan.is_none() {
            self.library.choices.remove(0);
            return true;
        }
        std::mem::take(&mut self.library.task_history_open)
    }

    pub(in crate::desktop) fn task_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        if let Some(import) = &self.library.pending_directory {
            return Some(self.folder_dialog(import, cx));
        }
        self.cue_dialog(cx).or_else(|| {
            self.library
                .task_history_open
                .then(|| self.task_history_dialog(cx))
        })
    }

    fn folder_dialog(&self, import: &FolderImport, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let list = div()
            .id("lib-import-paths")
            .max_h(px(175.))
            .overflow_y_scroll()
            .children(import.directories.iter().map(|dir| {
                div()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("folder", p.muted))
                    .child(label(&p, dir.display().to_string()))
            }))
            .children(import.images.iter().map(|image| {
                div()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("file", p.muted))
                    .child(label(&p, image.display().to_string()))
            }));
        let action = div()
            .flex()
            .justify_end()
            .gap_2()
            .child(self.library_button(
                "lib-cancel-folder",
                Text::LibraryCancel,
                false,
                cx,
                |v, _, cx| {
                    v.library.pending_directory = None;
                    cx.notify();
                },
            ))
            .child(self.library_button(
                "lib-confirm-folder",
                Text::LibraryScan,
                true,
                cx,
                |v, _, cx| {
                    if v.library.scan.is_none()
                        && let Some(import) = v.library.pending_directory.take()
                    {
                        let mut sources = import
                            .directories
                            .into_iter()
                            .map(|dir| Source::Directory(dir, v.library.recursive))
                            .collect::<Vec<_>>();
                        if !import.images.is_empty() {
                            sources.push(Source::Files(import.images));
                        }
                        v.scan_sources(sources, cx);
                    }
                },
            ));
        let recursive = switch(
            &self.fonts,
            &p,
            "lib-recursive",
            self.tr(Text::LibraryRecursive),
            self.tr(Text::LibraryScanSubfoldersHint),
            self.library.recursive,
            true,
        )
        .role(gpui::Role::Switch)
        .aria_label(self.tr(Text::LibraryRecursive))
        .aria_toggled(self.library.recursive.into())
        .debug_selector(|| "lib-recursive".into())
        .on_click(cx.listener(|v, _, _, cx| {
            v.library.recursive = !v.library.recursive;
            cx.notify();
        }));
        self.library_modal(
            self.tr(Text::LibraryAddFolder),
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(list)
                .child(recursive)
                .child(action),
            cx,
        )
    }

    fn cue_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let p = self.library_palette();
        let direct = self.library.direct.as_ref();
        let choice = self
            .library
            .choices
            .first()
            .filter(|_| self.library.scan.is_none());
        if let Some(cues) = direct.map(|c| &c.cues).or_else(|| choice.map(|c| &c.cues)) {
            let direct = direct.map(|c| (c.fast_boot, c.recent));
            let mut body = div().flex().flex_col().gap_3();
            if direct.is_none()
                && let Some(choice) = choice
            {
                body = body.child(label(&p, choice.track.display().to_string()));
            }
            body = body
                .child(
                    div()
                        .id("lib-cue-options")
                        .max_h(px(224.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(cues.iter().enumerate().map(|(i, path)| {
                            let path = path.clone();
                            super::button(
                                &self.fonts,
                                &p,
                                ("lib-open-cue", i),
                                path.display().to_string(),
                                false,
                            )
                            .role(gpui::Role::Button)
                            .debug_selector(move || format!("lib-open-cue-{i}"))
                            .on_click(cx.listener(
                                move |v, _, _, cx| match direct {
                                    Some((fast, recent)) => {
                                        v.library.direct = None;
                                        v.direct_disc(path.clone(), fast, recent, cx);
                                    }
                                    None => {
                                        if v.library.scan.is_none() {
                                            v.library.choices.remove(0);
                                            v.scan_sources(
                                                vec![Source::Files(vec![path.clone()])],
                                                cx,
                                            );
                                        }
                                    }
                                },
                            ))
                        })),
                )
                .child(div().flex().justify_end().child(self.library_button(
                    "lib-cancel-track",
                    Text::LibraryCancel,
                    false,
                    cx,
                    |v, _, cx| {
                        v.close_task_dialog();
                        cx.notify();
                    },
                )));
            return Some(self.library_modal(self.tr(Text::LibraryTrackChoice), body, cx));
        }
        None
    }

    fn scan_counts(&self) -> String {
        let scan = &self.library.report;
        let mut counts = vec![format!("{}/{}", scan.processed, scan.total)];
        counts.extend(
            [
                Text::LibraryAdded,
                Text::LibraryExisting,
                Text::LibraryPending,
                Text::LibraryFailed,
            ]
            .into_iter()
            .zip(scan.counts)
            .map(|(key, n)| format!("{} {n}", self.tr(key))),
        );
        counts.join(" · ")
    }

    pub(super) fn scan_report_panel(&self) -> Div {
        let mut panel = div().flex().flex_col().gap_1();
        let p = self.library_palette();
        if let Some(status) = self.library.report.status {
            panel = panel.child(label(
                &p,
                format!("{} · {}", self.tr(status), self.scan_counts()),
            ));
        }
        panel
            .children(
                self.library
                    .report
                    .errors
                    .iter()
                    .map(|error| label(&p, error.clone()).text_color(rgb(p.on_error))),
            )
            .children(self.library.roots.iter().filter(|r| !r.available).map(|r| {
                label(
                    &p,
                    format!("{} · {}", self.tr(Text::LibraryOffline), r.path.display()),
                )
            }))
    }

    pub(super) fn cover_progress(&self, cx: &Context<Self>) -> Div {
        self.task_banner(
            "reset",
            Text::LibraryDownloading,
            self.tr(Text::LibraryCover).to_owned(),
        )
        .when_some(
            self.library.cover.as_ref().map(|task| task.cancel.clone()),
            |row, cancel| {
                row.child(self.library_button(
                    "lib-stop-cover",
                    Text::LibraryCancel,
                    false,
                    cx,
                    move |_, _, _| cancel.store(true, Ordering::Relaxed),
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

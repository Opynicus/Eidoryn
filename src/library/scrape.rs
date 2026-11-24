//! Background scraping of release metadata and regional cover art.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub use super::jobs::{
    CoverClaim, JobOutcome, JobRecord, JobStatus, cancel_games, due_games, enqueue_games,
    failed_games, recent_failures, recent_jobs,
};
use super::{
    Library, covers,
    jobs::{self, Failure, JobKind, Stage},
    metadata,
};

/// A manual refresh revalidates remote results. New imports reuse cached results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Full,
    Incremental,
}

/// Finished includes every completed entry, including skips and failures.
#[derive(Debug, Default)]
pub struct Report {
    pub finished: usize,
    pub total: usize,
    pub skipped: usize,
    pub needs_choice: usize,
    pub not_found: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub retry_waiting: usize,
    pub pending: usize,
    pub cancelled: usize,
    pub writer_batches: usize,
    pub max_batch: usize,
    pub max_in_flight: usize,
    /// Sampled transaction latencies in microseconds, bounded to 2048 batches.
    pub commit_micros: Vec<u64>,
}

/// Number of game entries that a single batch can process concurrently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "i64")]
pub struct WorkerCount(u8);

impl WorkerCount {
    pub const MAX: u8 = 20;

    pub const fn get(self) -> u8 {
        self.0
    }
}

impl From<i64> for WorkerCount {
    fn from(value: i64) -> Self {
        Self(value.clamp(1, i64::from(Self::MAX)) as u8)
    }
}

impl Default for WorkerCount {
    fn default() -> Self {
        Self(6)
    }
}

/// Settings captured when a batch starts.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub mode: Mode,
    pub provider: covers::Provider,
    pub template: &'a str,
    pub workers: WorkerCount,
    pub cover_workers: WorkerCount,
}

/// Run a batch using the default worker count.
pub fn run(
    lib: &Library,
    ids: &[i64],
    mode: Mode,
    provider: covers::Provider,
    template: &str,
    cancel: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Report {
    run_with_options(
        lib,
        ids,
        Options {
            mode,
            provider,
            template,
            workers: WorkerCount::default(),
            cover_workers: WorkerCount::from(3),
        },
        cancel,
        progress,
    )
}

/// Run a durable refresh, reporting aggregate game counts.
pub fn run_with_options(
    lib: &Library,
    ids: &[i64],
    options: Options<'_>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Report {
    run_events(lib, ids, options, cancel, |event| {
        progress(event.finished, event.total);
    })
}

/// A committed game update or the initial batch totals. Repeated IDs can be coalesced by the view.
pub struct Event {
    pub game_id: Option<i64>,
    pub started: Option<i64>,
    pub finished: usize,
    pub total: usize,
    pub failed: usize,
}

pub fn run_events(
    lib: &Library,
    ids: &[i64],
    options: Options<'_>,
    cancel: &AtomicBool,
    event: impl FnMut(Event),
) -> Report {
    run_batch(lib, ids, options, cancel, event, false)
}

/// Resume queued stages without creating a new generation or resetting the retry deadline.
pub fn resume_events(
    lib: &Library,
    ids: &[i64],
    options: Options<'_>,
    cancel: &AtomicBool,
    event: impl FnMut(Event),
) -> Report {
    run_batch(lib, ids, options, cancel, event, true)
}

fn run_batch(
    lib: &Library,
    ids: &[i64],
    options: Options<'_>,
    cancel: &AtomicBool,
    mut event: impl FnMut(Event),
    resume: bool,
) -> Report {
    let pending: BTreeSet<_> = ids.iter().copied().collect();
    let mut report = Report {
        total: pending.len(),
        ..Report::default()
    };
    event(Event {
        game_id: None,
        started: None,
        finished: 0,
        total: report.total,
        failed: 0,
    });
    let run = || -> Result<(), String> {
        if lib.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        super::transport::configure(options.workers.get(), options.cover_workers.get());
        let mut db = lib.connect()?;
        jobs::prepare_targets(&mut db, ids, options.mode, options.provider, resume)
            .map_err(|e| e.to_string())?;
        let slots = [options.workers, options.cover_workers]
            .map(|n| usize::from(n.get()).min(pending.len()));
        let (meta_tx, meta_rx) = mpsc::sync_channel(slots[Stage::Metadata as usize]);
        let (art_tx, art_rx) = mpsc::sync_channel(slots[Stage::Cover as usize]);
        let receivers = [Mutex::new(meta_rx), Mutex::new(art_rx)];
        let (output, results) = mpsc::sync_channel(slots.iter().sum());
        let metadata = metadata::Batch::default();
        let providers = lib.for_batch();
        let abort = AtomicBool::new(false);
        let mut writer = BatchWriter {
            db,
            owner: uuid::Uuid::new_v4().to_string(),
            pending,
            report: &mut report,
            event: &mut event,
            cancel,
            shutdown: &lib.shutdown,
            abort: &abort,
        };
        thread::scope(|scope| {
            for (receiver, limit) in receivers.iter().zip(slots) {
                for _ in 0..limit {
                    let output = output.clone();
                    let metadata = &metadata;
                    let providers = &providers;
                    let abort = &abort;
                    scope.spawn(move || {
                        loop {
                            let job = receiver.lock().expect("job channel lock poisoned").recv();
                            let Ok(claim) = job else {
                                break;
                            };
                            let result = work(providers, metadata, &claim, options.template, abort);
                            if output.send((claim, result)).is_err() {
                                break;
                            }
                        }
                    });
                }
            }
            drop(output);
            // Dropping both senders wakes idle workers on cancellation and writer errors.
            writer.drive(&[meta_tx, art_tx], slots, &results)
        })
    };
    if let Err(error) = run() {
        report.failed += 1;
        report.errors.push(error);
    }
    report
}

fn work(
    lib: &Library,
    metadata: &metadata::Batch,
    claim: &CoverClaim,
    template: &str,
    cancel: &AtomicBool,
) -> Work {
    if cancel.load(Ordering::Relaxed) {
        return Work::Cancelled;
    }
    match claim.kind {
        JobKind::Metadata | JobKind::GbaMetadata => {
            match metadata.prepare(lib, claim.game, claim.mode, cancel) {
                Ok(prepared) => Work::Metadata(prepared),
                Err(e) => Work::Failed(Failure {
                    code: e.code(),
                    message: "Release metadata lookup failed",
                    retry: e.retryable(),
                    deadline: e.retry_after(),
                }),
            }
        }
        JobKind::Cover(provider) => match covers::scrape_claim(
            lib, claim.game, provider, template, claim.mode, cancel, claim,
        ) {
            Ok(state) if state == "retry" || state == "failed" => {
                Work::Failed(Failure::cover(&state))
            }
            Ok(state) => Work::Cover(state),
            Err(e) => Work::Failed(Failure::cover(&e)),
        },
    }
}

struct BatchWriter<'a, F> {
    db: rusqlite::Connection,
    owner: String,
    pending: BTreeSet<i64>,
    report: &'a mut Report,
    event: &'a mut F,
    cancel: &'a AtomicBool,
    shutdown: &'a AtomicBool,
    abort: &'a AtomicBool,
}

impl<F: FnMut(Event)> BatchWriter<'_, F> {
    fn drive(
        &mut self,
        senders: &[mpsc::SyncSender<CoverClaim>; 2],
        slots: [usize; 2],
        results: &mpsc::Receiver<(CoverClaim, Work)>,
    ) -> Result<(), String> {
        let mut active = [0_usize; 2];
        let mut heartbeat = Instant::now();
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                self.abort.store(true, Ordering::Release);
                return jobs::release_owner(&self.db, &self.owner).map_err(|e| e.to_string());
            }
            if self.cancel.load(Ordering::Relaxed) {
                self.abort.store(true, Ordering::Release);
                return jobs::cancel(&self.db, &self.owner).map_err(|e| e.to_string());
            }
            if heartbeat.elapsed() >= Duration::from_secs(30) {
                jobs::renew(&self.db, &self.owner).map_err(|e| e.to_string())?;
                heartbeat = Instant::now();
            }
            for stage in [Stage::Metadata, Stage::Cover] {
                let i = stage as usize;
                let claims = jobs::claim(&mut self.db, &self.owner, stage, slots[i] - active[i])
                    .map_err(|e| e.to_string())?;
                if let Some(first) = claims.first() {
                    (self.event)(Event {
                        game_id: None,
                        started: Some(first.game),
                        finished: self.report.finished,
                        total: self.report.total,
                        failed: self.report.failed,
                    });
                }
                for claim in claims {
                    senders[i].send(claim).map_err(|e| e.to_string())?;
                    active[i] += 1;
                }
            }
            let in_flight = active.iter().sum();
            self.report.max_in_flight = self.report.max_in_flight.max(in_flight);
            if in_flight == 0 {
                break;
            }
            let first = match results.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(e) => return Err(e.to_string()),
            };
            let mut batch = vec![first];
            let until = Instant::now() + Duration::from_micros(300);
            while batch.len() < 32 && batch.len() < in_flight {
                let Ok(result) =
                    results.recv_timeout(until.saturating_duration_since(Instant::now()))
                else {
                    break;
                };
                batch.push(result);
            }
            if !self.cancel.load(Ordering::Relaxed) && !self.shutdown.load(Ordering::Acquire) {
                self.apply(batch, &mut active)?;
            }
        }
        for id in std::mem::take(&mut self.pending) {
            record(
                self.report,
                jobs::state(&self.db, id).map_err(|e| e.to_string())?,
            );
            self.notify(id);
        }
        Ok(())
    }

    fn notify(&mut self, id: i64) {
        (self.event)(Event {
            game_id: Some(id),
            started: None,
            finished: self.report.finished,
            total: self.report.total,
            failed: self.report.failed,
        });
    }

    fn apply(
        &mut self,
        batch: Vec<(CoverClaim, Work)>,
        active: &mut [usize; 2],
    ) -> Result<(), String> {
        self.report.max_batch = self.report.max_batch.max(batch.len());
        let started = Instant::now();
        let mut changed = BTreeSet::new();
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        for (claim, result) in batch {
            active[claim.kind.stage() as usize] -= 1;
            changed.insert(claim.game);
            tx.execute_batch("SAVEPOINT publish_job")
                .map_err(|e| e.to_string())?;
            if let Err(error) = commit_result(&tx, &claim, result) {
                tx.execute_batch("ROLLBACK TO publish_job")
                    .map_err(|e| e.to_string())?;
                let failure = Failure {
                    code: "commit",
                    message: "Could not commit current job result",
                    retry: true,
                    deadline: None,
                };
                jobs::fail(&tx, &claim, &failure).map_err(|e| format!("{error}: {e}"))?;
            }
            tx.execute_batch("RELEASE publish_job")
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.report.writer_batches += 1;
        if self.report.commit_micros.len() < 2048 {
            self.report
                .commit_micros
                .push(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
        }
        for id in changed {
            let state = jobs::state(&self.db, id).map_err(|e| e.to_string())?;
            if state.outcome != jobs::GameState::Pending && self.pending.remove(&id) {
                record(self.report, state);
            }
            self.notify(id);
        }
        Ok(())
    }
}

fn record(report: &mut Report, state: jobs::State) {
    match state.outcome {
        jobs::GameState::RetryWaiting => report.retry_waiting += 1,
        jobs::GameState::Pending => report.pending += 1,
        jobs::GameState::Cancelled => report.cancelled += 1,
        completed => {
            report.finished += 1;
            if state.error.is_some() {
                report.failed += 1;
            } else {
                match completed {
                    jobs::GameState::NeedsChoice => report.needs_choice += 1,
                    jobs::GameState::NotFound => report.not_found += 1,
                    jobs::GameState::Skipped => report.skipped += 1,
                    _ => {}
                }
            }
        }
    }
    if let Some(error) = state.error
        && report.errors.len() < 8
    {
        report.errors.push(error);
    }
}

enum Work {
    Metadata(metadata::Prepared),
    Cover(String),
    Failed(Failure),
    Cancelled,
}

fn commit_result(
    tx: &rusqlite::Transaction<'_>,
    claim: &CoverClaim,
    result: Work,
) -> Result<(), String> {
    if !claim.owns(tx).map_err(|e| e.to_string())? {
        return Ok(());
    }
    let finished = match result {
        Work::Metadata(prepared) => match prepared.commit_claim(tx, claim.game)? {
            Some(metadata::RefreshOutcome::Updated) => "updated",
            Some(metadata::RefreshOutcome::Candidates) => {
                if claim.mode == Mode::Full && claim.kind == JobKind::Metadata {
                    jobs::release_serial_covers(tx, claim.id).map_err(|e| e.to_string())?;
                }
                jobs::skip_dependent_covers(tx, claim.id).map_err(|e| e.to_string())?;
                "needs_choice"
            }
            Some(metadata::RefreshOutcome::NoSerial) | None => "skip_cover",
            Some(metadata::RefreshOutcome::NotFound) => {
                if claim.kind == JobKind::GbaMetadata {
                    jobs::skip_dependent_covers(tx, claim.id).map_err(|e| e.to_string())?;
                }
                "not_found"
            }
        },
        Work::Cover(ref outcome) => outcome,
        Work::Failed(failure) => return jobs::fail(tx, claim, &failure).map_err(|e| e.to_string()),
        Work::Cancelled => return Ok(()),
    };
    if claim.finish(tx, finished).map_err(|e| e.to_string())? {
        Ok(())
    } else {
        Err("Job lease expired before result publication".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ui-tests")]
    #[test]
    fn full_refresh_restores_serial_art_without_assigning_an_ambiguous_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let lib = Library::open(&path).unwrap();
        let mut db = lib.connect().unwrap();
        let tx = db.transaction().unwrap();
        tx.execute(
            "INSERT INTO games(id,fallback_title) VALUES(1,'Local disc')",
            [],
        )
        .unwrap();
        metadata::identify(&tx, 1, Some("SCUS-94900"), Some("USA"), "Local disc").unwrap();
        tx.commit().unwrap();
        lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
        let rows = ["Q100", "Q101"].map(|qid| {
            serde_json::json!({
                "game": {"value": format!("http://www.wikidata.org/entity/{qid}")},
                "title": {"value": "Two possible editions"}
            })
        });
        let metadata = serde_json::json!({"results":{"bindings": rows}});
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(32, 24)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let lib = lib.with_scrape_responses(
            http::Response::new(metadata.to_string().into_bytes()),
            http::Response::new(png.into_inner()),
        );
        let report = run(
            &lib,
            &[1],
            Mode::Full,
            covers::Provider::Custom,
            "https://example.invalid/${serial}.png",
            &AtomicBool::default(),
            |_, _| {},
        );
        assert_eq!((report.finished, report.failed), (1, 0));
        let reopened = Library::open(&path).unwrap();
        let rows = metadata::list_by_id(&reopened, &[1]).unwrap();
        let card = &rows[&1];
        assert!(card.pending);
        assert!(card.release_id.is_none());
        assert!(!card.cache_cleared);
        assert_eq!(card.serial.as_deref(), Some("SCUS-94900"));
        let cover = image::open(card.thumbnail.as_ref().unwrap()).unwrap();
        assert_eq!(cover.width() * 3, cover.height() * 4);
    }

    #[test]
    fn incremental_run_reuses_metadata_and_cover_cache_only_for_given_games() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(&tmp.path().join("library.db")).unwrap();
        let mut db = lib.connect().unwrap();
        let tx = db.transaction().unwrap();
        let serial = "SCUS-94900";
        for id in [1, 2, 3] {
            tx.execute(
                "INSERT INTO games(id,fallback_title) VALUES(?,'Disc')",
                [id],
            )
            .unwrap();
            metadata::identify(&tx, id, (id != 3).then_some(serial), Some("USA"), "Disc").unwrap();
        }
        tx.execute(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
             VALUES('Q1/SCUS-94900',?,'USA','Cached title','1996','','')",
            [serial],
        )
        .unwrap();
        let key = covers::provider_key(
            covers::Provider::Custom,
            "https://example.invalid/${serial}.png",
            serial,
            "",
            "",
        )
        .unwrap();
        tx.execute(
            "INSERT INTO cover_attempts(game_id,serial,url,state,retry_after,updated)
            VALUES(1,?,?,'not_found',unixepoch()+3600,unixepoch())",
            [serial, key.as_str()],
        )
        .unwrap();
        tx.commit().unwrap();
        let report = run(
            &lib,
            &[1, 1, 3],
            Mode::Incremental,
            covers::Provider::Custom,
            "https://example.invalid/${serial}.png",
            &AtomicBool::new(false),
            |_, _| {},
        );
        assert_eq!((report.finished, report.failed, report.skipped), (2, 0, 1));
        let rows = metadata::list(&lib).unwrap();
        assert_eq!(rows[&1].title.as_deref(), Some("Cached title"));
        assert_ne!(rows[&2].title.as_deref(), Some("Cached title"));
    }
}

//! Background scraping of release metadata and regional cover art.
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;

use super::{Library, covers, metadata};

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
    pub failed: usize,
    pub errors: Vec<String>,
}

enum Outcome {
    Done,
    Skipped,
    Failed(String),
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
        Self(3)
    }
}

/// Settings captured when a batch starts.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub mode: Mode,
    pub provider: covers::Provider,
    pub template: &'a str,
    pub workers: WorkerCount,
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
        },
        cancel,
        progress,
    )
}

/// Run on a background executor. Progress is delivered sequentially on the calling thread.
/// Cancelling stops new work and lets pending network requests reach their timeout.
pub fn run_with_options(
    lib: &Library,
    ids: &[i64],
    options: Options<'_>,
    cancel: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Report {
    let Options {
        mode,
        provider,
        template,
        workers,
    } = options;
    let metadata = metadata::Batch::default();
    dispatch(ids, workers, cancel, progress, |id| {
        let release = metadata.refresh(lib, id, mode, cancel);
        if matches!(
            release,
            Ok(metadata::RefreshOutcome::NoSerial | metadata::RefreshOutcome::Candidates)
        ) {
            return Outcome::Skipped;
        }
        if cancel.load(Ordering::Relaxed) {
            return Outcome::Skipped;
        }
        let cover = covers::scrape(lib, id, provider, template, mode, cancel);
        if release == Ok(metadata::RefreshOutcome::NotFound) && cover.as_deref() == Ok("not_found")
        {
            return Outcome::Skipped;
        }
        let mut errors = Vec::new();
        if let Err(error) = release {
            errors.push(error);
        }
        match cover {
            Err(error) => errors.push(error),
            Ok(state) if state == "retry" => {
                errors.push("Cover provider rate limit reached. Retry later.".into());
            }
            Ok(state) if state == "failed" => {
                errors.push("Recent cover request failed. Retry later.".into());
            }
            _ => {}
        }
        if errors.is_empty() {
            Outcome::Done
        } else {
            Outcome::Failed(errors.join("\n"))
        }
    })
}

fn dispatch(
    ids: &[i64],
    workers: WorkerCount,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
    job: impl Fn(i64) -> Outcome + Sync,
) -> Report {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut report = Report {
        total: ids.len(),
        ..Report::default()
    };
    progress(0, report.total);
    if ids.is_empty() || cancel.load(Ordering::Relaxed) {
        return report;
    }
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let (send, recv) = mpsc::channel();
        for _ in 0..usize::from(workers.get()).min(ids.len()) {
            let send = send.clone();
            let ids = &ids;
            let next = &next;
            let job = &job;
            scope.spawn(move || {
                while !cancel.load(Ordering::Relaxed) {
                    let Some(&id) = ids.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = job(id);
                    if cancel.load(Ordering::Relaxed) || send.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(send);
        for result in recv {
            match result {
                Outcome::Done => {}
                Outcome::Skipped => report.skipped += 1,
                Outcome::Failed(error) => {
                    report.failed += 1;
                    if report.errors.len() < 8 && !report.errors.contains(&error) {
                        report.errors.push(error);
                    }
                }
            }
            report.finished += 1;
            progress(report.finished, report.total);
        }
    });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex};

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
        tx.execute("INSERT INTO cover_attempts(game_id,serial,url,state,retry_after,updated)
            VALUES(1,?,'https://example.invalid/SCUS-94900.png','not_found',unixepoch()+3600,unixepoch())", [serial]).unwrap();
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

    #[test]
    fn workers_overlap_and_visit_only_distinct_requested_entries() {
        let barrier = Barrier::new(3);
        let seen = Mutex::new(Vec::new());
        let mut progress = Vec::new();
        let cancel = AtomicBool::new(false);
        let report = dispatch(
            &[5, 2, 5, 1],
            WorkerCount::default(),
            &cancel,
            |n, total| progress.push((n, total)),
            |id| {
                barrier.wait();
                seen.lock().unwrap().push(id);
                match id {
                    1 => Outcome::Skipped,
                    2 => Outcome::Failed("offline".into()),
                    _ => Outcome::Done,
                }
            },
        );
        let mut seen = seen.into_inner().unwrap();
        seen.sort_unstable();
        assert_eq!(seen, [1, 2, 5]);
        assert_eq!(progress, [(0, 3), (1, 3), (2, 3), (3, 3)]);
        assert_eq!((report.finished, report.failed, report.skipped), (3, 1, 1));
        assert_eq!(report.errors, ["offline"]);
    }

    #[test]
    fn dispatch_uses_the_configured_number_of_workers() {
        use std::{collections::HashSet, sync::Condvar, time::Duration};

        for limit in [1, 7, 20] {
            let workers = WorkerCount::from(limit);
            let expected = usize::from(workers.get());
            let ids: Vec<_> = (1..=limit * 2).collect();
            let reached = Condvar::new();
            let threads = Mutex::new(HashSet::new());
            let report = dispatch(
                &ids,
                workers,
                &AtomicBool::new(false),
                |_, _| {},
                |_| {
                    let mut threads = threads.lock().unwrap();
                    threads.insert(thread::current().id());
                    reached.notify_all();
                    let (threads, _) = reached
                        .wait_timeout_while(threads, Duration::from_secs(3), |ids| {
                            ids.len() < expected
                        })
                        .unwrap();
                    assert_eq!(threads.len(), expected, "worker limit {limit}");
                    Outcome::Done
                },
            );
            assert_eq!(report.finished, ids.len());
            assert_eq!(threads.into_inner().unwrap().len(), expected);
        }
    }

    #[test]
    fn cancellation_stops_the_queue_and_joins_in_flight_workers() {
        let cancel = AtomicBool::new(false);
        let started = AtomicUsize::new(0);
        let mut ids = Vec::from_iter(1..100);
        ids.push(1);
        let report = dispatch(
            &ids,
            WorkerCount::default(),
            &cancel,
            |_, _| {},
            |_| {
                started.fetch_add(1, Ordering::Relaxed);
                cancel.store(true, Ordering::Relaxed);
                Outcome::Done
            },
        );
        assert!(started.load(Ordering::Relaxed) <= 3);
        assert_eq!((report.finished, report.total), (0, 99));
    }
}

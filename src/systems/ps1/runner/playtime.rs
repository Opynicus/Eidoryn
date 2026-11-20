//! Desktop playtime follows the runner's forward intervals. A separate writer owns disk I/O.
use crate::library::Library;
use rusqlite::params;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod recovery;

const CHECKPOINT: Duration = Duration::from_secs(30);
// A running worker normally samples every frame. On platforms whose monotonic
// clock includes suspend, discard an unobserved gap and start a fresh interval.
const MAX_SAMPLE_GAP: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Checkpoint {
    started_at: i64,
    seconds: u64,
    last_played: i64,
}

#[derive(Default)]
struct ForwardTime {
    started_at: Option<i64>,
    elapsed: Duration,
    previous: Option<Instant>,
    committed: Duration,
}

impl ForwardTime {
    fn sample(&mut self, now: Instant, utc: i64, forward: bool) -> Option<Checkpoint> {
        let previous = self.previous.take();
        if let Some(previous) = previous {
            let delta = now.saturating_duration_since(previous);
            if delta <= MAX_SAMPLE_GAP {
                self.elapsed += delta;
            }
        }
        if forward {
            self.previous = Some(now);
        }
        if forward != previous.is_some()
            || self.elapsed.saturating_sub(self.committed) >= CHECKPOINT
        {
            self.committed = self.elapsed;
            return Some(Checkpoint {
                started_at: *self.started_at.get_or_insert(utc),
                seconds: self.elapsed.as_secs(),
                last_played: utc,
            });
        }
        None
    }
}

struct Disc {
    path: PathBuf,
    identity: String,
    fast_boot: bool,
}

enum Event {
    Disc(Uuid, Disc),
    Progress(Uuid, Checkpoint),
    Finish(Uuid, ExitReason),
    Clear(i64, mpsc::Sender<Result<(), String>>),
}

#[derive(Clone, Copy)]
enum ExitReason {
    Normal,
    Interrupted,
}

pub(super) struct Playtime {
    sender: Option<mpsc::Sender<Event>>,
    writer: Option<thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    current: Option<(Uuid, ForwardTime)>,
}

impl Playtime {
    pub(super) fn open(library: Library) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let error = Arc::new(Mutex::new(None));
        let feedback = Arc::clone(&error);
        let writer = thread::Builder::new()
            .name("eidoryn-library-writer".into())
            .spawn(move || write_sessions(&library, &receiver, &feedback))?;
        Ok(Self {
            sender: Some(sender),
            writer: Some(writer),
            error,
            current: None,
        })
    }

    fn send(&self, event: Event) {
        if self
            .sender
            .as_ref()
            .is_some_and(|tx| tx.send(event).is_err())
            && let Ok(mut error) = self.error.lock()
        {
            *error = Some("Library writer stopped; play statistics are unsaved".into());
        }
    }

    pub(super) fn select(&mut self, path: PathBuf, identity: String, fast_boot: bool) {
        self.finish();
        let id = Uuid::new_v4();
        self.send(Event::Disc(
            id,
            Disc {
                path,
                identity,
                fast_boot,
            },
        ));
        self.current = Some((id, ForwardTime::default()));
    }

    pub(super) fn sample(&mut self, forward: bool) {
        let Some((id, clock)) = &mut self.current else {
            return;
        };
        let utc = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if let Some(point) = clock.sample(Instant::now(), utc, forward) {
            let id = *id;
            self.send(Event::Progress(id, point));
        }
    }

    pub(super) fn clear(&mut self, game: i64, completed: mpsc::Sender<Result<(), String>>) {
        // Queue the last partial interval before advancing the persisted baseline.
        self.sample(false);
        self.send(Event::Clear(game, completed));
    }

    pub(super) fn finish(&mut self) {
        self.end(ExitReason::Normal);
    }

    fn end(&mut self, reason: ExitReason) {
        self.sample(false);
        if let Some((id, _)) = self.current.take() {
            self.send(Event::Finish(id, reason));
        }
    }

    pub(super) fn error(&self) -> Option<String> {
        self.error.lock().ok()?.clone()
    }
}

impl Drop for Playtime {
    fn drop(&mut self) {
        self.end(if thread::panicking() {
            ExitReason::Interrupted
        } else {
            ExitReason::Normal
        });
        drop(self.sender.take());
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(error) = self.error() {
            eprintln!("Eidoryn: {error}");
        }
    }
}

struct Pending {
    disc: Disc,
    game: Option<i64>,
    point: Option<Checkpoint>,
    finished: Option<ExitReason>,
    owner: Option<recovery::SessionLock>,
}

impl Pending {
    fn flush(&mut self, library: &Library, id: Uuid) -> Result<(), String> {
        let game = if let Some(game) = self.game {
            game
        } else {
            let game = library.associate_opened(
                &self.disc.path,
                &self.disc.identity,
                self.disc.fast_boot,
            )?;
            self.game = Some(game);
            game
        };
        if let Some(point) = self.point {
            if self.owner.is_none() {
                self.owner =
                    Some(recovery::SessionLock::acquire(library, id).map_err(|e| e.to_string())?);
            }
            save_checkpoint(&library.connect()?, id, game, point)?;
            self.point = None;
        }
        if let Some(reason) = self.finished
            && self.owner.is_some()
        {
            let reason = match reason {
                ExitReason::Normal => "normal",
                ExitReason::Interrupted => "interrupted",
            };
            library
                .connect()?
                .execute(
                    "UPDATE sessions SET ended_at=max(last_played,unixepoch()),exit_reason=?2
                 WHERE session_id=?1 AND ended_at IS NULL",
                    params![id.to_string(), reason],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn save_checkpoint(
    conn: &rusqlite::Connection,
    id: Uuid,
    game: i64,
    point: Checkpoint,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sessions(session_id,game_id,seconds,last_played,started_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(session_id) DO UPDATE SET seconds=max(seconds,excluded.seconds),
                                              last_played=max(last_played,excluded.last_played)",
        params![id.to_string(), game, point.seconds, point.last_played, point.started_at],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn write_sessions(library: &Library, rx: &mpsc::Receiver<Event>, error: &Mutex<Option<String>>) {
    let mut sessions: HashMap<Uuid, Pending> = HashMap::new();
    let mut recover = true;
    let mut retry = false;
    loop {
        let recovery_error = if recover {
            match recovery::close_interrupted(library) {
                Ok(()) => {
                    recover = false;
                    None
                }
                Err(e) => Some(format!("Play session recovery failed: {e}")),
            }
        } else {
            None
        };
        let event = if retry || recover {
            rx.recv_timeout(Duration::from_secs(5))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let shutdown = matches!(event, Err(mpsc::RecvTimeoutError::Disconnected));
        let mut clear = None;
        match event {
            Ok(Event::Disc(id, disc)) => {
                sessions.insert(
                    id,
                    Pending {
                        disc,
                        game: None,
                        point: None,
                        finished: None,
                        owner: None,
                    },
                );
            }
            Ok(Event::Progress(id, point)) => {
                if let Some(pending) = sessions.get_mut(&id) {
                    pending.point = Some(point);
                }
            }
            Ok(Event::Finish(id, reason)) => {
                if let Some(pending) = sessions.get_mut(&id) {
                    pending.finished = Some(reason);
                }
            }
            Ok(Event::Clear(game, reply)) => clear = Some((game, reply)),
            Err(_) => {}
        }
        let mut failure = recovery_error;
        sessions.retain(|id, session| match session.flush(library, *id) {
            Ok(()) => session.finished.is_none(),
            Err(e) => {
                failure = Some(format!("Play statistics could not be saved: {e}"));
                true
            }
        });
        if let Some((game, reply)) = clear {
            let result = if let Some(error) = &failure {
                Err(error.clone())
            } else {
                library.clear_statistics(game)
            };
            let _ = reply.send(result);
        }
        retry = failure.is_some();
        if let Ok(mut previous) = error.lock() {
            *previous = failure;
        }
        if shutdown {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_time_includes_idle_frames_and_excludes_pause_rewind_and_suspend() {
        let start = Instant::now();
        let mut clock = ForwardTime::default();
        let mut checkpoints = Vec::new();
        let mut observe = |second: u64, forward| {
            if let Some(point) =
                clock.sample(start + Duration::from_secs(second), second as i64, forward)
            {
                checkpoints.push(point);
            }
        };
        observe(0, false);
        for second in 0..60 {
            observe(second, true);
        }
        observe(60, false);
        observe(60, false);
        observe(75, false); // paused save/load and debug step
        for second in 80..110 {
            observe(second, true);
        } // last ten seconds without input
        observe(110, false);
        for second in 115..125 {
            observe(second, true);
        }
        observe(125, false);
        assert_eq!(checkpoints.last().unwrap().seconds, 100);
        assert!(checkpoints.iter().any(|p| p.seconds == 30));
        assert_eq!(
            checkpoints.iter().filter(|p| p.last_played == 60).count(),
            1
        );
        clock.sample(start + Duration::from_secs(130), 130, true);
        clock.sample(start + Duration::from_secs(730), 730, true); // no samples during suspend
        let stopped = clock
            .sample(start + Duration::from_secs(732), 732, false)
            .unwrap();
        assert_eq!(stopped.seconds, 102);
        assert_eq!(stopped.started_at, 0);
    }

    #[test]
    fn media_changes_and_forward_checkpoints_queue_while_library_initialization_is_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("library.sqlite");
        let mut locked = rusqlite::Connection::open(&database).unwrap();
        let lock = locked
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let library = Library::deferred(&database);
        let mut recorder = Playtime::open(library.clone()).unwrap();
        for (name, value, elapsed) in [("cli", 1, 3), ("next", 2, 5)] {
            let path = temp.path().join(format!("{name}.iso"));
            std::fs::write(&path, [value; 2048]).unwrap();
            let identity = crate::disc::DiscImage::open(&path)
                .unwrap()
                .identity()
                .to_owned();
            recorder.select(path, identity, false);
            let start = Instant::now();
            for (at, forward) in [(0, true), (elapsed, false)] {
                let (id, clock) = recorder.current.as_mut().unwrap();
                let point = clock
                    .sample(start + Duration::from_secs(at), 100 + at as i64, forward)
                    .unwrap();
                let id = *id;
                recorder.send(Event::Progress(id, point));
            }
        }
        recorder.finish();
        lock.commit().unwrap();
        drop(recorder);
        let games = library
            .games()
            .unwrap()
            .into_iter()
            .map(|game| (game.title, game.seconds))
            .collect::<Vec<_>>();
        assert_eq!(games, [("cli".into(), 3), ("next".into(), 5)]);
    }

    #[test]
    fn storage_failure_reports_unsaved_time_and_retries_the_latest_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let library = Library::open(&temp.path().join("library.sqlite")).unwrap();
        let path = temp.path().join("disc.iso");
        std::fs::write(&path, [0; 2048]).unwrap();
        let identity = crate::disc::DiscImage::open(&path)
            .unwrap()
            .identity()
            .to_owned();
        let conn = library.connect().unwrap();
        conn.execute_batch("CREATE TRIGGER unavailable BEFORE INSERT ON sessions BEGIN SELECT RAISE(FAIL,'disk unavailable'); END").unwrap();
        let writer = Playtime::open(library.clone()).unwrap();
        let id = Uuid::new_v4();
        writer.send(Event::Disc(
            id,
            Disc {
                path,
                identity,
                fast_boot: false,
            },
        ));
        writer.send(Event::Progress(
            id,
            Checkpoint {
                started_at: 170,
                seconds: 0,
                last_played: 170,
            },
        ));
        writer.send(Event::Progress(
            id,
            Checkpoint {
                started_at: 170,
                seconds: 30,
                last_played: 200,
            },
        ));
        let (reply, feedback) = mpsc::channel();
        writer.send(Event::Clear(0, reply));
        let error = feedback
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("disk unavailable"));
        conn.execute_batch("DROP TRIGGER unavailable").unwrap();
        writer.send(Event::Progress(
            id,
            Checkpoint {
                started_at: 170,
                seconds: 45,
                last_played: 215,
            },
        ));
        writer.send(Event::Finish(id, ExitReason::Normal));
        drop(writer);
        let games = library.games().unwrap();
        assert_eq!((games[0].seconds, games[0].last_played), (45, Some(215)));
        let (reason, started): (String, i64) = conn
            .query_row(
                "SELECT exit_reason,started_at FROM play_sessions WHERE session_id=?",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((reason.as_str(), started), ("normal", 170));
    }

    #[test]
    fn crash_recovery_replay_and_out_of_order_checkpoints_are_idempotent() {
        const CHILD_DB: &str = "EIDORYN_CHECKPOINT_CRASH_TEST_DATABASE";
        let id = Uuid::from_u128(1);
        let thirty = Checkpoint {
            started_at: 170,
            seconds: 30,
            last_played: 200,
        };
        if let Some(child_path) = std::env::var_os(CHILD_DB) {
            let library = Library::open(&PathBuf::from(child_path)).unwrap();
            let conn = library.connect().unwrap();
            conn.execute("INSERT INTO games(id, fallback_title) VALUES(1,'Test')", [])
                .unwrap();
            save_checkpoint(&conn, id, 1, thirty).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            save_checkpoint(
                &conn,
                id,
                1,
                Checkpoint {
                    started_at: 170,
                    seconds: 47,
                    last_played: 217,
                },
            )
            .unwrap();
            // Exit without destructors or a final commit, leaving SQLite to recover the WAL.
            std::process::exit(73);
        }
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("library.sqlite");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "systems::ps1::runner::playtime::tests::crash_recovery_replay_and_out_of_order_checkpoints_are_idempotent"])
            .env(CHILD_DB, &path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        let reopened = Library::open(&path).unwrap();
        let conn = reopened.connect().unwrap();
        let totals = || {
            conn.query_row(
                "SELECT count(*),sum(seconds),max(last_played) FROM sessions",
                [],
                |r| {
                    Ok((
                        r.get::<_, u32>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(totals(), (1, 30, 200));
        for point in [
            thirty,
            Checkpoint {
                started_at: 170,
                seconds: 42,
                last_played: 212,
            },
            thirty,
        ] {
            save_checkpoint(&conn, id, 1, point).unwrap();
        }
        assert_eq!(totals(), (1, 42, 212));
        let owner = recovery::SessionLock::acquire(&reopened, id).unwrap();
        recovery::close_interrupted(&reopened).unwrap();
        let ended = || {
            conn.query_row("SELECT ended_at,exit_reason FROM play_sessions", [], |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .unwrap()
        };
        assert_eq!(ended(), (None, None));
        drop(owner);
        recovery::close_interrupted(&reopened).unwrap();
        assert_eq!(ended(), (Some(212), Some("interrupted".into())));
        recovery::close_interrupted(&reopened).unwrap();
        assert_eq!(totals(), (1, 42, 212));
    }
}

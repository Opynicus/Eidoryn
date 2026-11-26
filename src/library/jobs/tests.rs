use super::*;
use crate::library::{metadata, scrape};
use std::sync::atomic::AtomicBool;

fn library() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    tx.execute("INSERT INTO games(id,fallback_title) VALUES(1,'Disc')", [])
        .unwrap();
    metadata::identify(&tx, 1, None, None, "Disc").unwrap();
    tx.commit().unwrap();
    (dir, lib)
}

#[test]
fn queued_job_kinds_keep_their_persisted_stage_and_provider() {
    for (stage, provider, kind) in [
        (Stage::Metadata, "wikidata", JobKind::Metadata),
        (Stage::Metadata, "libretro-gba", JobKind::GbaMetadata),
        (Stage::Metadata, "libretro-nds", JobKind::NdsMetadata),
        (Stage::Cover, "libretro", JobKind::Cover(Provider::Libretro)),
        (
            Stage::Cover,
            "screenscraper",
            JobKind::Cover(Provider::ScreenScraper),
        ),
        (Stage::Cover, "igdb", JobKind::Cover(Provider::Igdb)),
        (Stage::Cover, "custom", JobKind::Cover(Provider::Custom)),
    ] {
        assert_eq!(kind.stored_names(), (stage.name(), provider));
        assert_eq!(JobKind::from_stored(stage, provider).unwrap(), kind);
    }
}

#[test]
fn task_history_survives_reopen_and_keeps_only_recent_visible_stages() {
    let (dir, lib) = library();
    let db = lib.connect().unwrap();
    db.execute(
        "UPDATE game_metadata SET user_title='My disc' WHERE game_id=1",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO games(id,fallback_title,hidden) VALUES(2,'Removed',1)",
        [],
    )
    .unwrap();
    for index in 0..22 {
        db.execute(
            "INSERT INTO scrape_jobs(game_id,provider,job_type,mode,status,updated_at,outcome)
             VALUES(1,'libretro','cover','full','succeeded',?,'skipped')",
            [index],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO scrape_jobs(game_id,provider,job_type,mode,status)
         VALUES(2,'wikidata','metadata','incremental','failed')",
        [],
    )
    .unwrap();
    drop(db);
    drop(lib);
    let reopened = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    let rows = recent_jobs(&reopened).unwrap();
    assert_eq!(rows.len(), 20);
    assert_eq!((rows[0].updated_at, rows[19].updated_at), (21, 2));
    assert!(rows.iter().all(|row| row.title == "My disc"
        && row.cover
        && row.outcome == Some(JobOutcome::Skipped)
        && row.status == JobStatus::Succeeded));
}

#[cfg(feature = "ui-tests")]
#[test]
fn gba_import_and_manual_refresh_report_the_reason_for_a_skipped_cover_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let mut lib = Library::open(&path).unwrap();
    lib.configure_import_scraping(Some(Provider::Custom));
    let mut rom = [0u8; 192];
    for code in ["AXXJ", "BXXE"] {
        rom[0xac..0xb0].copy_from_slice(code.as_bytes());
        let image = dir.path().join(format!("{code}.gba"));
        std::fs::write(&image, rom).unwrap();
        lib.scan_files(&[image], &AtomicBool::default(), |_| {})
            .unwrap();
    }
    assert_eq!(
        due_games(&lib).unwrap(),
        [(1, Mode::Incremental), (2, Mode::Incremental)]
    );
    drop(lib);

    let dat = br#"game (
        name "Game (Japan)"
        region "Japan"
        serial "AXXJ"
    )
    game (
        name "Game (Japan) (Rev 1)"
        region "Japan"
        serial "AXXJ"
    )"#;
    let lib = Library::open(&path).unwrap().with_scrape_responses(
        http::Response::new(dat.to_vec()),
        http::Response::new(Vec::new()),
    );
    for mode in [Mode::Incremental, Mode::Full] {
        if mode == Mode::Full {
            enqueue_games(&lib, &[1, 2], mode, Provider::Custom).unwrap();
        }
        let report = scrape::resume_events(
            &lib,
            &[1, 2],
            scrape::Options {
                mode,
                provider: Provider::Custom,
                template: "",
                workers: 2.into(),
                cover_workers: 2.into(),
            },
            &AtomicBool::default(),
            |_| {},
        );
        assert_eq!(
            (report.finished, report.failed),
            (2, 0),
            "{:?}",
            report.errors
        );
        assert_eq!(
            (report.needs_choice, report.not_found, report.skipped),
            (1, 1, 0)
        );
        let records = recent_jobs(&lib).unwrap();
        let latest = &records[..4];
        assert_eq!(
            latest
                .iter()
                .filter(|j| j.outcome == Some(JobOutcome::NeedsChoice))
                .count(),
            1
        );
        assert_eq!(
            latest
                .iter()
                .filter(|j| j.outcome == Some(JobOutcome::NotFound))
                .count(),
            1
        );
        assert_eq!(
            latest
                .iter()
                .filter(|j| j.cover && j.outcome == Some(JobOutcome::Skipped))
                .count(),
            2
        );
        assert_eq!(due_games(&lib).unwrap(), []);
    }
}

#[test]
fn expired_lease_is_reclaimed_and_stale_results_cannot_commit() {
    let (_dir, lib) = library();
    let mut first = lib.connect().unwrap();
    prepare_targets(&mut first, &[1, 1], Mode::Full, Provider::Libretro, false).unwrap();
    let old = claim(&mut first, "first", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    let mut second = lib.connect().unwrap();
    prepare_targets(&mut second, &[1], Mode::Full, Provider::Libretro, true).unwrap();
    assert!(
        claim(&mut second, "next", Stage::Metadata, 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(due_games(&lib).unwrap(), []);
    first
        .execute(
            "UPDATE scrape_jobs SET lease_until=unixepoch()-1 WHERE id=?",
            [old.id],
        )
        .unwrap();
    assert_eq!(due_games(&lib).unwrap(), [(1, Mode::Full)]);
    recover(&second).unwrap();
    let current = claim(&mut second, "next", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(current.kind, JobKind::Metadata);
    assert!(!old.finish(&first, "updated").unwrap());
    assert!(current.finish(&second, "updated").unwrap());
    let cover = claim(&mut second, "next", Stage::Cover, 1)
        .unwrap()
        .pop()
        .unwrap();
    {
        let tx = second.transaction().unwrap();
        cover.finish_cover(&tx).unwrap();
        tx.rollback().unwrap();
    }
    let tx = second.transaction().unwrap();
    assert!(cover.owns(&tx).unwrap());
    tx.commit().unwrap();
    cancel(&second, "next").unwrap();
    assert!(cover.finish_cover(&second).is_err());
    assert_eq!(due_games(&lib).unwrap(), []);
}

#[test]
fn retry_deadline_and_deduplication_survive_reopen() {
    let (dir, lib) = library();
    let mut db = lib.connect().unwrap();
    prepare_targets(&mut db, &[1, 1], Mode::Full, Provider::Igdb, false).unwrap();
    let current = claim(&mut db, "worker", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    let deadline = crate::library::http_retry::seconds(SystemTime::now()) + 100;
    let tx = db.transaction().unwrap();
    fail(
        &tx,
        &current,
        &Failure {
            code: "limited",
            message: "rate limited",
            retry: true,
            deadline: Some(deadline),
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let persisted = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    assert_eq!(due_games(&persisted).unwrap(), []);
    assert_eq!(failed_games(&persisted).unwrap(), [1]);
    let state: (i64, i64) = db
        .query_row(
            "SELECT next_attempt_at,attempt_count FROM scrape_jobs WHERE id=?",
            [current.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, (deadline, 1));
    db.execute(
        "UPDATE scrape_jobs SET next_attempt_at=0 WHERE id=?",
        [current.id],
    )
    .unwrap();
    let report = scrape::resume_events(
        &persisted,
        &[1],
        scrape::Options {
            mode: Mode::Full,
            provider: Provider::Libretro,
            template: "",
            workers: 1.into(),
            cover_workers: 1.into(),
        },
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(
        (report.finished, report.skipped, report.failed),
        (1, 1, 0),
        "{:?}",
        report.errors
    );
    assert_eq!(due_games(&persisted).unwrap(), []);
    assert_eq!(failed_games(&persisted).unwrap(), [] as [i64; 0]);
}

#[test]
fn full_refresh_can_fetch_serial_art_while_release_lookup_is_waiting() {
    for provider in [
        Provider::Libretro,
        Provider::ScreenScraper,
        Provider::Custom,
        Provider::Igdb,
    ] {
        let (dir, lib) = library();
        lib.connect()
            .unwrap()
            .execute(
                "UPDATE game_metadata SET disc_serial='SLUS-00067',cache_cleared=1 WHERE game_id=1",
                [],
            )
            .unwrap();
        enqueue_games(&lib, &[1], Mode::Full, provider).unwrap();
        let mut db = lib.connect().unwrap();
        prepare_targets(&mut db, &[1], Mode::Full, provider, true).unwrap();
        let metadata = claim(&mut db, "worker", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        assert!(
            claim(&mut db, "worker", Stage::Cover, 1)
                .unwrap()
                .is_empty()
        );
        let tx = db.transaction().unwrap();
        fail(
            &tx,
            &metadata,
            &Failure {
                code: "rate_limited",
                message: "Release source unavailable",
                retry: true,
                deadline: Some(i64::MAX),
            },
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            due_games(&lib).unwrap().is_empty(),
            provider == Provider::Igdb
        );
        // Older versions left this dependency attached throughout metadata backoff.
        db.execute(
            "UPDATE scrape_jobs SET dependency=? WHERE job_type='cover'",
            [metadata.id],
        )
        .unwrap();
        drop(db);
        drop(lib);
        let reopened = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        enqueue_games(&reopened, &[1], Mode::Full, provider).unwrap();
        let mut db = reopened.connect().unwrap();
        assert_eq!(
            db.query_row(
                "SELECT next_attempt_at FROM scrape_jobs WHERE id=?",
                [metadata.id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            i64::MAX
        );
        prepare_targets(&mut db, &[1], Mode::Full, provider, true).unwrap();
        let art = claim(&mut db, "reopened", Stage::Cover, 1).unwrap();
        if provider == Provider::Igdb {
            assert!(art.is_empty(), "Title-based lookup needs release metadata");
        } else {
            assert_eq!(art.len(), 1, "{provider:?} can look up the disc by serial");
            assert_eq!(art[0].mode, Mode::Full);
            art[0].finish_cover(&db).unwrap();
        }
    }
}

#[test]
fn sqlite_job_and_field_commit_survive_process_exit_together() {
    use std::process::Command;
    const CHILD: &str = "EIDORYN_TEST_JOB_CRASH_DIR";
    if let Some(dir) = std::env::var_os(CHILD) {
        let lib = Library::open(&std::path::Path::new(&dir).join("library.sqlite3")).unwrap();
        let mut db = lib.connect().unwrap();
        prepare_targets(&mut db, &[1], Mode::Full, Provider::Libretro, true).unwrap();
        let job = claim(&mut db, "interrupted", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        let tx = db.transaction().unwrap();
        tx.execute(
            "UPDATE game_metadata SET auto_title='Published' WHERE game_id=1",
            [],
        )
        .unwrap();
        assert!(job.finish(&tx, "updated").unwrap());
        if std::env::var_os("EIDORYN_TEST_JOB_COMMIT").is_some() {
            tx.commit().unwrap();
        }
        // Skip destructors to simulate a process exit on either side of COMMIT.
        std::process::exit(75);
    }
    for committed in [false, true] {
        let (dir, lib) = library();
        enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "library::jobs::tests::sqlite_job_and_field_commit_survive_process_exit_together",
            ])
            .env(CHILD, dir.path())
            .env_remove("EIDORYN_TEST_JOB_COMMIT");
        if committed {
            child.env("EIDORYN_TEST_JOB_COMMIT", "1");
        }
        assert_eq!(child.output().unwrap().status.code(), Some(75));
        let reopened = Library::open(&dir.path().join("library.sqlite3")).unwrap();
        let db = reopened.connect().unwrap();
        let (title, status): (Option<String>, String) = db
            .query_row(
                "SELECT m.auto_title,j.status FROM game_metadata m
            JOIN scrape_jobs j ON j.game_id=m.game_id WHERE j.job_type='metadata'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        if committed {
            assert_eq!(title.as_deref(), Some("Published"));
            assert_eq!(status, "succeeded");
        } else {
            assert!(title.is_none());
            assert_eq!(status, "running");
            db.execute(
                "UPDATE scrape_jobs SET lease_until=0 WHERE status='running'",
                [],
            )
            .unwrap();
        }
        assert_eq!(due_games(&reopened).unwrap(), [(1, Mode::Full)]);
    }
}

#[test]
fn sqlx_enqueue_is_unique_under_race_and_cancelled_claim_loses_its_lease() {
    let (_dir, lib) = library();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let store = &lib;
            scope.spawn(move || enqueue_games(store, &[1, 1], Mode::Full, Provider::Igdb).unwrap());
        }
    });
    let mut db = lib.connect().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM scrape_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    prepare_targets(&mut db, &[1], Mode::Full, Provider::Igdb, true).unwrap();
    let active = claim(&mut db, "other", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    cancel_games(&lib, &[1]).unwrap();
    assert!(!active.finish(&db, "updated").unwrap());
    assert_eq!(due_games(&lib).unwrap(), []);
    enqueue_games(&lib, &[1], Mode::Full, Provider::Igdb).unwrap();
    assert_eq!(due_games(&lib).unwrap(), [(1, Mode::Full)]);
}

fn legacy_v3(path: &std::path::Path) -> Connection {
    let mut legacy = Connection::open(path).unwrap();
    legacy.pragma_update(None, "journal_mode", "WAL").unwrap();
    let tx = legacy.transaction().unwrap();
    for ddl in [
        super::super::SCHEMA,
        metadata::SCHEMA,
        include_str!("../metadata/online.sql"),
        include_str!("../covers/libretro.sql"),
    ] {
        tx.execute_batch(ddl).unwrap();
    }
    metadata::migrate_local_cache(&tx).unwrap();
    tx.execute_batch(
        "INSERT INTO games(id,fallback_title) VALUES(5,'Local');
        INSERT INTO sessions(session_id,game_id,seconds,last_played) VALUES('session',5,87,1);
        PRAGMA user_version=3;",
    )
    .unwrap();
    tx.commit().unwrap();
    legacy
}

#[test]
fn upgrade_keeps_a_restorable_pre_migration_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("user.db");
    let _legacy = legacy_v3(&path);
    // Keep a reader open so the live WAL is part of the required snapshot.
    let upgraded = Library::open(&path).unwrap();
    let backup = Connection::open(dir.path().join("user.db.v3.bak")).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        3
    );
    for db in [backup, upgraded.connect().unwrap()] {
        assert_eq!(
            db.query_row(
                "SELECT seconds FROM sessions WHERE session_id='session'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            87
        );
    }
}

#[test]
fn normal_shutdown_preserves_the_remaining_jobs_for_the_next_launch() {
    let (dir, lib) = library();
    enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
    let options = scrape::Options {
        mode: Mode::Full,
        provider: Provider::Libretro,
        template: "",
        workers: 1.into(),
        cover_workers: 1.into(),
    };
    lib.request_scrape_shutdown();
    let report = scrape::resume_events(&lib, &[1], options, &AtomicBool::new(false), |_| {});
    assert_eq!(report.finished, 0);
    let reopened = Library::open(&dir.path().join("library.sqlite3")).unwrap();
    assert_eq!(due_games(&reopened).unwrap(), [(1, Mode::Full)]);
    let report = scrape::resume_events(&reopened, &[1], options, &AtomicBool::new(false), |_| {});
    assert_eq!((report.finished, report.failed), (1, 0));
}

#[test]
fn a_full_refresh_upgrades_queued_work_and_revokes_an_in_flight_cache_lookup() {
    let (_dir, lib) = library();
    let mut db = lib.connect().unwrap();
    prepare_targets(&mut db, &[1], Mode::Incremental, Provider::Libretro, false).unwrap();
    let stale = claim(&mut db, "worker", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
    enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
    assert!(!stale.finish(&db, "updated").unwrap());
    assert_eq!(due_games(&lib).unwrap(), [(1, Mode::Full)]);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM scrape_jobs WHERE status='pending' AND mode='full'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

#[test]
fn a_manual_job_is_recovered_ahead_of_a_page_of_disabled_automatic_jobs() {
    let (_dir, lib) = library();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    for id in 2..=514 {
        tx.execute(
            "INSERT INTO games(id,fallback_title) VALUES(?,'Disc')",
            [id],
        )
        .unwrap();
        enqueue(
            &tx,
            id,
            Provider::Libretro,
            if id == 514 {
                Mode::Full
            } else {
                Mode::Incremental
            },
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let due = due_games(&lib).unwrap();
    assert_eq!(due.len(), 512);
    assert_eq!(due[0], (514, Mode::Full));
}

#[test]
fn switching_cover_source_supersedes_its_active_lease_and_dismisses_old_diagnostics() {
    for previously_failed in [false, true] {
        let (_dir, lib) = library();
        let mut db = lib.connect().unwrap();
        prepare_targets(&mut db, &[1], Mode::Full, Provider::Igdb, false).unwrap();
        let metadata = claim(&mut db, "first", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        metadata.finish(&db, "updated").unwrap();
        let old = claim(&mut db, "first", Stage::Cover, 1)
            .unwrap()
            .pop()
            .unwrap();
        if previously_failed {
            let tx = db.transaction().unwrap();
            fail(
                &tx,
                &old,
                &Failure {
                    code: "credentials",
                    message: "Missing credentials",
                    retry: false,
                    deadline: None,
                },
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(failed_games(&lib).unwrap(), [1]);
        }
        enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
        assert!(!old.finish(&db, "ready").unwrap());
        let mut newer = lib.connect().unwrap();
        prepare_targets(&mut newer, &[1], Mode::Full, Provider::Libretro, true).unwrap();
        let fresh = claim(&mut newer, "next", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        fresh.finish(&newer, "updated").unwrap();
        let cover = claim(&mut newer, "next", Stage::Cover, 1)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(cover.kind, JobKind::Cover(Provider::Libretro));
        cover.finish_cover(&newer).unwrap();
        assert_eq!(failed_games(&lib).unwrap(), [] as [i64; 0]);
        assert_eq!(recent_failures(&lib).unwrap(), []);
        let history: String = db
            .query_row("SELECT status FROM scrape_jobs WHERE id=?", [old.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            history,
            if previously_failed {
                "failed"
            } else {
                "cancelled"
            }
        );
    }
}

#[test]
fn both_durable_stages_obey_independent_worker_limits() {
    let (_dir, lib) = library();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    let ids: Vec<_> = (2..=25).collect();
    for id in &ids {
        tx.execute(
            "INSERT INTO games(id,fallback_title) VALUES(?,'Disc')",
            [id],
        )
        .unwrap();
        metadata::identify(&tx, *id, None, None, "Disc").unwrap();
    }
    tx.commit().unwrap();
    for (metadata_workers, cover_workers) in [(1, 7), (7, 20), (20, 1)] {
        let options = scrape::Options {
            mode: Mode::Incremental,
            provider: Provider::Libretro,
            template: "",
            workers: metadata_workers.into(),
            cover_workers: cover_workers.into(),
        };
        let cancel = AtomicBool::new(false);
        let report = scrape::run_events(&lib, &ids, options, &cancel, |_| {});
        assert_eq!(report.max_in_flight, metadata_workers as usize);
        assert_eq!(
            (report.finished, report.failed, report.skipped),
            (ids.len(), 0, ids.len())
        );
        // Resume a batch interrupted between its metadata and cover stages.
        enqueue_games(&lib, &ids, Mode::Incremental, Provider::Libretro).unwrap();
        db.execute("UPDATE scrape_jobs SET status='succeeded',outcome='updated' WHERE status='pending' AND job_type='metadata'",[]).unwrap();
        let report = scrape::resume_events(&lib, &ids, options, &cancel, |_| {});
        assert_eq!(report.max_in_flight, cover_workers as usize);
        // These fixture files are unidentified and each cover request fails locally.
        assert_eq!(report.failed, ids.len());
    }
}

#[test]
fn late_result_cannot_finish_or_fail_a_newer_attempt_of_the_same_batch() {
    let (_dir, lib) = library();
    let mut db = lib.connect().unwrap();
    prepare_targets(&mut db, &[1], Mode::Full, Provider::Libretro, false).unwrap();
    let old = claim(&mut db, "same-batch", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    db.execute("UPDATE scrape_jobs SET lease_until=0 WHERE id=?", [old.id])
        .unwrap();
    recover(&db).unwrap();
    let new = claim(&mut db, "same-batch", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!((old.id, old.owner.as_str()), (new.id, new.owner.as_str()));
    assert_eq!(new.attempt, old.attempt + 1);
    let tx = db.transaction().unwrap();
    assert!(!old.owns(&tx).unwrap());
    assert!(old.finish_cover(&tx).is_err());
    fail(
        &tx,
        &old,
        &Failure {
            code: "invalid",
            message: "stale response",
            retry: false,
            deadline: None,
        },
    )
    .unwrap();
    assert!(new.owns(&tx).unwrap());
    assert!(new.finish(&tx, "updated").unwrap());
    tx.commit().unwrap();
}

#[test]
fn concurrent_reupgrade_after_a_rollback_refreshes_the_default_backup() {
    use std::{fs, process::Command};
    const CHILD_PATH: &str = "EIDORYN_TEST_CONCURRENT_MIGRATION";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let library = Library::open(std::path::Path::new(&path)).unwrap();
        assert_eq!(
            library
                .connect()
                .unwrap()
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            crate::library::SCHEMA_VERSION
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("user.db");
    let latest = dir.path().join("user.db.v3.bak");
    let unrelated = b"existing file, not a database";
    fs::write(&latest, unrelated).unwrap();
    drop(legacy_v3(&path));
    let upgrade = || {
        let mut children = Vec::new();
        for _ in 0..3 {
            children.push(Command::new(std::env::current_exe().unwrap())
                .args(["--exact","library::jobs::tests::concurrent_reupgrade_after_a_rollback_refreshes_the_default_backup"])
                .env(CHILD_PATH,&path).spawn().unwrap());
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
    };
    let seconds = |file: &std::path::Path| {
        Connection::open(file)
            .unwrap()
            .query_row(
                "SELECT seconds FROM sessions WHERE session_id='session'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    upgrade();
    assert_eq!(seconds(&latest), 87);
    let rollback = dir.path().join("before-rollback");
    fs::create_dir(&rollback).unwrap();
    for file in ["user.db", "user.db-wal", "user.db-shm"] {
        let current = dir.path().join(file);
        if current.exists() {
            fs::rename(current, rollback.join(file)).unwrap();
        }
    }
    fs::copy(&latest, &path).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE sessions SET seconds=115 WHERE session_id='session'",
            [],
        )
        .unwrap();
    upgrade();
    assert_eq!(seconds(&latest), 115);
    let archives: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p != &latest && p.extension().is_some_and(|e| e == "bak"))
        .collect();
    assert_eq!(archives.len(), 2);
    let (unknown, snapshots): (Vec<_>, Vec<_>) = archives
        .into_iter()
        .partition(|file| fs::read(file).unwrap() == unrelated);
    assert_eq!(unknown.len(), 1);
    assert_eq!(seconds(&snapshots[0]), 87);
}

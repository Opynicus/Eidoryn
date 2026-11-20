use super::refresh::{cache_releases, commit_one, refresh_with};
use super::*;
use crate::library::scrape::Mode;
use std::sync::atomic::Ordering;

const TEST_SERIAL: &str = "SCUS-94900";

fn response(releases: Vec<Release>) -> SourceResponse {
    let bindings: Vec<_> = releases.iter().map(|r| serde_json::json!({
        "game": {"value": format!("http://www.wikidata.org/entity/{}", r.id.split('/').next().unwrap())},
        "title": {"value": r.title},
    })).collect();
    let raw =
        serde_json::to_vec_pretty(&serde_json::json!({"results":{"bindings":bindings}})).unwrap();
    SourceResponse {
        source: wikidata::Wikidata::SOURCE,
        releases,
        raw,
    }
}

#[test]
fn import_uses_only_local_release_data_and_names_are_suggestions() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    fixture(&lib, 1, Some(TEST_SERIAL));
    let rows = list(&lib).unwrap();
    assert!(rows[&1].release_id.is_none());
    assert!(rows[&1].source.is_none());
    assert!(!rows[&1].pending);
    assert!(candidates(&lib, 1, TEST_SERIAL).unwrap().is_empty());
    let mut release = downloaded("Q1", "Test Adventure");
    release.aliases = "Midnight|北の大地".into();
    refresh_with(&lib, 1, |_| Ok(response(vec![release]))).unwrap();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    for (id, serial, region, name) in [
        (2, None, None, "北の大地 [copy]"),
        (3, Some(TEST_SERIAL), Some("Japan"), "Conflicting region"),
        (4, None, None, "Unrelated"),
    ] {
        tx.execute(
            "INSERT INTO games(id,fallback_title) VALUES(?,?)",
            params![id, name],
        )
        .unwrap();
        identify(&tx, id, serial, region, name).unwrap();
    }
    tx.commit().unwrap();
    let rows = list(&lib).unwrap();
    for id in [2, 3] {
        assert!(rows[&id].pending);
        assert!(rows[&id].release_id.is_none());
        assert_eq!(candidates(&lib, id, "").unwrap().len(), 1);
    }
    assert!(!rows[&4].pending);
    assert!(candidates(&lib, 4, "").unwrap().is_empty());
    assert_eq!(candidates(&lib, 4, "midn").unwrap().len(), 1);
    select(&lib, 2, "Q1/SCUS-94900").unwrap();
    set_override(&lib, 2, Field::Region, Some("User region")).unwrap();
    set_override(&lib, 2, Field::Date, Some("2000")).unwrap();
    for invalid in [
        "0000",
        "2021-02-29",
        "2022-00",
        "２０２３",
        "2022-5-5",
        "199--1",
    ] {
        assert!(
            set_override(&lib, 2, Field::Date, Some(invalid)).is_err(),
            "{invalid}"
        );
    }
    refresh(&lib, 2).unwrap();
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&2].region.as_deref(), Some("User region"));
    assert_eq!(rows[&2].date.as_deref(), Some("2000"));
    set_override(&lib, 2, Field::Date, None).unwrap();
    assert_eq!(list(&lib).unwrap()[&2].date.as_deref(), Some("1996"));
}

#[test]
#[ignore = "requires six locally supplied PS1 disc images in tests/"]
fn local_ps1_image_sample() {
    use eidoryn_ps1::disc::{DiscImage, DiscRegion};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    for (id, (name, serial)) in (1..).zip([
        (
            "Castlevania - Symphony of the Night (USA).chd",
            "SLUS-00067",
        ),
        ("Castlevania - Symphony of the Night.cue", "SLUS-00067"),
        ("Crash Bandicoot (USA).cue", "SCUS-94900"),
        ("Resident Evil 2 (USA) (Disc 1).chd", "SLUS-00421"),
        ("Resident Evil 2 (USA) (Disc 2).chd", "SLUS-00592"),
        ("Tekken 3 (USA).cue", "SLUS-00402"),
    ]) {
        let probe = DiscImage::probe(&root.join(name)).unwrap();
        assert_eq!(probe.region, Some(DiscRegion::America), "{name}");
        assert_eq!(probe.serial.as_deref(), Some(serial), "{name}");
        fixture(&lib, id, probe.serial.as_deref());
        assert!(list(&lib).unwrap()[&id].release_id.is_none(), "{name}");
    }
}

#[test]
fn work_publication_is_distinct_from_the_regional_release_date() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    fixture(&lib, 1, Some(TEST_SERIAL));
    let mut release = downloaded("Q1", "Test Adventure");
    release.date.clear();
    release.work_date = "2002".into();
    refresh_with(&lib, 1, |_| Ok(response(vec![release]))).unwrap();
    let rows = list(&lib).unwrap();
    assert!(rows[&1].date.is_none());
    assert_eq!(rows[&1].work_date.as_deref(), Some("2002"));
}

fn fixture(lib: &Library, id: i64, serial: Option<&str>) {
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO games(id,fallback_title) VALUES(?,'Disc')",
        [id],
    )
    .unwrap();
    identify(&tx, id, serial, Some("USA"), "Disc").unwrap();
    tx.commit().unwrap();
}

fn downloaded(key: &str, title: &str) -> Release {
    Release {
        id: format!("{key}/SCUS-94900"),
        serial: "SCUS-94900".into(),
        title: title.into(),
        region: "USA".into(),
        date: "1996".into(),
        aliases: "Online alias".into(),
        work_date: String::new(),
    }
}

#[test]
fn online_results_survive_reopen_and_seed_new_offline_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let lib = Library::open(&path).unwrap();
    fixture(&lib, 1, Some("SCUS-94900"));
    let release = downloaded("Q719176", "Source title");
    set_override(&lib, 1, Field::Title, Some("Manual title")).unwrap();
    assert_eq!(
        refresh_with(&lib, 1, |s| {
            assert_eq!(s, "SCUS-94900");
            Ok(response(vec![release.clone()]))
        })
        .unwrap(),
        RefreshOutcome::Updated
    );
    drop(lib);
    let lib = Library::open(&path).unwrap();
    fixture(&lib, 2, Some("SCUS-94900"));
    refresh(&lib, 1).unwrap();
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&1].title.as_deref(), Some("Manual title"));
    assert_eq!(rows[&1].date.as_deref(), Some("1996"));
    assert_eq!(rows[&2].title.as_deref(), Some("Source title"));
    assert_eq!(rows[&2].source.as_deref(), Some(LIVE));
    let choices = candidates(&lib, 1, "SCUS-94900").unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].title, "Source title");
    assert_eq!(
        refresh_with(&lib, 1, |_| Ok(response(Vec::new()))).unwrap(),
        RefreshOutcome::NotFound
    );
    assert!(refresh_with(&lib, 1, |_| Err("offline".into())).is_err());
    let raw = response(vec![release.clone()]).raw;
    assert_eq!(list(&lib).unwrap()[&1].release_id, Some(release.id));
    let bytes: Vec<u8> = lib
        .connect()
        .unwrap()
        .query_row(
            "SELECT payload FROM metadata_responses WHERE serial=?",
            [TEST_SERIAL],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes, raw);
}

#[test]
fn conflicts_are_persistent_and_a_newer_manual_match_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    fixture(&lib, 1, Some("SCUS-94900"));
    fixture(&lib, 2, None);
    assert_eq!(
        refresh_with(&lib, 2, |_| panic!("No serial to request")).unwrap(),
        RefreshOutcome::NoSerial
    );
    let a = downloaded("Q1", "Edition A");
    let b = downloaded("Q2", "Edition B");
    assert_eq!(
        refresh_with(&lib, 1, |_| Ok(response(vec![a.clone(), b.clone()]))).unwrap(),
        RefreshOutcome::Candidates
    );
    assert!(list(&lib).unwrap()[&1].pending);
    assert_eq!(candidates(&lib, 1, "").unwrap().len(), 2);
    select(&lib, 1, &b.id).unwrap();
    assert!(!list(&lib).unwrap()[&1].pending);
    assert_eq!(
        refresh_with(&lib, 1, |_| Ok(response(vec![a.clone(), b.clone()]))).unwrap(),
        RefreshOutcome::Updated
    );
    assert_eq!(list(&lib).unwrap()[&1].title.as_deref(), Some("Edition B"));

    let japanese = Release {
        id: "Q20/SCPS-10031".into(),
        serial: "SCPS-10031".into(),
        region: "Japan".into(),
        ..downloaded("Q20", "Edition JP")
    };
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    cache_releases(&tx, "SCPS-10031", &[japanese], LIVE).unwrap();
    tx.commit().unwrap();
    let later = refresh_with(&lib, 1, |_| {
        select(&lib, 1, "Q20/SCPS-10031")?;
        Ok(response(vec![a.clone()]))
    });
    assert!(later.is_err());
    assert_eq!(
        list(&lib).unwrap()[&1].release_id.as_deref(),
        Some("Q20/SCPS-10031")
    );

    select(&lib, 1, &a.id).unwrap();
    let stale = refresh_with(&lib, 1, |_| {
        refresh_with(&lib, 1, |_| Ok(response(vec![downloaded("Q1", "Latest")])))?;
        Ok(response(vec![a.clone()]))
    });
    assert!(stale.is_err());
    assert_eq!(list(&lib).unwrap()[&1].title.as_deref(), Some("Latest"));
    let stored: Vec<u8> = lib
        .connect()
        .unwrap()
        .query_row(
            "SELECT payload FROM metadata_responses WHERE serial=?",
            [TEST_SERIAL],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, response(vec![downloaded("Q1", "Latest")]).raw);
}

#[test]
fn v1_upgrade_preserves_user_edits_and_session_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(crate::library::SCHEMA).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db.execute_batch(
        "PRAGMA user_version=1; INSERT INTO games(id,fallback_title) VALUES(8,'Original');
        INSERT INTO game_metadata(game_id,filename,user_title) VALUES(8,'Image','Manual');
        INSERT INTO sessions(session_id,game_id,seconds,last_played) VALUES('old',8,100,21);",
    )
    .unwrap();
    drop(db);
    let lib = Library::open(&path).unwrap();
    assert_eq!(list(&lib).unwrap()[&8].title.as_deref(), Some("Manual"));
    let db = lib.connect().unwrap();
    assert_eq!(
        db.query_row("SELECT seconds FROM sessions WHERE game_id=8", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        100
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
}

#[test]
fn shared_serial_is_fetched_once_and_applied_to_each_game_independently() {
    use std::{sync::Barrier, sync::atomic::AtomicUsize, thread};

    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    let serial = "SCUS-94900";
    fixture(&lib, 1, Some(serial));
    fixture(&lib, 2, Some(serial));
    set_override(&lib, 2, Field::Title, Some("My copy")).unwrap();
    let scope = Batch::default();
    let start = Barrier::new(2);
    let count = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    thread::scope(|threads| {
        for id in [1, 2] {
            let (start, scope, cancel, lib, count) = (&start, &scope, &cancel, &lib, &count);
            threads.spawn(move || {
                start.wait();
                assert_eq!(
                    scope
                        .refresh_with(lib, id, Mode::Full, cancel, |key| {
                            count.fetch_add(1, Ordering::Relaxed);
                            assert_eq!(key, serial);
                            Ok(response(vec![downloaded("Q100", "Shared lookup")]))
                        })
                        .unwrap(),
                    RefreshOutcome::Updated
                );
            });
        }
    });
    assert_eq!(count.load(Ordering::Relaxed), 1);
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&1].title.as_deref(), Some("Shared lookup"));
    assert_eq!(rows[&2].title.as_deref(), Some("My copy"));
    let cached = Batch::default();
    assert_eq!(
        cached
            .refresh_with(&lib, 1, Mode::Incremental, &cancel, |_| panic!(
                "incremental cache miss"
            ))
            .unwrap(),
        RefreshOutcome::Updated
    );
    assert_eq!(
        Batch::default()
            .refresh_with(&lib, 1, Mode::Full, &cancel, |_| Ok(response(vec![
                downloaded("Q100", "Refreshed title")
            ])))
            .unwrap(),
        RefreshOutcome::Updated
    );
    assert_eq!(
        list(&lib).unwrap()[&1].title.as_deref(),
        Some("Refreshed title")
    );
}

#[test]
fn manual_serial_search_populates_candidates_before_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    fixture(&lib, 1, None);
    for query in [
        "",
        "My local image",
        "/media/SLUS-00000.iso",
        "SCUS-9",
        "FAKE-12345",
    ] {
        assert!(
            search_with(&lib, 1, query, |_| panic!("non-serial query sent upstream"))
                .unwrap()
                .is_empty()
        );
    }
    let choices = search_with(&lib, 1, " scus-94900 ", |s| {
        assert_eq!(s, TEST_SERIAL);
        Ok(response(vec![downloaded("Q1", "Test Adventure")]))
    })
    .unwrap();
    assert_eq!(choices.len(), 1);
    assert!(list(&lib).unwrap()[&1].release_id.is_none());
    for query in ["scus-94900", "Adventure"] {
        let cached = search_with(&lib, 1, query, |_| panic!("unexpected network access")).unwrap();
        assert_eq!(cached[0].id, choices[0].id);
    }
    select(&lib, 1, &choices[0].id).unwrap();
    assert_eq!(
        list(&lib).unwrap()[&1].title.as_deref(),
        Some("Test Adventure")
    );
}

#[test]
fn old_local_matches_keep_provenance_and_overrides_until_fetched() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(crate::library::SCHEMA).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db.execute_batch(include_str!("online.sql")).unwrap();
    db.execute_batch(include_str!("../covers/libretro.sql"))
        .unwrap();
    db.execute_batch(
        "PRAGMA user_version=2;
         INSERT INTO games(id,fallback_title) VALUES(1,'Local'),(2,'Unmatched');
         INSERT INTO game_metadata(game_id,filename,release_id,release_serial,auto_region,
             auto_title,auto_date,source,user_title,confirmed) VALUES
             (1,'Local','Q1/SCUS-94900','SCUS-94900','USA','Saved title','2001','Old snapshot','Personal title',1);
         INSERT INTO game_metadata(game_id,filename,pending) VALUES(2,'Unknown',1);",
    ).unwrap();
    drop(db);
    let lib = Library::open(&path).unwrap();
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&1].source.as_deref(), Some("Old snapshot"));
    assert_eq!(rows[&1].title.as_deref(), Some("Personal title"));
    assert!(!rows[&2].pending);
    assert_eq!(candidates(&lib, 1, "Saved").unwrap().len(), 1);
    fixture(&lib, 3, Some(TEST_SERIAL));
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&3].title.as_deref(), Some("Saved title"));
    assert_eq!(rows[&3].source.as_deref(), Some("Old snapshot"));
    let updated = Batch::default()
        .refresh_with(&lib, 1, Mode::Incremental, &AtomicBool::new(false), |_| {
            Ok(response(vec![downloaded("Q1", "Online title")]))
        })
        .unwrap();
    assert_eq!(updated, RefreshOutcome::Updated);
    let rows = list(&lib).unwrap();
    assert_eq!(rows[&1].title.as_deref(), Some("Personal title"));
    assert_eq!(rows[&1].source.as_deref(), Some(LIVE));
}

#[test]
fn prepared_results_and_source_provenance_roll_back_with_the_job() {
    let temp = tempfile::tempdir().unwrap();
    let lib = Library::open(&temp.path().join("index.db")).unwrap();
    fixture(&lib, 1, Some(TEST_SERIAL));
    let cancel = AtomicBool::new(false);
    let batch = Batch::default();
    let prepare = || {
        batch
            .prepare_with(&lib, 1, Mode::Full, &cancel, |_| {
                Ok(response(vec![downloaded("Q202", "Example:   ADVENTURE!")]))
            })
            .unwrap()
    };
    let mut db = lib.connect().unwrap();
    let initial = prepare();
    let tx = db.transaction().unwrap();
    assert_eq!(initial.commit(&tx, 1).unwrap(), RefreshOutcome::Updated);
    let source: String = tx
        .query_row(
            "SELECT external_id FROM metadata_sources WHERE game_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(source, "Q202/SCUS-94900");
    tx.rollback().unwrap();
    assert!(list(&lib).unwrap()[&1].source.is_none());
    assert!(
        !db.prepare("SELECT 1 FROM metadata_responses")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert!(!has_title_candidate(&db, "example adventure").unwrap());

    // The failed writer dropped its body. A later job must refetch rather than
    // mark an empty cache successful.
    let retried = prepare();
    commit_one(&lib, 1, retried).unwrap();
    assert!(has_title_candidate(&db, &title_hint("  Example–Adventure (Disc 2) [copy]")).unwrap());
    assert_eq!(candidates(&lib, 1, "EXAMPLE ADVENTURE").unwrap().len(), 1);
    assert_eq!(
        title_hint("A (Different) Title [backup] (Japan)"),
        "a different title"
    );
    select(&lib, 1, "Q202/SCUS-94900").unwrap();
    fixture(&lib, 2, None);
    assert_eq!(candidates(&lib, 2, "Example adventurre").unwrap().len(), 1);
    assert!(list(&lib).unwrap()[&2].release_id.is_none());
    assert!(
        candidates(&lib, 2, "Entirely Different Title")
            .unwrap()
            .is_empty()
    );
    let priority: i64 = db
        .query_row(
            "SELECT priority FROM metadata_sources WHERE game_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(priority, 2);
}

#[test]
fn same_batch_retries_transient_source_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(&tmp.path().join("index.db")).unwrap();
    fixture(&lib, 1, Some(TEST_SERIAL));
    let batch = Batch::default();
    let cancel = AtomicBool::new(false);
    let offline = batch.prepare_with(&lib, 1, Mode::Full, &cancel, |_| {
        Err(ProviderError::Transport)
    });
    assert!(matches!(offline, Err(ProviderError::Transport)));
    let retry = batch
        .prepare_with(&lib, 1, Mode::Full, &cancel, |_| {
            Ok(response(vec![downloaded("Q3", "New result")]))
        })
        .unwrap();
    assert_eq!(commit_one(&lib, 1, retry).unwrap(), RefreshOutcome::Updated);
}

#[test]
fn refreshing_one_copy_preserves_another_editions_exact_source_bytes_until_reselected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let lib = Library::open(&path).unwrap();
    fixture(&lib, 1, Some(TEST_SERIAL));
    let old = downloaded("Q1", "Earlier edition");
    let first = response(vec![old.clone()]);
    let first_raw = first.raw.clone();
    refresh_with(&lib, 1, |_| Ok(first)).unwrap();
    fixture(&lib, 2, Some(TEST_SERIAL));
    select(&lib, 2, &old.id).unwrap();
    lib.connect()
        .unwrap()
        .execute(
            "UPDATE metadata_responses SET fetched_at=11 WHERE serial=?",
            [TEST_SERIAL],
        )
        .unwrap();
    let new = downloaded("Q2", "Current edition");
    let second = response(vec![new.clone()]);
    let second_raw = second.raw.clone();
    refresh_with(&lib, 1, |_| Ok(second)).unwrap();
    drop(lib);

    let lib = Library::open(&path).unwrap();
    let db = lib.connect().unwrap();
    let evidence_for = |id| {
        db.query_row(
            "SELECT external_id,raw_payload,fetched_at FROM metadata_sources WHERE game_id=?",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap()
    };
    assert_eq!(evidence_for(2), (old.id, first_raw, 11));
    let (_, current_raw, stamp) = evidence_for(1);
    assert_eq!(current_raw, second_raw);
    assert!(stamp > 11);
    select(&lib, 2, &new.id).unwrap();
    assert_eq!(evidence_for(2), (new.id, second_raw, stamp));
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM referenced_metadata_responses",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "Last historical reference released");
}

#[test]
fn upgrade_links_only_identifiable_v3_responses() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.db");
    let db = Connection::open(&path).unwrap();
    for sql in [
        crate::library::SCHEMA,
        SCHEMA,
        include_str!("online.sql"),
        include_str!("local_cache.sql"),
        include_str!("../covers/libretro.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute_batch("PRAGMA user_version=3;
        INSERT INTO games(id,fallback_title) VALUES(1,'A'),(2,'B');
        INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date) VALUES('Q1/SCUS-94900','SCUS-94900','USA','Current','1996','','');
        INSERT INTO metadata_responses(serial,payload,fetched_at) VALUES('SCUS-94900',x'AA',10);
        INSERT INTO game_metadata(game_id,filename,release_id,release_serial,auto_title,auto_region,auto_date,refreshed,source)
        VALUES (1,'A','Q1/SCUS-94900','SCUS-94900','Current','USA','1996',15,'Wikidata (online)'),
               (2,'B','Q1/SCUS-94900','SCUS-94900','Older','USA','1995',7,'Wikidata (online)');").unwrap();
    drop(db);
    let lib = Library::open(&path).unwrap();
    let db = lib.connect().unwrap();
    let source = |id| {
        db.query_row(
            "SELECT raw_payload,fetched_at FROM metadata_sources WHERE game_id=?",
            [id],
            |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    assert_eq!(source(1), (Some(vec![0xAA]), 10));
    assert_eq!(source(2), (None, 7));
    assert_eq!(list(&lib).unwrap()[&2].title.as_deref(), Some("Older"));
}

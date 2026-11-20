use super::*;

#[test]
fn snapshot_fields_and_acceptance_sample() {
    let mut ids = std::collections::HashSet::new();
    for r in CATALOG.iter() {
        assert!(ids.insert(&r.id), "duplicate edition {}", r.id);
        assert_eq!(r.serial.len(), 10);
        assert_ne!(r.title, "");
        assert!(r.work_date.is_empty() || valid_date(&r.work_date));
        assert!(
            r.date.is_empty() || valid_date(&r.date),
            "bad date for {}",
            r.id
        );
    }
    for (serial, region, title, date) in [
        ("SCUS-94900", "USA", "Crash Bandicoot", "1996-09-09"),
        (
            "SCPS-10031",
            "Japan",
            "クラッシュ・バンディクー",
            "1996-12-06",
        ),
        ("SLPS-00001", "Japan", "リッジレーサー", "1994-12-03"),
        ("SCUS-94300", "USA", "Ridge Racer", "1995-09-09"),
        ("SLES-00943", "Europe", "Lucky Luke", "1998-04"),
        ("SLPS-01776", "Japan", "スーパーヒーロー作戦", "1999"),
    ] {
        let r = matching_serial(&CATALOG, Some(serial), Some(region)).unwrap();
        assert_eq!(
            (r.title.as_str(), r.date.as_str()),
            (title, date),
            "{serial}"
        );
    }
    assert!(matching_serial(&CATALOG, Some("SCUS-94900"), Some("Japan")).is_none());
    assert!(CATALOG.iter().any(|r| r.suggested(Some("SCUS-94900"), "")));
    assert!(matching_serial(&CATALOG, None, None).is_none());
    assert!(
        CATALOG
            .iter()
            .any(|r| r.suggested(None, &title_hint("Crash Bandicoot (Japan)")))
    );
    assert!(
        matching_serial(&CATALOG, Some("SLUS-00402"), Some("USA")).is_none(),
        "Tekken 3 should exercise a real catalog miss"
    );
    assert!(
        !CATALOG
            .iter()
            .any(|r| r.suggested(None, "my homebrew game"))
    );
    for invalid in [
        "0000",
        "2021-02-29",
        "2022-00",
        "２０２３",
        "2022-5-5",
        "199--1",
    ] {
        assert!(!valid_date(invalid), "{invalid}");
    }
}

#[test]
fn field_edits_and_confirmed_matches_survive_reimport_refresh_and_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.sqlite3");
    let lib = Library::open(&path).unwrap();
    let mut conn = lib.connect().unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute(
        "INSERT INTO games(id,fallback_title) VALUES(1,'Test'),(2,'Crash Bandicoot')",
        [],
    )
    .unwrap();
    identify(&tx, 1, Some("SCUS-94900"), Some("USA"), "Test").unwrap();
    identify(&tx, 2, None, None, "Crash Bandicoot").unwrap();
    tx.commit().unwrap();
    assert!(needs_match(&conn, 2).unwrap());
    assert!(candidates(&lib, 2, "").unwrap().len() >= 2);
    set_override(&lib, 1, Field::Title, Some("Preferred")).unwrap();
    set_override(&lib, 1, Field::Region, Some("My region")).unwrap();
    set_override(&lib, 1, Field::Date, Some("2000")).unwrap();
    let tx = conn.transaction().unwrap();
    identify(&tx, 1, Some("SCPS-10031"), Some("Japan"), "Moved").unwrap();
    tx.commit().unwrap();
    select(&lib, 1, "Q719176/SCPS-10031").unwrap();
    let reopened = Library::open(&path).unwrap();
    refresh(&reopened, 1).unwrap();
    let m = &list(&reopened).unwrap()[&1];
    assert_eq!(m.title.as_deref(), Some("Preferred"));
    assert_eq!(m.region.as_deref(), Some("My region"));
    assert_eq!(m.date.as_deref(), Some("2000"));
    assert!(m.aliases.iter().any(|a| a == "Crash Bandicoot"));
    assert_eq!(m.release_id.as_deref(), Some("Q719176/SCPS-10031"));
    assert_eq!(m.source.as_deref(), Some(SNAPSHOT));
    assert!(m.refreshed.is_some());
    set_override(&reopened, 1, Field::Date, None).unwrap();
    assert_eq!(
        list(&reopened).unwrap()[&1].date.as_deref(),
        Some("1996-12-06")
    );
}

#[test]
#[ignore = "requires six locally supplied PS1 disc images in tests/"]
fn local_ps1_image_sample() {
    use eidoryn_ps1::disc::{DiscImage, DiscRegion};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let paths = [
        "Castlevania - Symphony of the Night (USA).chd",
        "Castlevania - Symphony of the Night.cue",
        "Crash Bandicoot (USA).cue",
        "Resident Evil 2 (USA) (Disc 1).chd",
        "Resident Evil 2 (USA) (Disc 2).chd",
        "Tekken 3 (USA).cue",
    ];
    for name in paths {
        let probe = DiscImage::probe(&root.join(name)).unwrap();
        assert_eq!(probe.region, Some(DiscRegion::America), "{name}");
        let serial = probe.serial.as_deref().unwrap_or("unknown");
        let found = matching_serial(&CATALOG, Some(serial), Some("USA"));
        if name.starts_with("Crash") {
            assert_eq!(found.unwrap().title, "Crash Bandicoot");
        }
        println!(
            "{name} | {serial} | {}",
            found.map_or("no match", |r| r.title.as_str())
        );
    }
}

#[test]
fn work_publication_is_distinct_from_the_regional_release_date() {
    let release = matching_serial(&CATALOG, Some("SLUS-01524"), Some("USA")).unwrap();
    assert_eq!(release.date, "");
    assert_eq!(release.work_date, "2002");
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
            Ok(vec![release.clone()])
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
        refresh_with(&lib, 1, |_| Ok(Vec::new())).unwrap(),
        RefreshOutcome::NotFound
    );
    assert!(refresh_with(&lib, 1, |_| Err("offline".into())).is_err());
    assert_eq!(list(&lib).unwrap()[&1].release_id, Some(release.id));
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
        refresh_with(&lib, 1, |_| Ok(vec![a.clone(), b.clone()])).unwrap(),
        RefreshOutcome::Candidates
    );
    assert!(list(&lib).unwrap()[&1].pending);
    assert_eq!(candidates(&lib, 1, "").unwrap().len(), 2);
    select(&lib, 1, &b.id).unwrap();
    assert!(!list(&lib).unwrap()[&1].pending);
    assert_eq!(
        refresh_with(&lib, 1, |_| Ok(vec![a.clone(), b.clone()])).unwrap(),
        RefreshOutcome::Updated
    );
    assert_eq!(list(&lib).unwrap()[&1].title.as_deref(), Some("Edition B"));

    let later = refresh_with(&lib, 1, |_| {
        select(&lib, 1, "Q719176/SCPS-10031")?;
        Ok(vec![a.clone()])
    });
    assert!(later.is_err());
    assert_eq!(
        list(&lib).unwrap()[&1].release_id.as_deref(),
        Some("Q719176/SCPS-10031")
    );

    select(&lib, 1, &a.id).unwrap();
    let stale = refresh_with(&lib, 1, |_| {
        refresh_with(&lib, 1, |_| Ok(vec![downloaded("Q1", "Latest")]))?;
        Ok(vec![a.clone()])
    });
    assert!(stale.is_err());
    assert_eq!(list(&lib).unwrap()[&1].title.as_deref(), Some("Latest"));
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
        2
    );
}

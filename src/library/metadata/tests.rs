use super::*;

#[test]
fn snapshot_fields_and_acceptance_sample() {
    let mut ids = std::collections::HashSet::new();
    for r in CATALOG.iter() {
        assert!(ids.insert(r.id), "duplicate edition {}", r.id);
        assert_eq!(r.serial.len(), 10);
        assert_ne!(r.title, "");
        assert!(r.work_date.is_empty() || valid_date(r.work_date));
        assert!(
            r.date.is_empty() || valid_date(r.date),
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
        let r = matching_serial(Some(serial), Some(region)).unwrap();
        assert_eq!((r.title, r.date), (title, date), "{serial}");
    }
    assert!(matching_serial(Some("SCUS-94900"), Some("Japan")).is_none());
    assert!(suggestions(Some("SCUS-94900"), "").next().is_some());
    assert!(matching_serial(None, None).is_none());
    assert!(
        suggestions(None, "Crash Bandicoot (Japan)")
            .next()
            .is_some()
    );
    assert!(
        matching_serial(Some("SLUS-00402"), Some("USA")).is_none(),
        "Tekken 3 should exercise a real catalog miss"
    );
    assert!(suggestions(None, "My Homebrew Game").next().is_none());
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
        let found = matching_serial(Some(serial), Some("USA"));
        if name.starts_with("Crash") {
            assert_eq!(found.unwrap().title, "Crash Bandicoot");
        }
        println!(
            "{name} | {serial} | {}",
            found.map_or("no match", |r| r.title)
        );
    }
}

#[test]
fn work_publication_is_distinct_from_the_regional_release_date() {
    let release = matching_serial(Some("SLUS-01524"), Some("USA")).unwrap();
    assert_eq!(release.date, "");
    assert_eq!(release.work_date, "2002");
}

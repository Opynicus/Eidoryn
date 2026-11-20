use super::*;
use crate::library::covers::Provider;

fn seed(lib: &Library, id: i64, serial: &str) {
    fixture(lib, id, Some(serial));
    let mut release = downloaded("Test", "Catalog title");
    release.serial = serial.to_owned();
    release.id = format!("Test/{serial}");
    refresh_with(lib, id, |_| Ok(response(vec![release]))).unwrap();
}

fn cached(lib: &Library, serial: &str) -> bool {
    lib.connect()
        .unwrap()
        .prepare("SELECT 1 FROM metadata_responses WHERE serial=?")
        .unwrap()
        .exists([serial])
        .unwrap()
}

#[test]
fn clearing_a_confirmed_release_drops_its_snapshot_and_historical_payload() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.db");
    let lib = Library::open(&path).unwrap();
    seed(&lib, 4, TEST_SERIAL);
    select(&lib, 4, &format!("Test/{TEST_SERIAL}")).unwrap();
    fixture(&lib, 5, Some(TEST_SERIAL));
    fixture(&lib, 6, Some(TEST_SERIAL));
    select(&lib, 6, &format!("Test/{TEST_SERIAL}")).unwrap();
    let mut newer = downloaded("Other", "Updated title");
    newer.work_date = "1995".into();
    refresh_with(&lib, 5, |_| Ok(response(vec![newer]))).unwrap();

    let count = |db: &Connection, table: &str| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, usize>(0)
        })
        .unwrap()
    };
    assert_eq!(
        count(&lib.connect().unwrap(), "referenced_metadata_responses"),
        1
    );
    lib.remove_games_and_clear_metadata(&[4]).unwrap();
    let db = lib.connect().unwrap();
    assert_eq!(count(&db, "referenced_metadata_responses"), 1);
    assert!(list(&lib).unwrap()[&6].release_id.is_some());
    lib.remove_games_and_clear_metadata(&[6]).unwrap();
    assert_eq!(count(&db, "referenced_metadata_responses"), 0);
    assert!(cached(&lib, TEST_SERIAL)); // Game 5 still uses the new revision.
    let rows = list(&lib).unwrap();
    assert!(rows[&4].release_id.is_none());
    assert_eq!(rows[&5].title.as_deref(), Some("Updated title"));

    lib.clear_metadata_cache(CacheScope::All).unwrap();
    drop(db);
    drop(lib);
    let lib = Library::open(&path).unwrap();
    let db = lib.connect().unwrap();
    for table in [
        "fetched_releases",
        "release_names",
        "metadata_responses",
        "referenced_metadata_responses",
        "metadata_sources",
    ] {
        assert_eq!(count(&db, table), 0, "{table}");
    }
    for row in list(&lib).unwrap().values() {
        assert!(row.release_id.is_none() && row.release_title.is_none());
        assert!(row.date.is_none() && row.work_date.is_none() && row.aliases.is_empty());
        assert!(row.source.is_none() && row.refreshed.is_none());
    }
}

#[test]
fn v8_upgrade_clears_old_snapshots_and_refetches_an_explicit_edition_by_its_serial() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.db");
    let lib = Library::open(&path).unwrap();
    let mut chosen = downloaded("Q2", "Previously chosen title");
    let other = downloaded("Q1", "Another edition");
    for (id, serial) in [(1, Some("SLPS-00001")), (2, None)] {
        fixture(&lib, id, serial);
        search_with(&lib, id, TEST_SERIAL, |_| {
            Ok(response(vec![other.clone(), chosen.clone()]))
        })
        .unwrap();
        select(&lib, id, &chosen.id).unwrap();
    }
    let unaffected = "SLUS-12345";
    seed(&lib, 3, unaffected);
    select(&lib, 3, &format!("Test/{unaffected}")).unwrap();
    set_override(&lib, 1, Field::Title, Some("Manual title")).unwrap();
    // Simulate v8 leaving all confirmed fields in place on a cache clear.
    lib.connect()
        .unwrap()
        .execute_batch(
            "UPDATE game_metadata SET cache_cleared=1,cache_generation=1 WHERE game_id IN (1,2);
         ALTER TABLE game_metadata DROP COLUMN preferred_release_id;
         PRAGMA user_version=8;",
        )
        .unwrap();
    drop(lib);

    let lib = Library::open(&path).unwrap();
    assert!(path.with_extension("db.v8.bak").exists());
    assert!(!cached(&lib, TEST_SERIAL));
    assert!(cached(&lib, unaffected));
    let rows = list(&lib).unwrap();
    for id in [1, 2] {
        assert!(rows[&id].release_id.is_none());
        assert!(rows[&id].release_title.is_none() && rows[&id].date.is_none());
        assert!(rows[&id].source.is_none() && rows[&id].cache_cleared);
        assert!(candidates(&lib, id, TEST_SERIAL).unwrap().is_empty());
    }
    assert_eq!(rows[&1].title.as_deref(), Some("Manual title"));
    assert_eq!(
        rows[&3].release_title.as_deref(),
        Some("Catalog title · USA")
    );

    // Fetching the same ambiguous catalog must select the exact preferred ID,
    // including for a game whose own serial was missing or from another market.
    chosen.title = "Newly downloaded title".into();
    for id in [1, 2] {
        assert_eq!(
            refresh_with(&lib, id, |serial| {
                assert_eq!(serial, TEST_SERIAL);
                Ok(response(vec![other.clone(), chosen.clone()]))
            })
            .unwrap(),
            RefreshOutcome::Updated
        );
        let card = &list(&lib).unwrap()[&id];
        assert_eq!(card.release_id.as_deref(), Some(chosen.id.as_str()));
        assert_eq!(
            card.release_title.as_deref(),
            Some("Newly downloaded title · USA")
        );
    }
}

#[test]
fn clearing_the_last_active_copy_releases_data_shared_with_a_previously_cleared_copy() {
    let root = tempfile::tempdir().unwrap();
    let lib = Library::open(&root.path().join("index.db")).unwrap();
    seed(&lib, 1, TEST_SERIAL);
    fixture(&lib, 2, Some(TEST_SERIAL));
    for id in [1, 2] {
        select(&lib, id, &format!("Test/{TEST_SERIAL}")).unwrap();
    }
    lib.remove_games_and_clear_metadata(&[1]).unwrap();
    assert!(cached(&lib, TEST_SERIAL));
    lib.remove_games_and_clear_metadata(&[2]).unwrap();
    assert!(!cached(&lib, TEST_SERIAL));
    assert!(candidates(&lib, 2, TEST_SERIAL).unwrap().is_empty());
}

#[test]
fn cleanup_scopes_preserve_shared_editions_user_data_and_covers() {
    let root = tempfile::tempdir().unwrap();
    let lib = Library::open(&root.path().join("library.db")).unwrap();
    let first = "SLUS-12345";
    let shared = "SLUS-12346";
    seed(&lib, 1, first);
    seed(&lib, 2, shared);
    fixture(&lib, 3, Some(shared));
    set_override(&lib, 1, Field::Title, Some("My title")).unwrap();
    set_override(&lib, 3, Field::Date, Some("2000")).unwrap();
    lib.connect()
        .unwrap()
        .execute_batch(
            "INSERT INTO sessions(session_id,game_id,seconds,last_played) VALUES('session',1,3600,100);
         INSERT INTO cover_assets(game_id,kind,source,original,thumb)
         VALUES(1,'user','local','custom.png','custom-thumb.png');",
        )
        .unwrap();
    lib.remove_games(&[1]).unwrap();
    assert!(cached(&lib, first));
    lib.remove_games_and_clear_metadata(&[2]).unwrap();
    assert!(cached(&lib, first));
    assert!(cached(&lib, shared));
    let rows = list(&lib).unwrap();
    assert!(rows[&2].release_id.is_none());
    assert!(rows[&3].release_id.is_some());
    lib.clear_metadata_cache(CacheScope::Removed).unwrap();
    assert!(!cached(&lib, first));
    assert!(cached(&lib, shared));
    let after = list(&lib).unwrap();
    assert!(after[&1].date.is_none());
    assert_eq!(after[&1].title.as_deref(), Some("My title"));
    assert!(after[&1].manual_cover);
    assert_eq!(
        lib.connect()
            .unwrap()
            .query_row("SELECT seconds FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3600
    );

    // A manually searched serial need not belong to any game.
    let orphan = "SLUS-12348";
    search_with(&lib, 3, orphan, |_| {
        let mut release = downloaded("Orphan", "Search result");
        release.serial = orphan.into();
        Ok(response(vec![release]))
    })
    .unwrap();
    assert!(cached(&lib, orphan));
    for _ in 0..2 {
        lib.clear_metadata_cache(CacheScope::All).unwrap();
        assert!(!cached(&lib, orphan));
        assert!(!cached(&lib, shared));
        assert!(candidates(&lib, 1, shared).unwrap().is_empty());
        let rows = list(&lib).unwrap();
        assert!(rows[&3].release_id.is_none());
        assert_eq!(rows[&3].date.as_deref(), Some("2000"));
        assert_eq!(rows[&3].serial.as_deref(), Some(shared));
    }
}

#[test]
fn removing_a_game_retains_candidates_needed_for_another_games_pending_title_match() {
    let root = tempfile::tempdir().unwrap();
    let lib = Library::open(&root.path().join("index.db")).unwrap();
    seed(&lib, 1, TEST_SERIAL);
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO games(id,fallback_title) VALUES(2,'Catalog title')",
        [],
    )
    .unwrap();
    identify(&tx, 2, None, None, "Catalog title (USA)").unwrap();
    tx.commit().unwrap();
    lib.remove_games_and_clear_metadata(&[1]).unwrap();
    assert!(list(&lib).unwrap()[&2].pending);
    assert_eq!(candidates(&lib, 2, "").unwrap().len(), 1);
    lib.clear_metadata_cache(CacheScope::All).unwrap();
    assert!(!list(&lib).unwrap()[&2].pending);
    assert!(candidates(&lib, 2, "").unwrap().is_empty());
}

#[test]
fn late_fetch_or_manual_serial_search_cannot_refill_a_cleared_game() {
    let root = tempfile::tempdir().unwrap();
    let lib = Library::open(&root.path().join("index.db")).unwrap();
    seed(&lib, 1, TEST_SERIAL);
    fixture(&lib, 2, Some(TEST_SERIAL));
    let batch = Batch::default();
    let cancel = AtomicBool::new(false);
    let cached_lookup = batch
        .prepare_with(&lib, 1, Mode::Incremental, &cancel, |_| {
            panic!("already cached")
        })
        .unwrap();
    let online_lookup = batch
        .prepare_with(&lib, 1, Mode::Full, &cancel, |_| {
            Ok(response(vec![downloaded("New", "New response")]))
        })
        .unwrap();
    crate::library::scrape::enqueue_games(&lib, &[1, 2], Mode::Full, Provider::Libretro).unwrap();
    lib.remove_games_and_clear_metadata(&[1]).unwrap();
    assert!(commit_one(&lib, 1, cached_lookup).is_err());
    assert!(commit_one(&lib, 1, online_lookup).is_err());
    assert!(list(&lib).unwrap()[&1].release_id.is_none());
    assert!(cached(&lib, TEST_SERIAL));
    assert_eq!(
        crate::library::scrape::due_games(&lib).unwrap(),
        [(2, Mode::Full)]
    );
    assert!(
        search_with(&lib, 2, "SLUS-45678", |_| {
            lib.clear_metadata_cache(CacheScope::All)?;
            let mut release = downloaded("Late", "Stale manual search");
            release.serial = "SLUS-45678".into();
            Ok(response(vec![release]))
        })
        .is_err()
    );
    assert!(!cached(&lib, "SLUS-45678"));
    assert_eq!(crate::library::scrape::due_games(&lib).unwrap(), []);
    refresh_with(&lib, 2, |_| {
        Ok(response(vec![downloaded("New", "New response")]))
    })
    .unwrap();
    assert_eq!(
        list(&lib).unwrap()[&2].title.as_deref(),
        Some("New response")
    );
}

#[test]
fn failed_cache_cleanup_rolls_back_a_batch_removal() {
    let root = tempfile::tempdir().unwrap();
    let lib = Library::open(&root.path().join("index.db")).unwrap();
    seed(&lib, 1, TEST_SERIAL);
    lib.connect()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_clear BEFORE DELETE ON fetched_releases
         BEGIN SELECT RAISE(ABORT,'cache cleanup failed'); END;",
        )
        .unwrap();
    assert!(lib.remove_games_and_clear_metadata(&[1]).is_err());
    assert!(
        !lib.connect()
            .unwrap()
            .query_row("SELECT hidden FROM games WHERE id=1", [], |r| r
                .get::<_, bool>(0))
            .unwrap()
    );
    assert!(list(&lib).unwrap()[&1].release_id.is_some());
    assert!(cached(&lib, TEST_SERIAL));
}

use super::{GameMove, Library, Placement};
use crate::disc::DiscImage;
use rusqlite::params;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

struct Fixture {
    root: tempfile::TempDir,
    media: PathBuf,
    db: Library,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db = Library::open(&temp.path().join("index.sqlite3")).unwrap();
        let media = temp.path().join("media");
        fs::create_dir(&media).unwrap();
        Self {
            root: temp,
            db,
            media,
        }
    }
    fn image(&self, name: &str, byte: u8) -> PathBuf {
        let path = self.media.join(name);
        fs::write(&path, [byte; 2048 * 5]).unwrap();
        path.canonicalize().unwrap()
    }
    fn scan(&self) -> super::ScanReport {
        self.db
            .scan_directory(&self.media, true, &AtomicBool::new(false), |_| {})
            .unwrap()
    }
    fn session(&self, game: i64, seconds: i64) {
        self.db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO sessions(session_id,game_id,seconds,last_played) VALUES(?,?,?,110)",
                params![format!("session-{game}"), game, seconds],
            )
            .unwrap();
    }
}
fn replace_file(path: &Path, byte: u8) {
    let mut file = File::create(path).unwrap();
    file.write_all(&[byte; 2048 * 5]).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(2))
        .unwrap();
}

#[test]
fn rescanning_after_a_clear_only_queues_new_games() {
    use super::{covers::Provider, metadata, scrape};
    let mut f = Fixture::new();
    f.image("Existing.iso", 1);
    f.scan();
    let original = f.db.games().unwrap()[0].id;
    f.db.clear_metadata_cache(metadata::CacheScope::All)
        .unwrap();
    f.db = Library::open(&f.root.path().join("index.sqlite3")).unwrap();
    f.db.configure_import_scraping(Some(Provider::Libretro));
    f.image("New.iso", 2);
    let report = f.scan();
    assert_eq!((report.existing, report.added), (1, 1));
    let rows = metadata::list(&f.db).unwrap();
    assert!(rows[&original].cache_cleared);
    let new =
        f.db.games()
            .unwrap()
            .into_iter()
            .find(|g| g.id != original)
            .unwrap();
    assert!(!rows[&new.id].cache_cleared);
    assert_eq!(
        scrape::due_games(&f.db).unwrap(),
        [(new.id, scrape::Mode::Incremental)]
    );
}

#[test]
fn scans_are_incremental_and_an_overwritten_path_does_not_inherit_playtime() {
    let f = Fixture::new();
    let image = f.image("Game.iso", 0);
    assert_eq!(f.scan().added, 1);
    let game = f.db.games().unwrap().remove(0);
    let identity = DiscImage::open(&image).unwrap().identity().to_owned();
    assert_eq!(
        f.db.associate_opened(&image, &identity, true).unwrap(),
        game.id
    );
    f.session(game.id, 100);
    assert_eq!(f.scan().existing, 1);
    replace_file(&image, 1);
    assert!(f.db.associate_opened(&image, &identity, false).is_err());
    assert_eq!(f.scan().added, 1);
    let games = f.db.games().unwrap();
    assert_eq!(games.len(), 2);
    assert_eq!(games[0].seconds, 100);
    assert!(!games[0].locations[0].available);
    assert_eq!(games[1].seconds, 0);
    assert!(games[1].locations[0].available);
    assert!(f.db.relocate(games[0].locations[0].id, &image).is_err());
    // Removing the superseded revision must leave the new occupant launchable.
    f.db.remove(games[0].id).unwrap();
    assert!(f.db.removed_recent_paths(&[image]).unwrap().is_empty());
}

#[test]
fn verified_images_keep_their_history_across_timestamp_changes_and_moves() {
    let f = Fixture::new();
    let path = f.image("old.iso", 13);
    f.scan();
    let id = DiscImage::open(&path).unwrap().identity().to_owned();
    let game = f.db.associate_opened(&path, &id, true).unwrap();
    f.session(game, 95);
    let location = f.db.games().unwrap()[0].locations[0].id;
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    assert_eq!(f.scan().existing, 1);
    let same = f.db.games().unwrap();
    assert_eq!(same.len(), 1);
    assert_eq!(
        (same[0].id, same[0].seconds, same[0].locations[0].id),
        (game, 95, location)
    );
    assert!(same[0].locations[0].fast_boot);
    let next = f.media.join("moved.iso");
    fs::rename(path, &next).unwrap();
    let next = next.canonicalize().unwrap();
    f.scan();
    let moved = f.db.games().unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!((moved[0].id, moved[0].seconds), (game, 95));
    assert!(
        moved[0]
            .locations
            .iter()
            .any(|l| l.available && l.path == next)
    );
}

#[test]
fn concurrent_imports_of_the_same_path_create_one_entry() {
    let mut f = Fixture::new();
    let additions = f.db.watch_additions();
    let path = f.image("shared.iso", 9);
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|workers| {
        for _ in 0..4 {
            workers.spawn(|| {
                barrier.wait();
                f.db.scan_files(std::slice::from_ref(&path), &AtomicBool::new(false), |_| {})
                    .unwrap();
            });
        }
    });
    let games = f.db.games().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].locations.len(), 1);
    assert_eq!(additions.try_iter().collect::<Vec<_>>(), [games[0].id]);
}

#[test]
fn rename_offline_remove_and_explicit_readd_preserve_game() {
    let f = Fixture::new();
    let path = f.image("old.iso", 1);
    f.scan();
    let g = f.db.games().unwrap().remove(0);
    f.session(g.id, 67);
    let new = f.media.join("renamed.iso");
    fs::rename(path, &new).unwrap();
    f.db.relocate(g.locations[0].id, &new).unwrap();
    assert_eq!(
        f.db.games().unwrap()[0].locations[0].path,
        new.canonicalize().unwrap()
    );
    let unplugged = f.media.with_extension("offline");
    fs::rename(&f.media, &unplugged).unwrap();
    assert_eq!(f.scan().failed, 1);
    let roots = f.db.roots().unwrap();
    assert_eq!(roots.len(), 1);
    assert!(!roots[0].available);
    assert!(!f.db.games().unwrap()[0].locations[0].available);
    fs::rename(unplugged, &f.media).unwrap();
    assert_eq!(f.scan().existing, 1);
    f.db.remove(g.id).unwrap();
    assert_eq!(f.scan().added, 0);
    assert!(f.db.games().unwrap().is_empty());
    let reloaded = Library::open(&f.root.path().join("index.sqlite3")).unwrap();
    reloaded
        .scan_files(&[new], &AtomicBool::new(false), |_| {})
        .unwrap();
    let restored = reloaded.games().unwrap().remove(0);
    assert_eq!((restored.id, restored.seconds), (g.id, 67));
}

#[test]
#[cfg(unix)]
fn offline_scan_through_a_directory_alias_updates_the_registered_root() {
    let f = Fixture::new();
    let alias = f.root.path().join("alias");
    std::os::unix::fs::symlink(f.root.path(), &alias).unwrap();
    let root = alias.join("media");
    f.image("Disc.iso", 1);
    let scan = || {
        f.db.scan_directory(&root, false, &AtomicBool::default(), |_| {})
            .unwrap()
    };
    assert_eq!(scan().added, 1);
    fs::rename(&f.media, f.root.path().join("offline")).unwrap();
    assert_eq!(scan().failed, 1);
    let roots = f.db.roots().unwrap();
    assert_eq!(roots.len(), 1);
    assert!(!roots[0].available);
    assert!(!f.db.games().unwrap()[0].locations[0].available);
}

#[test]
fn relocate_after_rescan_retains_both_session_streams_and_original_metadata() {
    let f = Fixture::new();
    let old = f.image("original.iso", 1);
    f.scan();
    let original = f.db.games().unwrap().remove(0);
    super::metadata::set_override(
        &f.db,
        original.id,
        super::metadata::Field::Title,
        Some("Custom"),
    )
    .unwrap();
    let moved = f.media.join("renamed.iso");
    fs::rename(old, &moved).unwrap();
    assert_eq!(f.scan().added, 1); // no strong identity was yet available
    let second = f.db.games().unwrap().remove(1);
    f.session(original.id, 180);
    f.session(second.id, 240);
    f.db.relocate(original.locations[0].id, &moved).unwrap();
    let games = f.db.games().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!((games[0].id, games[0].seconds), (original.id, 420));
    assert_eq!(games[0].locations.len(), 1);
    assert_eq!(games[0].locations[0].path, moved.canonicalize().unwrap());
    assert_eq!(
        super::metadata::list(&f.db).unwrap()[&original.id]
            .title
            .as_deref(),
        Some("Custom")
    );
    // The runner still owns the session for the formerly independent disc.
    f.db.connect()
        .unwrap()
        .execute(
            "UPDATE sessions SET seconds=260 WHERE game_id=?",
            [second.id],
        )
        .unwrap();
    assert_eq!(f.db.games().unwrap()[0].seconds, 440);
}

#[test]
fn confirmed_disc_group_aggregates_independent_sessions_and_can_be_undone() {
    let f = Fixture::new();
    f.image("disc1.iso", 1);
    f.image("disc2.iso", 2);
    f.scan();
    let games = f.db.games().unwrap();
    f.session(games[0].id, 180);
    f.session(games[1].id, 240);
    let second = games[1].locations[0].disc_id;
    f.db.group_disc(second, games[0].id).unwrap();
    let grouped = f.db.games().unwrap();
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].seconds, 7 * 60);
    assert_eq!(grouped[0].locations[1].disc_number, 2);
    f.db.separate_disc(second).unwrap();
    let separated = f.db.games().unwrap();
    assert_eq!(separated[0].seconds, 180);
    assert_eq!(separated[1].seconds, 240);
}

#[test]
fn candidate_copies_are_independent_until_verified() {
    let f = Fixture::new();
    f.image("first.iso", 1);
    f.image("copy.iso", 1);
    assert_eq!(f.scan().pending, 1);
    assert_eq!(f.scan().pending, 2); // both cached entries still await verification
    let games = f.db.games().unwrap();
    let (a, b) = (&games[0].locations[0], &games[1].locations[0]);
    f.db.confirm_copy(a.id, b.id).unwrap();
    assert_eq!(f.scan().pending, 0);
    let grouped = f.db.games().unwrap();
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].locations.len(), 2);
    assert_eq!(
        grouped[0].locations[0].disc_number,
        grouped[0].locations[1].disc_number
    );
    f.db.separate_disc(a.disc_id).unwrap();
    assert_eq!(f.db.games().unwrap().len(), 2);
}

#[test]
fn unresolved_metadata_survives_incremental_rescans() {
    let f = Fixture::new();
    f.db.connect()
        .unwrap()
        .execute_batch(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
             VALUES('Q1/SCUS-00001','SCUS-00001','USA','Test Adventure','','','');
             INSERT INTO release_names(name,release_id) VALUES('test adventure','Q1/SCUS-00001')",
        )
        .unwrap();
    f.image("Test Adventure.iso", 3);
    assert_eq!(f.scan().pending, 1); // name-only cached candidate
    let cached = f.scan();
    assert_eq!((cached.existing, cached.pending), (1, 1));
}

#[test]
fn confirming_copies_preserves_prior_hashes_if_a_replacement_keeps_its_timestamp() {
    let f = Fixture::new();
    let path = f.image("original.iso", 1);
    f.image("other.iso", 2);
    f.scan();
    let games = f.db.games().unwrap();
    let original = &games[0].locations[0];
    let other = &games[1].locations[0];
    let identity = DiscImage::open(&path).unwrap().identity().to_owned();
    f.db.associate_opened(&path, &identity, true).unwrap();
    let stamp = fs::metadata(&path).unwrap().modified().unwrap();
    fs::copy(&other.path, &path).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    for (a, b) in [(original, other), (other, original)] {
        assert!(f.db.confirm_copy(a.id, b.id).is_err());
    }
    assert_eq!(f.db.games().unwrap().len(), 2);
    let stored: String =
        f.db.connect()
            .unwrap()
            .query_row(
                "SELECT strong_id FROM discs WHERE id = ?",
                [original.disc_id],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(stored, identity);
}

#[test]
fn cue_suppresses_tracks_and_reports_invalid_or_ambiguous_entries() {
    let f = Fixture::new();
    let data = f.media.join("d.bin");
    let mut sector = [0; 2352];
    sector[1..11].fill(255);
    sector[15] = 2;
    fs::write(&data, sector).unwrap();
    fs::write(f.media.join("audio.bin"), [42; 2352]).unwrap();
    let cue = f.media.join("disc.cue");
    fs::write(&cue,"FILE \"d.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 01 00:00:00\n").unwrap();
    fs::write(
        f.media.join("bad.cue"),
        "FILE \"absent.bin\" BINARY\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n",
    )
    .unwrap();
    let report = f.scan();
    assert_eq!((report.added, report.failed), (1, 1));
    assert!(report.errors[0].contains("absent.bin"));
    assert_eq!(f.db.games().unwrap().len(), 1);
    let other = f.media.join("alt.cue");
    fs::copy(&cue, &other).unwrap();
    let single =
        f.db.scan_files(&[data], &AtomicBool::new(false), |_| {})
            .unwrap();
    assert_eq!((single.added, single.pending), (0, 1));
    assert_eq!(single.choices[0].cues.len(), 2);
    fs::remove_file(f.media.join("audio.bin")).unwrap();
    assert!(f.scan().failed >= 2);
    assert!(!f.db.games().unwrap()[0].locations[0].available);
}

#[test]
fn cooked_firmware_bin_is_not_a_game() {
    let f = Fixture::new();
    fs::write(f.media.join("firmware.bin"), vec![0; 512 * 1024]).unwrap();
    let result = f.scan();
    assert_eq!((result.added, result.failed), (0, 1));
}

#[test]
fn finding_a_cue_after_its_track_hides_the_standalone_card() {
    let f = Fixture::new();
    let mut sector = [0; 2352];
    sector[1..11].fill(255);
    sector[15] = 2;
    fs::write(f.media.join("track.bin"), sector).unwrap();
    assert_eq!(f.scan().added, 1);
    let track_id = f.db.games().unwrap()[0].id;
    let cue = f.media.join("disc.cue");
    fs::write(
        &cue,
        "FILE track.bin BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
    )
    .unwrap();
    assert_eq!(f.scan().added, 1);
    let shown = f.db.games().unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].locations[0].path, cue.canonicalize().unwrap());
    fs::remove_file(&cue).unwrap();
    f.scan();
    assert!(
        f.db.games()
            .unwrap()
            .iter()
            .any(|g| g.id == track_id && g.locations[0].available)
    );
}

#[test]
fn recent_bin_follows_all_current_cue_owners() {
    let f = Fixture::new();
    let track = f.media.join("track.bin");
    let mut sector = [0; 2352];
    sector[1..11].fill(255);
    sector[15] = 2;
    fs::write(&track, sector).unwrap();
    assert_eq!(f.scan().added, 1);
    for name in ["first.cue", "second.cue"] {
        fs::write(
            f.media.join(name),
            "FILE track.bin BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
        )
        .unwrap();
    }
    assert_eq!(f.scan().added, 2);
    let games = f.db.games().unwrap();
    let [first, second] = games.as_slice() else {
        panic!("expected two visible CUEs");
    };
    let first_path = first.locations[0].path.clone();
    let second_path = second.locations[0].path.clone();
    let paths = [
        track.canonicalize().unwrap(),
        first_path.clone(),
        second_path,
    ];
    f.db.remove(first.id).unwrap();
    assert_eq!(
        f.db.removed_recent_paths(&paths).unwrap(),
        [first_path].into()
    );
    f.db.remove(second.id).unwrap();
    assert_eq!(
        f.db.removed_recent_paths(&paths).unwrap(),
        paths.clone().into()
    );
    f.db.migrate_recents(&[crate::preferences::RecentDisc {
        path: paths[0].clone(),
        fast_boot: true,
    }])
    .unwrap();
    assert!(f.db.games().unwrap().is_empty());
    assert_eq!(
        f.db.connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM migrated_recent", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn removed_recent_paths_follow_grouped_discs_copies_and_explicit_readd() {
    let f = Fixture::new();
    let first = f.image("Disc 1.iso", 11);
    let copy = f.image("Copy.iso", 11);
    let second = f.image("Disc 2.iso", 12);
    let unknown = f.image("Unindexed.iso", 13);
    let mut homes = Vec::new();
    for path in [&first, &copy, &second] {
        let identity = DiscImage::open(path).unwrap().identity().to_owned();
        homes.push(f.db.associate_opened(path, &identity, true).unwrap());
    }
    let second_game = f.db.games_by_id(&[homes[2]]).unwrap().remove(0);
    f.db.group_disc(second_game.locations[0].disc_id, homes[0])
        .unwrap();
    let paths = [first, copy, second, unknown];
    f.db.remove_games_and_clear_metadata(&[homes[0]]).unwrap();
    let reopened = Library::open(&f.root.path().join("index.sqlite3")).unwrap();
    assert_eq!(
        reopened.removed_recent_paths(&paths).unwrap(),
        paths[..3].iter().cloned().collect()
    );
    reopened
        .scan_files(&paths[..1], &AtomicBool::default(), |_| {})
        .unwrap();
    assert!(reopened.removed_recent_paths(&paths).unwrap().is_empty());
}

#[test]
fn cancellation_preserves_completed_entries_migration_is_idempotent() {
    let mut f = Fixture::new();
    let additions = f.db.watch_additions();
    let first = f.image("a.iso", 0);
    f.image("b.iso", 2);
    let flag = AtomicBool::new(false);
    let partial =
        f.db.scan_directory(&f.media, true, &flag, |r| {
            if r.added == 1 {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(partial.added, 1);
    let first_id = f.db.games().unwrap()[0].id;
    assert_eq!(additions.try_iter().collect::<Vec<_>>(), [first_id]);
    assert_eq!(f.scan().added, 1);
    let second_id = f.db.games().unwrap()[1].id;
    assert_eq!(additions.try_iter().collect::<Vec<_>>(), [second_id]);
    let recent = crate::preferences::RecentDisc {
        path: first,
        fast_boot: true,
    };
    for _ in 0..2 {
        f.db.migrate_recents(std::slice::from_ref(&recent)).unwrap();
    }
    let games = f.db.games().unwrap();
    assert_eq!(games.len(), 2);
    assert!(games[0].locations[0].fast_boot);
    assert!(games[0].last_played.is_none());
    f.db.remove(games[0].id).unwrap();
    f.db.migrate_recents(&[recent]).unwrap();
    assert_eq!(f.db.games().unwrap().len(), 1);
}

#[test]
fn additions_include_first_launch_and_migration_but_skip_verified_copies_and_reopens() {
    let mut f = Fixture::new();
    let additions = f.db.watch_additions();
    let writer = f.db.clone();
    let path = f.image("direct.iso", 3);
    let identity = DiscImage::open(&path).unwrap().identity().to_owned();
    let id = writer.associate_opened(&path, &identity, true).unwrap();
    assert_eq!(additions.try_iter().collect::<Vec<_>>(), [id]);
    writer.associate_opened(&path, &identity, false).unwrap();
    let copy = f.image("copy.iso", 3);
    let copy_home = writer.associate_opened(&copy, &identity, true).unwrap();
    assert_ne!(copy_home, id);
    assert_eq!(f.db.games().unwrap().len(), 1);
    assert!(additions.try_iter().next().is_none());

    let recent = crate::preferences::RecentDisc {
        path: f.image("recent.iso", 4),
        fast_boot: false,
    };
    for _ in 0..2 {
        f.db.migrate_recents(std::slice::from_ref(&recent)).unwrap();
    }
    f.scan();
    let recent_id = f.db.games().unwrap()[1].id;
    assert_eq!(additions.try_iter().collect::<Vec<_>>(), [recent_id]);
}

#[test]
fn opening_a_future_schema_preserves_data() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("future.sqlite3");
    let db = rusqlite::Connection::open(&file).unwrap();
    db.execute_batch(
        "CREATE TABLE marker(x); INSERT INTO marker VALUES(10); PRAGMA user_version=32;",
    )
    .unwrap();
    assert!(Library::open(&file).is_err());
    assert_eq!(
        db.query_row("SELECT x FROM marker", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn bulk_remove_is_atomic_and_explicit_reimport_restores_data() {
    let f = Fixture::new();
    for (i, name) in ["A.iso", "B.iso", "C.iso"].into_iter().enumerate() {
        f.image(name, i as u8);
    }
    f.scan();
    let games = f.db.games().unwrap();
    let ids = [games[0].id, games[1].id, games[0].id, -1];
    f.session(ids[0], 92);
    super::metadata::set_override(&f.db, ids[0], super::metadata::Field::Title, Some("Custom"))
        .unwrap();
    let db = f.db.connect().unwrap();
    db.execute_batch(&format!(
        "CREATE TRIGGER deny_hide BEFORE UPDATE OF hidden ON games WHEN NEW.id = {}
         BEGIN SELECT RAISE(ABORT, 'cannot hide'); END;",
        ids[1]
    ))
    .unwrap();
    assert!(f.db.remove_games(&ids).is_err());
    assert_eq!(f.db.games().unwrap().len(), 3);
    let ignored = || {
        db.query_row("SELECT COUNT(*) FROM ignored", [], |r| r.get::<_, usize>(0))
            .unwrap()
    };
    assert_eq!(ignored(), 0);
    db.execute_batch("DROP TRIGGER deny_hide").unwrap();
    f.db.remove_games(&[]).unwrap();
    f.db.remove_games(&ids).unwrap();
    f.db.remove_games(&ids).unwrap();
    assert_eq!(ignored(), 2);
    f.scan();
    assert_eq!(f.db.games().unwrap()[0].id, games[2].id);
    assert_eq!(f.db.games().unwrap().len(), 1);
    let paths = [
        games[0].locations[0].path.clone(),
        games[1].locations[0].path.clone(),
    ];
    assert!(paths.iter().all(|p| p.is_file()));
    f.db.scan_files(&paths, &AtomicBool::default(), |_| {})
        .unwrap();
    assert_eq!(f.db.games().unwrap().len(), 3);
    assert_eq!(f.db.games_by_id(&[ids[0]]).unwrap()[0].seconds, 92);
    assert_eq!(
        super::metadata::list(&f.db).unwrap()[&ids[0]]
            .title
            .as_deref(),
        Some("Custom")
    );
}

#[test]
fn moves_use_the_displayed_order_and_keep_filtered_or_new_games_in_place() {
    let fixture = Fixture::new();
    for (i, name) in ["A.iso", "B.iso", "C.iso", "D.iso", "E.iso"]
        .into_iter()
        .enumerate()
    {
        fixture.image(name, i as u8);
    }
    fixture.scan();
    let games = fixture.db.games().unwrap();
    let [first, second_game, third, fourth, fifth] = games.as_slice() else {
        panic!("expected five games")
    };
    let movement = GameMove {
        source: first.id,
        target: third.id,
        placement: Placement::After,
        initial_order: vec![fifth.id, first.id, third.id, third.id, -1],
    };
    // A second window imports a game after the first window rendered its list.
    let second = Library::open(&fixture.root.path().join("index.sqlite3")).unwrap();
    let extra = fixture.image("F.iso", 5);
    second
        .scan_files(&[extra], &AtomicBool::default(), |_| {})
        .unwrap();
    let new = second.games().unwrap()[5].id;
    let expected = [fifth.id, second_game.id, third.id, fourth.id, first.id, new];
    let mut preview = vec![first.id, second_game.id, third.id, fourth.id, fifth.id, new];
    assert!(movement.apply(&mut preview));
    assert_eq!(preview, expected);
    assert_eq!(fixture.db.move_game(&movement).unwrap(), expected);
    let reopened = Library::open(&fixture.root.path().join("index.sqlite3")).unwrap();
    for game in reopened.games().unwrap() {
        assert_eq!(
            game.manual_position.map(|i| expected[i as usize]),
            Some(game.id)
        );
    }
    second.remove(third.id).unwrap();
    assert!(fixture.db.move_game(&movement).is_err());
    assert!(reopened.games_by_id(&[third.id]).unwrap().is_empty());
    // Hidden games retain their slot and reappear there on explicit reimport.
    let reverse = GameMove {
        source: new,
        target: fifth.id,
        placement: Placement::Before,
        initial_order: Vec::new(),
    };
    assert_eq!(
        second.move_game(&reverse).unwrap(),
        [new, fifth.id, third.id, second_game.id, fourth.id, first.id]
    );
    reopened
        .scan_files(
            std::slice::from_ref(&third.locations[0].path),
            &AtomicBool::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        reopened.games_by_id(&[third.id]).unwrap()[0].manual_position,
        Some(2)
    );
    fixture.image("G.iso", 6);
    fixture.scan();
    assert_eq!(
        reopened.games().unwrap().last().unwrap().manual_position,
        None
    );
}

#[test]
fn independent_simultaneous_drops_preserve_each_others_changes() {
    let fixture = Fixture::new();
    for (i, name) in ["A.iso", "B.iso", "C.iso", "D.iso"].into_iter().enumerate() {
        fixture.image(name, i as u8);
    }
    fixture.scan();
    let games = fixture.db.games().unwrap();
    let [first, second_game, third, fourth] = games.as_slice() else {
        panic!("expected four games")
    };
    let second = Library::open(&fixture.root.path().join("index.sqlite3")).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|workers| {
        for (store, source, target) in [
            (&fixture.db, second_game.id, first.id),
            (&second, fourth.id, third.id),
        ] {
            let barrier = &barrier;
            workers.spawn(move || {
                barrier.wait();
                store
                    .move_game(&GameMove {
                        source,
                        target,
                        placement: Placement::Before,
                        initial_order: Vec::new(),
                    })
                    .unwrap();
            });
        }
    });
    let mut after = fixture.db.games().unwrap();
    after.sort_by_key(|game| game.manual_position);
    assert_eq!(
        after.iter().map(|game| game.id).collect::<Vec<_>>(),
        [second_game.id, first.id, fourth.id, third.id]
    );
}

#[test]
fn migrating_v5_through_v7_preserves_games_positions_and_playtime() {
    for (version, position) in [(5, None), (6, Some(7)), (7, Some(8))] {
        let f = Fixture::new();
        f.image("Original.iso", 1);
        f.scan();
        let game = f.db.games().unwrap()[0].id;
        f.session(game, 26);
        let db = f.db.connect().unwrap();
        db.execute_batch("ALTER TABLE game_metadata DROP COLUMN preferred_release_id; ALTER TABLE game_metadata DROP COLUMN cache_cleared")
            .unwrap();
        if version < 7 {
            db.execute_batch("ALTER TABLE game_metadata DROP COLUMN cache_generation")
                .unwrap();
        } else {
            // v7 recorded earlier clears, but users could subsequently restore their data.
            db.execute_batch("UPDATE game_metadata SET cache_generation=2")
                .unwrap();
        }
        if version == 5 {
            db.execute_batch("ALTER TABLE games DROP COLUMN manual_position")
                .unwrap();
        } else {
            db.execute("UPDATE games SET manual_position=?", [position])
                .unwrap();
        }
        db.pragma_update(None, "user_version", version).unwrap();
        drop(db);
        drop(f.db);

        let migrated = Library::open(&f.root.path().join("index.sqlite3")).unwrap();
        let old_game = migrated.games().unwrap().remove(0);
        assert!(!super::metadata::list(&migrated).unwrap()[&game].cache_cleared);
        assert_eq!(
            (old_game.id, old_game.seconds, old_game.manual_position),
            (game, 26, position)
        );
        migrated
            .clear_metadata_cache(super::metadata::CacheScope::All)
            .unwrap();
        assert!(
            f.root
                .path()
                .join(format!("index.sqlite3.v{version}.bak"))
                .is_file()
        );
    }
}

/// Run in isolation with --ignored --nocapture --test-threads=1. Sparse test
/// media mix small and CD-sized images without using proprietary game bytes.
#[test]
#[ignore = "reproducible 1,000-image index benchmark"]
fn benchmark_thousand_images() {
    use std::time::Instant;
    let f = Fixture::new();
    let mut bytes = 0;
    for i in 0..1000_u32 {
        let mut file = File::create(f.media.join(format!("game-{i:04}.iso"))).unwrap();
        file.write_all(&i.to_le_bytes()).unwrap();
        let len = [48 * 2048, 1 << 20, 32 << 20, 600 << 20][i as usize % 4];
        file.set_len(len).unwrap();
        bytes += len;
    }
    // Linux's rchar includes SQLite and media file reads, even from page cache.
    let io_chars = || {
        fs::read_to_string("/proc/self/io").ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("rchar: ")?.parse::<u64>().ok())
        })
    };
    for cold in [true, false] {
        let before = io_chars();
        let start = Instant::now();
        let mut first = None;
        let result =
            f.db.scan_directory(&f.media, true, &AtomicBool::new(false), |r| {
                if first.is_none() && r.added + r.existing >= 12 {
                    first = Some(start.elapsed());
                }
            })
            .unwrap();
        let elapsed = start.elapsed();
        let delta = before.zip(io_chars()).map(|(a, b)| b - a);
        assert_eq!(
            (result.added, result.existing, result.failed),
            if cold { (1000, 0, 0) } else { (0, 1000, 0) }
        );
        let list = Instant::now();
        assert_eq!(f.db.games().unwrap().len(), 1000);
        let list = list.elapsed();
        eprintln!(
            "cold_index={cold}, logical_bytes={bytes}, first_12={first:?}, elapsed={elapsed:?}, load_rows={list:?}, read_chars={delta:?}"
        );
    }
    assert_eq!(
        f.db.connect()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM discs WHERE strong_id IS NOT NULL",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    if let Ok(status) = fs::read_to_string("/proc/self/status")
        && let Some(line) = status.lines().find(|l| l.starts_with("VmHWM:"))
    {
        eprintln!("{line}");
    }
}

#[test]
fn targeted_reads_include_only_requested_visible_games_and_their_statistics() {
    let f = Fixture::new();
    f.image("One.iso", 1);
    f.image("Two.iso", 2);
    f.image("Three.iso", 3);
    f.scan();
    let games = f.db.games().unwrap();
    let [one, two, three] = games.as_slice() else {
        panic!("expected three distinct entries")
    };
    f.session(two.id, 123);
    f.db.remove(three.id).unwrap();
    super::metadata::set_override(
        &f.db,
        two.id,
        super::metadata::Field::Title,
        Some("Localized"),
    )
    .unwrap();
    let ids = [two.id, three.id, two.id];
    let updated = f.db.games_by_id(&ids).unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!((updated[0].id, updated[0].seconds), (two.id, 123));
    assert_eq!(updated[0].locations[0].path, two.locations[0].path);
    let fields = super::metadata::list_by_id(&f.db, &ids).unwrap();
    assert_eq!(fields[&two.id].title.as_deref(), Some("Localized"));
    assert!(!fields.contains_key(&one.id));
    assert!(f.db.games_by_id(&[]).unwrap().is_empty());
}

#[test]
fn directory_scan_commits_the_parent_before_discovering_child_media() {
    let f = Fixture::new();
    f.image("root.iso", 1);
    let child = f.media.join("child");
    fs::create_dir(&child).unwrap();
    let mut inserted = false;
    let report =
        f.db.scan_directory(&f.media, true, &AtomicBool::default(), |progress| {
            if progress.added == 1 && !inserted {
                assert_eq!(f.db.games().unwrap()[0].title, "root");
                fs::write(child.join("later.iso"), [2; 2048 * 5]).unwrap();
                inserted = true;
            }
        })
        .unwrap();
    assert_eq!((report.added, report.total, report.processed), (2, 2, 2));
}

#[test]
fn cancelling_parent_scan_preserves_unvisited_child_availability() {
    let f = Fixture::new();
    let child = f.media.join("child");
    fs::create_dir(&child).unwrap();
    let missing = child.join("missing.iso");
    fs::write(&missing, [2; 2048 * 5]).unwrap();
    f.scan();
    fs::remove_file(&missing).unwrap();
    f.image("new.iso", 1);
    let cancel = AtomicBool::default();
    let report =
        f.db.scan_directory(&f.media, true, &cancel, |progress| {
            if progress.added == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(report.added, 1);
    let available = || {
        f.db.games()
            .unwrap()
            .iter()
            .find(|game| game.title == "missing")
            .unwrap()
            .locations[0]
            .available
    };
    assert!(available());
    f.scan();
    assert!(!available());
}

#[test]
fn parent_cue_claims_nested_bin_without_creating_a_second_game_or_scrape_jobs() {
    let mut f = Fixture::new();
    f.db.configure_import_scraping(Some(super::covers::Provider::Libretro));
    let child = f.media.join("audio");
    fs::create_dir(&child).unwrap();
    let mut sector = [0; 2352];
    sector[1..11].fill(255);
    sector[15] = 2;
    fs::write(child.join("track.bin"), sector).unwrap();
    let cue = f.media.join("release.cue");
    fs::write(
        &cue,
        "FILE \"audio/track.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
    )
    .unwrap();
    let report = f.scan();
    assert_eq!((report.added, report.failed, report.processed), (1, 0, 2));
    assert_eq!(
        f.db.games().unwrap()[0].locations[0].path,
        cue.canonicalize().unwrap()
    );
    let counts: (i64, i64) =
        f.db.connect()
            .unwrap()
            .query_row(
                "SELECT (SELECT count(*) FROM games),count(*) FROM scrape_jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
    assert_eq!(counts, (1, 2));
}

#[test]
fn explicit_reimport_after_missing_file_check_keeps_the_location_available() {
    let f = Fixture::new();
    let old = f.image("temporarily-offline.iso", 1);
    f.scan();
    let storage = f.root.path().join("detached.iso");
    fs::rename(&old, &storage).unwrap();
    let db = f.db.connect().unwrap();
    let seen: i64 = db
        .query_row("SELECT last_seen_at FROM locations", [], |r| r.get(0))
        .unwrap();
    let mut checked = false;
    f.db.scan_directory(&f.media, true, &AtomicBool::default(), |progress| {
        if progress.phase == super::ScanPhase::CheckingAvailability
            && progress.current_file.as_ref() == Some(&old)
        {
            assert!(!checked);
            fs::rename(&storage, &old).unwrap();
            let report =
                f.db.scan_files(std::slice::from_ref(&old), &AtomicBool::default(), |_| {})
                    .unwrap();
            assert_eq!(report.existing, 1);
            // Preserve equal timestamps even if the system clock ticked during the test.
            db.execute("UPDATE locations SET last_seen_at=?", [seen])
                .unwrap();
            assert!(
                db.query_row(
                    "SELECT availability_check IS NULL FROM locations",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
            );
            checked = true;
        }
    })
    .unwrap();
    assert!(checked);
    assert!(f.db.games().unwrap()[0].locations[0].available);
}

#[test]
fn relocating_after_missing_file_check_keeps_the_new_path_available() {
    let f = Fixture::new();
    let old = f.image("move.iso", 2);
    f.scan();
    let location = f.db.games().unwrap()[0].locations[0].id;
    let new = f.root.path().join("new.iso");
    fs::rename(&old, &new).unwrap();
    let mut relocated = false;
    f.db.scan_directory(&f.media, true, &AtomicBool::default(), |progress| {
        if progress.phase == super::ScanPhase::CheckingAvailability
            && progress.current_file.as_ref() == Some(&old)
        {
            f.db.relocate(location, &new).unwrap();
            relocated = true;
        }
    })
    .unwrap();
    assert!(relocated);
    let games = f.db.games().unwrap();
    let location = &games[0].locations[0];
    assert_eq!(location.path, new.canonicalize().unwrap());
    assert!(location.available);
}

#[test]
fn cancelling_availability_checks_releases_the_observation_and_preserves_the_location() {
    let f = Fixture::new();
    let old = f.image("unplugged.iso", 3);
    f.scan();
    fs::remove_file(&old).unwrap();
    let cancel = AtomicBool::default();
    f.db.scan_directory(&f.media, true, &cancel, |progress| {
        if progress.phase == super::ScanPhase::CheckingAvailability
            && progress.current_file.as_ref() == Some(&old)
        {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    let available: bool =
        f.db.connect()
            .unwrap()
            .query_row(
                "SELECT available AND availability_check IS NULL FROM locations",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(available);
}

#[test]
fn mixed_library_keeps_platforms_stats_and_catalog_work_separate() {
    use super::{GamePlatform, covers, metadata, scrape};
    use sha2::{Digest, Sha256};

    let mut f = Fixture::new();
    f.db.configure_import_scraping(Some(covers::Provider::Libretro));
    f.image("Disc.iso", 1);
    let cart_path = f.media.join("Disc.GbA");
    let mut rom = vec![0; 32 * 1024];
    rom[0xa0..0xa4].copy_from_slice(b"DEMO");
    rom[0xac..0xb0].copy_from_slice(b"ABCE");
    fs::write(&cart_path, &rom).unwrap();
    let scan = f.scan();
    assert_eq!((scan.added, scan.failed, scan.pending), (2, 0, 0));
    let games = f.db.games().unwrap();
    let disc = games
        .iter()
        .find(|g| g.platform_kind() == Some(GamePlatform::Ps1))
        .unwrap();
    let cart = games
        .iter()
        .find(|g| g.platform_kind() == Some(GamePlatform::Gba))
        .unwrap();
    assert_eq!(cart.title, "DEMO");
    assert_eq!(cart.serial.as_deref(), Some("ABCE"));
    assert_eq!(cart.region.as_deref(), Some("USA"));
    assert!(
        f.db.connect()
            .unwrap()
            .query_row(
                "SELECT strong_id IS NULL FROM discs WHERE home_game_id=? AND platform='GBA'",
                [cart.id],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert_eq!(scrape::due_games(&f.db).unwrap().len(), 2);
    scrape::enqueue_games(
        &f.db,
        &[cart.id],
        scrape::Mode::Full,
        covers::Provider::Libretro,
    )
    .unwrap();
    assert_eq!(scrape::due_games(&f.db).unwrap()[0].0, cart.id);
    assert!(
        metadata::search_candidates(&f.db, cart.id, "SCUS-94900")
            .unwrap()
            .is_empty()
    );
    let jobs = scrape::recent_jobs(&f.db).unwrap();
    assert!(
        jobs.iter()
            .any(|job| job.title == "DEMO" && job.provider == "libretro-gba")
    );
    assert!(
        f.db.confirm_copy(cart.locations[0].id, disc.locations[0].id)
            .is_err()
    );
    assert!(f.db.relocate(disc.locations[0].id, &cart_path).is_err());
    assert!(f.db.group_disc(cart.locations[0].disc_id, disc.id).is_err());
    assert_eq!(f.db.games().unwrap().len(), 2);

    // The usual launcher association binds the session to the existing cartridge.
    let sha = format!("{:x}", Sha256::digest(&rom));
    let home = f.db.associate_opened(&cart_path, &sha, false).unwrap();
    assert_eq!(home, cart.id);
    f.session(home, 120);
    metadata::set_override(&f.db, home, metadata::Field::Title, Some("My demo")).unwrap();
    let moved = f.media.join("Moved.gba");
    fs::rename(&cart_path, &moved).unwrap();
    f.db.relocate(cart.locations[0].id, &moved).unwrap();
    let reopened = Library::open(&f.root.path().join("index.sqlite3")).unwrap();
    let cart = reopened.games_by_id(&[home]).unwrap().remove(0);
    assert_eq!(cart.platform_kind(), Some(GamePlatform::Gba));
    assert_eq!(cart.locations[0].path, moved.canonicalize().unwrap());
    assert_eq!(cart.seconds, 120);
    assert_eq!(
        metadata::list(&reopened).unwrap()[&home].title.as_deref(),
        Some("My demo")
    );
}

#[test]
fn cartridge_sample_collisions_require_full_verification() {
    let f = Fixture::new();
    let first = f.media.join("A.gba");
    let second = f.media.join("B.gba");
    let mut bytes = vec![0; 128 * 1024];
    fs::write(&first, &bytes).unwrap();
    bytes[8192] = 1; // Outside the bounded scan pages.
    fs::write(&second, &bytes).unwrap();
    fs::write(f.media.join("truncated.gba"), [0; 16]).unwrap();
    let scan = f.scan();
    assert_eq!((scan.added, scan.pending, scan.failed), (2, 1, 1));
    let games = f.db.games().unwrap();
    assert_eq!(games[0].title, "A");
    assert!(
        f.db.confirm_copy(games[0].locations[0].id, games[1].locations[0].id)
            .is_err()
    );
    assert!(f.db.relocate(games[0].locations[0].id, &second).is_err());
}

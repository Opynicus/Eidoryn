use super::*;
use crate::desktop::{
    WorkspacePage,
    tests::{click_control, desktop, press_keys, wait_for_snapshot},
};
use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase, point, size,
};
use std::time::Instant;

fn bytes_read() -> Option<u64> {
    std::fs::read_to_string("/proc/self/io")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("rchar: ")?.parse().ok())
}

fn thumbnail_entries(root: &std::path::Path, games: i64) -> Vec<Entry> {
    let image = image::RgbImage::from_pixel(192, 192, image::Rgb([10, 20, 30]));
    let original = root.join("cover.png");
    image.save(&original).unwrap();
    (1..=games)
        .map(|id| {
            let mut entry = Entry::new(
                Game {
                    platform: "PS1".into(),
                    manual_position: None,
                    id,
                    title: format!("Disc {id:04}"),
                    serial: Some(format!("SLUS-{id:05}")),
                    region: None,
                    last_played: None,
                    seconds: 0,
                    locations: Vec::new(),
                },
                None,
            );
            let path = root.join(format!("thumb-{id}.png"));
            std::fs::copy(&original, &path).unwrap();
            entry.thumbnail = Some(path);
            entry
        })
        .collect()
}

fn scroll_by(cx: &mut gpui::VisualTestContext, position: gpui::Point<gpui::Pixels>, dy: f32) {
    cx.simulate_event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn library_grid_virtualizes_scrolling_and_switches_to_a_compact_list(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Library::open(&tmp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    let games: i64 = std::env::var("EIDORYN_BENCH_GAMES")
        .ok()
        .map_or(1000, |s| s.parse().unwrap());
    let entries = thumbnail_entries(tmp.path(), games);
    let before = bytes_read();
    let start = Instant::now();
    view.update(cx, |v, cx| {
        v.preferences_path = Some(tmp.path().join("preferences.json"));
        v.library = Workspace::new(Some(db));
        v.library.entries = entries;
        v.library.loaded = true;
        v.reorder_library();
        v.page = WorkspacePage::Library;
        cx.notify();
    });
    cx.run_until_parked();
    let first_paint = start.elapsed();
    let after_first = bytes_read();
    let first = view.read_with(cx, |v, _| v.library.rendered_cells.clone());
    let position = cx.debug_bounds("library-entry-1").unwrap().center();
    assert_eq!(first.start, 0);
    assert!((4..100).contains(&first.end), "first viewport {first:?}");
    let start = Instant::now();
    scroll_by(cx, position, -1500.);
    let scroll = start.elapsed();
    let after_scroll = bytes_read();
    let scrolled = view.read_with(cx, |v, _| v.library.rendered_cells.clone());
    assert!(
        scrolled.start > first.start && scrolled.len() < 100,
        "{scrolled:?}"
    );
    assert!(cx.debug_bounds("library-entry-1").is_none());
    let mut scroll_samples = Vec::new();
    for step in 0..20 {
        let start = Instant::now();
        scroll_by(cx, position, if step % 2 == 0 { 1000. } else { -1000. });
        scroll_samples.push(start.elapsed());
    }
    for _ in 0..4 {
        scroll_by(cx, position, -1200.);
    }
    assert_eq!(view.read_with(cx, |v, _| v.library.thumbnails.len()), 64);
    scroll_samples.sort_unstable();
    println!(
        "scroll_p50_us={} scroll_p95_us={}",
        scroll_samples[9].as_micros(),
        scroll_samples[18].as_micros()
    );
    click_control(cx, "lib-layout");
    assert!(view.read_with(cx, |v, _| v.preferences.library.list));
    let list = view.read_with(cx, |v, _| v.library.rendered_cells.clone());
    assert!((1..25).contains(&list.len()), "{list:?}");
    println!(
        "{games} games at 1280x900: grid {first:?} in {first_paint:?}, scroll {scrolled:?} in {scroll:?}, compact list {list:?}"
    );
    if let (Some(a), Some(b), Some(c)) = (before, after_first, after_scroll) {
        println!(
            "Process rchar bytes — first viewport {}, scrolled viewport {}",
            b - a,
            c - b
        );
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status")
        && let Some(peak) = status.lines().find(|s| s.starts_with("VmHWM:"))
    {
        println!("{peak}");
    }
    click_control(cx, "library-search");
    cx.simulate_input(&format!("SLUS-{games:05}"));
    assert_eq!(view.read_with(cx, |v, _| v.library.order.len()), 1);
    assert_eq!(
        view.read_with(cx, |v, _| v.library.entries[v.library.order[0]].game.id),
        games
    );
    measure_window_exit(view, cx);
}

fn measure_window_exit(view: Entity<Desktop>, cx: &mut gpui::VisualTestContext) {
    let start = Instant::now();
    view.update(cx, |v, cx| {
        v.command(crate::desktop::Command::RequestClose, cx);
    });
    wait_for_snapshot(&view, cx, |snapshot| snapshot.close_ready);
    drop(view);
    cx.run_until_parked();
    assert_eq!(cx.windows(), [] as [gpui::AnyWindowHandle; 0]);
    println!("window_exit_us={}", start.elapsed().as_micros());
}

#[gpui::test]
fn library_enter_requires_bios_then_navigates_only_after_success(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.iso");
    std::fs::write(&path, vec![0; 2048 * 32]).unwrap();
    let library = Library::open(&tmp.path().join("library.db")).unwrap();
    library
        .scan_files(std::slice::from_ref(&path), &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(tmp.path().join("preferences.json"));
        v.library = Workspace::new(Some(library));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-1");
    press_keys(cx, "enter");
    assert!(cx.debug_bounds("library-details").is_none());
    assert!(cx.debug_bounds("lib-bios-continue").is_some());
    assert_eq!(view.read_with(cx, |v, _| v.snapshot.media_revision), 0);
    click_control(cx, "lib-bios-continue");
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Library);
    let bios = tmp.path().join("firmware.bin");
    std::fs::write(&bios, vec![0; 512 * 1024]).unwrap();
    click_control(cx, "lib-bios-continue");
    cx.simulate_path_prompt_response(|_| Some(vec![bios]));
    cx.run_until_parked();
    wait_for_snapshot(&view, cx, |s| s.media_revision == 1);
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Game);
    cx.run_until_parked();

    click_control(cx, "nav-library");
    click_control(cx, "lib-start");
    assert!(!view.read_with(cx, |v, _| v.library.details));
    wait_for_snapshot(&view, cx, |s| s.media_revision == 2);
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Game);
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot.disc_path.clone()),
        Some(path.clone())
    );
    cx.run_until_parked();

    click_control(cx, "nav-library");
    std::fs::rename(&path, tmp.path().join("moved.iso")).unwrap();
    click_control(cx, "lib-start");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.multiple);
        None
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("library-details").is_none());
    assert_eq!(view.read_with(cx, |v, _| v.page), WorkspacePage::Library);
    assert_eq!(view.read_with(cx, |v, _| v.snapshot.media_revision), 2);
}

#[test]
fn verified_copies_share_a_disc_number_in_the_card_badge() {
    let entry = Entry::new(
        Game {
            platform: "PS1".into(),
            manual_position: None,
            id: 1,
            title: "Edition".into(),
            serial: None,
            region: None,
            seconds: 0,
            last_played: None,
            locations: [(1, 1), (2, 1), (3, 2)]
                .into_iter()
                .map(|(id, disc_number)| eidoryn_psx::library::Location {
                    id,
                    disc_id: id,
                    disc_number,
                    serial: None,
                    path: id.to_string().into(),
                    available: true,
                    fast_boot: false,
                })
                .collect(),
        },
        None,
    );
    assert_eq!(entry.disc_count(), 2);
}

#[gpui::test]
fn library_edit_title_survives_refresh_and_can_restore_automatic_title(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("Original.iso");
    std::fs::write(&path, vec![0; 2048 * 32]).unwrap();
    let db = Library::open(&tmp.path().join("library.db")).unwrap();
    db.scan_files(&[path], &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(tmp.path().join("preferences.json"));
        v.library = Workspace::new(Some(db));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-details");
    click_control(cx, "lib-edit-info");
    press_keys(cx, "ctrl-a");
    cx.simulate_input("My release");
    click_control(cx, "lib-save-edit");
    assert!(view.read_with(cx, |v, _| v.library.metadata_lookups.is_empty()));
    click_control(cx, "lib-refresh");
    assert!(view.read_with(cx, |v, _| matches!(
        v.library.metadata_lookups.get(&1),
        Some(MetadataLookup::Finished(Text::LibraryMetadataNoSerial))
    )));
    assert_eq!(
        view.read_with(cx, |v, _| v.library.entries[0].title.clone()),
        "My release"
    );
    click_control(cx, "lib-edit-info");
    click_control(cx, "lib-reset-edit");
    assert_eq!(
        view.read_with(cx, |v, _| v.library.entries[0].title.clone()),
        "Original"
    );
}

#[gpui::test]
fn cover_settings_persist_provider_and_keep_custom_template(cx: &mut TestAppContext) {
    use eidoryn_psx::{library::covers::Provider, preferences::Preferences};

    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("preferences.json");
    let db = Library::open(&tmp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(settings.clone());
        v.library = Workspace::new(Some(db));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "lib-settings");
    assert_eq!(
        view.read_with(cx, |v, _| v.preferences.library.cover_provider),
        Provider::Libretro,
    );
    click_control(cx, "lib-workers-up");
    assert_eq!(
        Preferences::load(&settings)
            .unwrap()
            .library
            .scrape_workers
            .get(),
        7
    );
    click_control(cx, "lib-workers-down");
    assert_eq!(
        Preferences::load(&settings)
            .unwrap()
            .library
            .scrape_workers
            .get(),
        6
    );
    click_control(cx, "lib-cover-workers-up");
    assert_eq!(
        Preferences::load(&settings)
            .unwrap()
            .library
            .cover_workers
            .get(),
        4
    );
    assert!(view.read_with(cx, |v, _| v.preferences.library.scrape_new_games));
    click_control(cx, "lib-scrape-new");
    assert!(
        !Preferences::load(&settings)
            .unwrap()
            .library
            .scrape_new_games
    );
    assert!(cx.debug_bounds("lib-template").is_none());
    click_control(cx, "lib-source-menu");
    click_control(cx, Text::LibraryCustomCoverSource.id());
    click_control(cx, "lib-template");
    press_keys(cx, "ctrl-a");
    let saved_template = Preferences::load(&settings).unwrap().library.cover_template;
    cx.simulate_input("http://example.com/cover");
    click_control(cx, "lib-save-edit");
    assert_eq!(
        Preferences::load(&settings).unwrap().library.cover_template,
        saved_template
    );
    click_control(cx, "lib-template");
    press_keys(cx, "ctrl-a");
    let template = "https://art.example/${serial}.png";
    cx.simulate_input(template);
    view.update(cx, Desktop::reload_library);
    cx.run_until_parked();
    click_control(cx, "lib-save-edit");
    for (label, provider) in [
        (Text::LibraryScreenScraper, Provider::ScreenScraper),
        (Text::LibraryIgdb, Provider::Igdb),
        (Text::LibraryLibretro, Provider::Libretro),
    ] {
        click_control(cx, "lib-source-menu");
        click_control(cx, label.id());
        let saved = Preferences::load(&settings).unwrap().library;
        assert_eq!(saved.cover_provider, provider);
        assert_eq!(saved.cover_template, template);
        assert!(!saved.auto_covers);
        assert!(cx.debug_bounds("lib-template").is_none());
    }
}

#[gpui::test]
fn library_folder_import_confirms_recursion_without_starting_game(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("games");
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("disc.iso"), vec![0; 2048 * 32]).unwrap();
    let library = Library::open(&tmp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(library.clone())));
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "lib-add-game");
    click_control(cx, "library-add-folder");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories);
        Some(vec![folder.clone()])
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v
            .library
            .pending_directory
            .as_ref()
            .map(|request| request.directories.clone())),
        Some(vec![folder.clone()])
    );
    assert_eq!(library.games().unwrap().len(), 0);
    click_control(cx, "lib-confirm-folder");
    for _ in 0..10 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(150));
        cx.run_until_parked();
        if view.read_with(cx, |v, _| v.library.scan.is_none()) {
            break;
        }
    }
    assert_eq!(view.read_with(cx, |v, _| v.library.entries.len()), 1);
    assert_eq!(library.roots().unwrap()[0].path, folder);
    assert!(
        view.read_with(cx, |v, _| v.preferences.recent_discs.is_empty()
            && v.snapshot.media_revision == 0)
    );
}

#[gpui::test]
fn scrape_refreshes_all_games_and_only_new_imports(cx: &mut TestAppContext) {
    use eidoryn_psx::library::scrape::Mode;
    use std::time::Duration;

    fn wait_for_batch(
        view: &Entity<Desktop>,
        cx: &mut gpui::VisualTestContext,
        mode: Mode,
        expected: usize,
    ) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            view.update(cx, Desktop::poll_library_scrape);
            cx.executor().advance_clock(Duration::from_millis(150));
            cx.run_until_parked();
            if let Some(outcome) = view.update(cx, |v, _| v.library.scraping.last.take()) {
                let (actual_mode, report, cancelled) = outcome;
                assert_eq!(actual_mode, mode);
                assert!(!cancelled);
                assert_eq!(
                    (report.total, report.finished, report.skipped, report.failed),
                    (expected, expected, expected, 0)
                );
                break;
            }
            assert!(Instant::now() < deadline, "no scrape report for {mode:?}");
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let db = Library::open(&tmp.path().join("library.db")).unwrap();
    let image = |name: &str, byte| {
        let path = tmp.path().join(name);
        std::fs::write(&path, vec![byte; 2048 * 8]).unwrap();
        path
    };
    let old = image("First.iso", 1u8);
    let second = image("Second.iso", 2);
    db.scan_files(&[old.clone(), second], &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let watched = view.update(cx, |v, _| {
        v.preferences_path = Some(tmp.path().join("preferences.json"));
        v.library = Workspace::new(Some(db));
        v.configure_imports();
        v.library.store.clone().unwrap()
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-search");
    cx.simulate_input("Second");
    assert_eq!(view.read_with(cx, |v, _| v.library.order.len()), 1);
    // Another library connection can commit a game before this view reloads.
    let external = Library::open(&tmp.path().join("library.db")).unwrap();
    external
        .scan_files(&[image("Third.iso", 4)], &AtomicBool::default(), |_| {})
        .unwrap();
    assert_eq!(view.read_with(cx, |v, _| v.library.entries.len()), 2);
    let refresh = cx.debug_bounds("lib-refresh-all").unwrap();
    assert_eq!(refresh.size, size(px(36.), px(36.)));
    let hint = "tooltip-刷新所有游戏的发行资料和封面";
    assert!(cx.debug_bounds(hint).is_none());
    cx.simulate_mouse_move(refresh.center(), None, Modifiers::none());
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.debug_bounds(hint).is_some());
    click_control(cx, "lib-refresh-all");
    wait_for_batch(&view, cx, Mode::Full, 3);

    // Import after leaving the filtered list, as when a game is opened directly.
    click_control(cx, "nav-game");
    let new = image("New.iso", 3);
    let files = [old, new];
    let scan = watched
        .scan_files(&files, &AtomicBool::default(), |_| {})
        .unwrap();
    assert_eq!((scan.added, scan.existing), (1, 1));
    wait_for_batch(&view, cx, Mode::Incremental, 1);

    let unchanged = watched
        .scan_files(&files, &AtomicBool::default(), |_| {})
        .unwrap();
    assert_eq!((unchanged.added, unchanged.existing), (0, 2));
    view.update(cx, Desktop::poll_library_scrape);
    cx.run_until_parked();
    view.update(cx, Desktop::poll_library_scrape);
    assert!(view.read_with(cx, |v, _| {
        !v.library.scraping.busy() && v.library.scraping.last.is_none()
    }));
}

#[gpui::test]
fn recovered_import_jobs_can_be_retried_after_reopening_library(cx: &mut TestAppContext) {
    use eidoryn_psx::library::{covers::Provider, scrape};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("library.db");
    let mut db = Library::open(&path).unwrap();
    db.configure_import_scraping(Some(Provider::Libretro));
    let image = tmp.path().join("Offline.iso");
    std::fs::write(&image, vec![11u8; 2048 * 8]).unwrap();
    db.scan_files(&[image], &AtomicBool::default(), |_| {})
        .unwrap();
    let ids = db.visible_game_ids().unwrap();
    assert_eq!(ids.len(), 1);
    let id = ids[0];
    // Reopen without delivering any of the previous process's in-memory notifications.
    drop(db);
    let db = Library::open(&path).unwrap();
    assert_eq!(
        scrape::due_games(&db).unwrap(),
        [(id, scrape::Mode::Incremental)]
    );
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(db.clone())));
    cx.simulate_resize(size(px(1280.), px(900.)));
    click_control(cx, "nav-library");
    cx.run_until_parked();
    assert_eq!(scrape::due_games(&db).unwrap(), []);
    view.update(cx, |v, _| {
        let (_, report, cancelled) = v.library.scraping.last.as_ref().unwrap();
        assert_eq!((report.finished, report.skipped, *cancelled), (1, 1, false));
    });
    // A terminal failure survives restart and the retry action is discoverable without a session report.
    scrape::enqueue_games(&db, &ids, scrape::Mode::Full, Provider::Libretro).unwrap();
    db.connect().unwrap().execute("UPDATE scrape_jobs SET status='failed', last_error_code='offline',last_error_message='Service unavailable' WHERE status='pending'",[]).unwrap();
    view.update(cx, |v, cx| {
        v.library = Workspace::new(Some(db.clone()));
        v.enter_library(cx);
    });
    cx.run_until_parked();
    click_control(cx, "lib-task-history");
    assert!(cx.debug_bounds("lib-retry-failed").is_some());
    click_control(cx, "lib-retry-failed");
    cx.run_until_parked();
    assert_eq!(scrape::failed_games(&db).unwrap(), [] as [i64; 0]);
    assert_eq!(view.read_with(cx, |v, _| v.library.entries.len()), 1);

    scrape::enqueue_games(&db, &ids, scrape::Mode::Full, Provider::Libretro).unwrap();
    db.connect().unwrap().execute(
        "UPDATE scrape_jobs SET status='retry_wait',next_attempt_at=unixepoch()+3600 WHERE status='pending'", []
    ).unwrap();
    view.update(cx, |v, cx| {
        v.library = Workspace::new(Some(db.clone()));
        v.enter_library(cx);
    });
    cx.run_until_parked();
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-details");
    click_control(cx, "lib-cancel-job");
    cx.run_until_parked();
    let cancelled: i64 = db
        .connect()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM scrape_jobs WHERE status='cancelled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cancelled, 2);
}

#[gpui::test]
fn first_window_paints_while_library_migration_waits_for_the_database_writer(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("library.sqlite");
    let mut locked = rusqlite::Connection::open(&file).unwrap();
    let lock = locked
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let store = Library::deferred(&file);
    let reader = store.clone();
    let (started, waiting) = std::sync::mpsc::channel();
    let initialization = std::thread::spawn(move || {
        started.send(()).unwrap();
        reader.wait_until_ready()
    });
    waiting.recv().unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = desktop(window, cx);
        view.library = Workspace::new(Some(store));
        view.page = WorkspacePage::Library;
        view
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("lib-add-game").is_some());
    assert!(!initialization.is_finished());
    lock.commit().unwrap();
    initialization.join().unwrap().unwrap();
    view.update(cx, |v, cx| {
        Desktop::start_library(v.library.store.clone().unwrap(), Vec::new(), cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.library.loaded));
}

#[gpui::test]
fn failed_deferred_open_reports_the_library_path_in_the_workspace(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let bad_path = temp.path().join("read-only-parent");
    std::fs::write(&bad_path, b"not a directory").unwrap();
    let path = bad_path.join("library.sqlite");
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, cx| {
        let store = Library::deferred(&path);
        v.library = Workspace::new(Some(store.clone()));
        Desktop::start_library(store, Vec::new(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.library.store.is_none());
        assert!(v.notice.as_ref().unwrap().contains(path.to_str().unwrap()));
    });
}

#[gpui::test]
fn library_sort_search_and_layout_keep_the_selected_game_visible(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let store = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(980.), px(680.)));
    let mut entries = thumbnail_entries(temp.path(), 3);
    entries[1].game.seconds = 1800;
    view.update(cx, |v, cx| {
        v.preferences_path = Some(temp.path().join("preferences.json"));
        v.library = Workspace::new(Some(store));
        v.library.entries = entries;
        v.library.loaded = true;
        v.reorder_library();
        v.page = WorkspacePage::Library;
        cx.notify();
    });
    cx.run_until_parked();
    click_control(cx, "lib-sort");
    click_control(cx, Text::LibraryTime.id());
    assert_eq!(view.read_with(cx, |v, _| v.library.order[0]), 1);
    click_control(cx, "library-entry-2");
    click_control(cx, "lib-layout");
    assert!(view.read_with(cx, |v, _| v.preferences.library.list
        && v.library.selected == Some(2)));
    click_control(cx, "lib-grid");
    click_control(cx, "library-search");
    cx.simulate_input("not in this collection");
    assert!(view.read_with(cx, |v, _| v.library.order.is_empty()
        && v.library.selected.is_none()));
    click_control(cx, "lib-clear-search");
    assert_eq!(view.read_with(cx, |v, _| v.library.order.len()), 3);
}

#[gpui::test]
fn filtered_drops_survive_concurrent_refresh_and_immediate_close(cx: &mut TestAppContext) {
    use eidoryn_psx::{library::Placement, preferences::Preferences};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let prefs = dir.path().join("preferences.json");
    let store = Library::open(&path).unwrap();
    for (i, name) in ["Zoo Set", "Other", "Alpha Set", "Beta Set"]
        .iter()
        .enumerate()
    {
        let disc = dir.path().join(format!("{name}.iso"));
        std::fs::write(&disc, vec![i as u8; 2048 * 32]).unwrap();
        store
            .scan_files(&[disc], &AtomicBool::default(), |_| {})
            .unwrap();
    }

    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(prefs.clone());
        v.library = Workspace::new(Some(store));
    });
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "library-search");
    cx.simulate_input("set");
    cx.run_until_parked();
    view.update(cx, |v, cx| {
        let ids = |w: &Workspace| {
            w.order
                .iter()
                .map(|&i| w.entries[i].game.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&v.library), [3, 4, 1]);
        v.reload_library(cx);
        v.move_library_game(4, 1, Placement::After, cx);
        assert_eq!(ids(&v.library), [3, 1, 4]);
        v.move_library_game(4, 1, Placement::Before, cx);
        assert_eq!(ids(&v.library), [3, 4, 1]);

        v.library.query.clear();
        v.reorder_library();
        assert_eq!(ids(&v.library), [3, 2, 4, 1]);
        v.command(crate::desktop::Command::RequestClose, cx);
    });
    wait_for_snapshot(&view, cx, |s| s.close_ready);
    cx.run_until_parked();
    assert_eq!(cx.windows(), [] as [gpui::AnyWindowHandle; 0]);
    assert_eq!(
        Preferences::load(&prefs).unwrap().library.sort,
        Sort::Manual
    );
    let mut games = Library::open(&path).unwrap().games().unwrap();
    games.sort_by_key(|g| g.manual_position);
    assert_eq!(
        games.into_iter().map(|g| g.id).collect::<Vec<_>>(),
        [3, 2, 4, 1]
    );
}

#[gpui::test]
fn library_add_dialog_traps_keyboard_and_dismisses_without_importing(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let store = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(store.clone())));
    click_control(cx, "nav-library");
    click_control(cx, "lib-add-game");
    assert!(cx.debug_bounds("library-add-images").is_some());
    for keys in ["tab tab tab tab", "shift-tab shift-tab", "f8"] {
        press_keys(cx, keys);
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.library_dialog_focus.scope.contains_focused(window, cx));
            assert!(!v.graphics.open);
        });
    }
    press_keys(cx, "escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("library-dialog").is_none());
    press_keys(cx, "enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("library-add-folder").is_some());
    click_control(cx, "lib-close-dialog");
    assert!(cx.debug_bounds("library-dialog").is_none());
    assert!(store.games().unwrap().is_empty());
}

#[gpui::test]
fn library_grid_fits_the_window_and_list_rows_match_the_header(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let store = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    let mut entries = thumbnail_entries(temp.path(), 12);
    // Measure both downloaded art and the typographic fallback.
    for entry in entries.iter_mut().step_by(2) {
        entry.thumbnail = None;
    }
    view.update(cx, |v, cx| {
        v.preferences_path = Some(temp.path().join("preferences.json"));
        v.library = Workspace::new(Some(store));
        v.library.entries = entries;
        v.library.loaded = true;
        v.reorder_library();
        v.page = WorkspacePage::Library;
        cx.notify();
    });
    for (width, last_tile) in [
        (1440., "library-entry-6"),
        (1280., "library-entry-5"),
        (980., "library-entry-4"),
    ] {
        cx.simulate_resize(size(px(width), px(900.)));
        cx.run_until_parked();
        let page = cx.debug_bounds("library-page").unwrap();
        let first = cx.debug_bounds("library-entry-1").unwrap();
        let last = cx.debug_bounds(last_tile).unwrap();
        assert_eq!(first.top(), last.top());
        assert!((last.right() - page.right()).abs() <= px(1.));
        for (cover, tags) in [
            ("library-art-1", "library-badges-1"),
            ("library-art-2", "library-badges-2"),
        ] {
            let art = cx.debug_bounds(cover).unwrap();
            let badges = cx.debug_bounds(tags).unwrap();
            assert!((art.size.width - art.size.height).abs() <= px(1.));
            assert!(badges.top() >= art.bottom() && badges.bottom() <= first.bottom());
        }
        click_control(cx, "lib-layout");
        let header = cx.debug_bounds("library-list-header").unwrap();
        let row = cx.debug_bounds("library-entry-2").unwrap();
        assert!((header.left() - row.left()).abs() <= px(1.));
        assert!((header.right() - row.right()).abs() <= px(1.));
        click_control(cx, "lib-grid");
    }
}

#[gpui::test]
fn dropping_a_single_image_on_the_library_imports_without_booting(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Dropped.iso");
    std::fs::write(&path, vec![0; 2048 * 32]).unwrap();
    let store = Library::open(&temp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(store.clone())));
    click_control(cx, "nav-library");
    cx.update(|window, cx| {
        let files = gpui::ExternalPaths([path.clone()].into_iter().collect());
        view.update(cx, |v, cx| v.drop_discs(&files, window, cx));
    });
    cx.run_until_parked();
    let games = store.games().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].locations[0].path, path);
    assert!(view.read_with(cx, |v, _| v.page == WorkspacePage::Library
        && v.snapshot.media_revision == 0
        && v.library.pending_bios.is_none()));
}

#[gpui::test]
fn library_details_reflow_and_removal_requires_confirmation(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Original.iso");
    std::fs::write(&path, vec![0; 2048 * 32]).unwrap();
    let db = Library::open(&temp.path().join("library.db")).unwrap();
    db.scan_files(std::slice::from_ref(&path), &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| v.library = Workspace::new(Some(db.clone())));
    cx.simulate_resize(size(px(1440.), px(1100.)));
    cx.run_until_parked();
    for control in ["nav-library", "library-entry-1", "lib-details"] {
        click_control(cx, control);
    }
    let edition = cx.debug_bounds("library-edition-panels").unwrap();
    let management = cx.debug_bounds("library-management-panels").unwrap();
    assert!(management.left() >= edition.right());
    click_control(cx, "lib-remove");
    assert!(cx.debug_bounds("lib-confirm").is_some());
    press_keys(cx, "escape");
    assert!(cx.debug_bounds("lib-confirm").is_none());
    assert_eq!(db.games().unwrap().len(), 1);
    cx.simulate_resize(size(px(980.), px(900.)));
    cx.run_until_parked();
    let edition = cx.debug_bounds("library-edition-panels").unwrap();
    let management = cx.debug_bounds("library-management-panels").unwrap();
    assert_eq!(edition.left(), management.left());
    assert!(management.top() >= edition.bottom());
    cx.simulate_resize(size(px(1440.), px(1100.)));
    cx.run_until_parked();
    click_control(cx, "lib-remove");
    click_control(cx, "lib-confirm");
    assert!(db.games().unwrap().is_empty());
    assert!(path.is_file());
    assert!(cx.debug_bounds("library-details").is_none());
}

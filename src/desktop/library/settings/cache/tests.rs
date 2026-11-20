use super::{CacheClearState, CacheScope, OverviewMenu};
use crate::desktop::{
    library::Workspace,
    tests::{click_control, desktop, press_keys},
};
use eidoryn_psx::{
    library::{Library, covers},
    preferences::Preferences,
};
use gpui::{TestAppContext, px, size};
use std::sync::atomic::AtomicBool;

#[gpui::test]
fn cache_controls_preserve_preferences_and_confirm_the_selected_scope(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let preferences = dir.path().join("preferences.json");
    let lib = Library::open(&dir.path().join("library.db")).unwrap();
    let discs = ["Keep.iso", "Removed.iso"].map(|name| dir.path().join(name));
    for (i, path) in discs.iter().enumerate() {
        std::fs::write(path, vec![i as u8; 2048 * 32]).unwrap();
    }
    lib.scan_files(&discs, &AtomicBool::default(), |_| {})
        .unwrap();
    let connection = lib.connect().unwrap();
    connection
        .execute("UPDATE game_metadata SET auto_title='Cached title'", [])
        .unwrap();
    lib.remove(2).unwrap();
    let cached_ids = || -> Vec<i64> {
        connection
            .prepare(
                "SELECT game_id FROM game_metadata WHERE auto_title IS NOT NULL ORDER BY game_id",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(cached_ids(), [1, 2]);
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(preferences.clone());
        v.library = Workspace::new(Some(lib));
    });
    cx.simulate_resize(size(px(1200.), px(1300.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    click_control(cx, "lib-settings");
    assert!(view.read_with(cx, |v, _| !v.preferences.library.clear_metadata_on_remove));
    click_control(cx, "lib-clear-metadata-on-remove");
    assert!(
        Preferences::load(&preferences)
            .unwrap()
            .library
            .clear_metadata_on_remove
    );
    press_keys(cx, "space");
    cx.run_until_parked();
    assert!(
        !Preferences::load(&preferences)
            .unwrap()
            .library
            .clear_metadata_on_remove
    );
    assert_eq!(cached_ids(), [1, 2]);

    press_keys(cx, "tab enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(
        v.library.menu,
        Some(OverviewMenu::ClearMetadata(CacheScope::All))
    )));
    click_control(cx, "lib-cache-scope-removed");
    press_keys(cx, "tab shift-tab f8");
    cx.update(|w, cx| {
        assert!(
            view.read(cx)
                .library_dialog_focus
                .scope
                .contains_focused(w, cx)
        );
        assert!(!view.read(cx).graphics.open);
    });
    press_keys(cx, "escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("lib-cache-confirm").is_none());
    assert_eq!(cached_ids(), [1, 2]);

    click_control(cx, "lib-clear-metadata");
    click_control(cx, "lib-cache-scope-removed");
    click_control(cx, "lib-cache-confirm");
    assert_eq!(cached_ids(), [1]);
    assert!(cx.debug_bounds("lib-cache-clear-status").is_some());

    connection
        .execute_batch(
            "CREATE TRIGGER prevent_clear BEFORE UPDATE ON game_metadata
             BEGIN SELECT RAISE(ABORT, 'example database failure'); END;",
        )
        .unwrap();
    click_control(cx, "lib-clear-metadata");
    click_control(cx, "lib-cache-confirm");
    assert!(view.read_with(cx, |v, _| matches!(
        &v.library.cache_clear, CacheClearState::Failed(message) if message.contains("example database failure")
    )));
    assert!(cx.debug_bounds("lib-cache-clear-status").is_some());
    assert_eq!(cached_ids(), [1]);
    connection
        .execute_batch("DROP TRIGGER prevent_clear")
        .unwrap();
    click_control(cx, "lib-clear-metadata");
    click_control(cx, "lib-cache-confirm");
    assert_eq!(cached_ids(), [] as [i64; 0]);
    assert!(view.read_with(cx, |v, _| v.library.cache_clear
        == CacheClearState::Finished));
}

#[gpui::test]
fn clearing_the_default_scope_keeps_cards_empty_after_restart(app: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("library.db");
    let discs = ["Local disc.iso", "Removed.iso"].map(|name| dir.path().join(name));
    for (i, disc) in discs.iter().enumerate() {
        std::fs::write(disc, vec![i as u8; 2048 * 32]).unwrap();
    }
    let lib = Library::open(&db_path).unwrap();
    lib.scan_files(&discs, &AtomicBool::default(), |_| {})
        .unwrap();
    let png = dir.path().join("box-art.png");
    image::RgbImage::from_pixel(32, 32, image::Rgb([40, 80, 120]))
        .save(&png)
        .unwrap();
    covers::set_local(&lib, 1, &png).unwrap();
    lib.connect()
        .unwrap()
        .execute_batch(
            "UPDATE game_metadata SET auto_title='Scraped title', auto_date='1998',
                disc_serial='SLUS-12345', release_serial='SLUS-12345', release_id='sample';
             UPDATE cover_assets SET kind='download', source='Libretro', serial='SLUS-12345';",
        )
        .unwrap();
    lib.remove(2).unwrap();
    let (view, cx) = app.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences.library.auto_covers = true;
        v.library = Workspace::new(Some(lib));
    });
    cx.simulate_resize(size(px(1200.), px(1300.)));
    click_control(cx, "nav-library");
    view.read_with(cx, |v, _| {
        let card = &v.library.entries[0];
        assert_eq!(card.title, "Scraped title");
        assert_eq!(card.date.as_deref(), Some("1998"));
        assert!(card.thumbnail.as_ref().is_some_and(|p| p.is_file()));
    });
    assert!(cx.debug_bounds("library-placeholder-1").is_none());
    click_control(cx, "lib-settings");
    click_control(cx, "lib-clear-metadata");
    click_control(cx, "lib-cache-scope-removed");
    click_control(cx, "lib-cache-confirm");
    click_control(cx, "lib-settings-back");
    view.read_with(cx, |v, _| {
        let card = &v.library.entries[0];
        assert_eq!(card.title, "Scraped title");
        assert!(card.thumbnail.as_ref().is_some_and(|p| p.is_file()));
    });
    click_control(cx, "lib-settings");
    click_control(cx, "lib-clear-metadata");
    click_control(cx, "lib-cache-confirm");
    click_control(cx, "lib-settings-back");
    view.read_with(cx, |v, _| {
        let card = &v.library.entries[0];
        assert!(
            card.thumbnail.is_none(),
            "clearing the library left its downloaded cover visible"
        );
        assert_eq!(card.title, "Local disc");
    });
    assert!(cx.debug_bounds("library-placeholder-1").is_some());

    cx.update(|window, _| window.remove_window());
    drop(view);
    let (reopened, cx) = app.add_window_view(desktop);
    reopened.update(cx, |v, _| {
        v.preferences.library.auto_covers = true;
        v.library = Workspace::new(Some(Library::open(&db_path).unwrap()));
    });
    click_control(cx, "nav-library");
    assert!(cx.debug_bounds("library-placeholder-1").is_some());
    reopened.read_with(cx, |v, _| {
        let card = &v.library.entries[0];
        assert_eq!(card.title, "Local disc");
        assert!(card.thumbnail.is_none());
        assert!(card.date.is_none());
        assert!(card.release_title.is_none());
        assert!(v.library.cover.is_none());
        assert!(v.library.auto_attempted.is_empty());
    });
}

#[gpui::test]
fn whole_library_clear_orders_pending_refreshes_and_later_retries(cx: &mut TestAppContext) {
    use crate::desktop::library::tasks::CoverTask;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.db")).unwrap();
    // More than one enqueue transaction (64 games each).
    let paths = (1..=65)
        .map(|id| {
            let path = dir.path().join(format!("Game {id:02}.iso"));
            std::fs::write(&path, vec![id; 2048 * 8]).unwrap();
            path
        })
        .collect::<Vec<_>>();
    lib.scan_files(&paths, &AtomicBool::default(), |_| {})
        .unwrap();
    let db = lib.connect().unwrap();
    let job_counts = || -> (usize, usize, Option<i64>) {
        db.query_row(
            "SELECT count(*) FILTER (WHERE status='cancelled'),
                    count(*) FILTER (WHERE status='pending'),
                    max(game_id) FILTER (WHERE status='pending')
             FROM scrape_jobs WHERE job_type='metadata'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let (view, cx) = cx.add_window_view(desktop);
    let (release, wait) = tokio::sync::oneshot::channel();
    let waiting_lookup = Arc::new(AtomicBool::new(false));
    let waiting_search = Arc::new(AtomicBool::new(false));
    view.update(cx, |v, cx| {
        v.library = Workspace::new(Some(lib));
        // The normal scheduler waits for an in-flight chosen cover. Inspect the
        // committed queue before any worker consumes jobs.
        v.library.cover = Some(CoverTask {
            cancel: Arc::default(),
            chosen_edition: true,
        });
        v.library.scraping.queue_write = Some(cx.spawn(async move |_, _| wait.await.unwrap()));
        v.library.match_cancel = Arc::clone(&waiting_search);
        v.library.metadata_lookups.insert(
            1,
            crate::desktop::library::MetadataLookup::Fetching(Arc::clone(&waiting_lookup)),
        );
        v.refresh_library_data(cx);
        v.retry_game(1, cx);
        v.clear_library_metadata_cache(CacheScope::All, cx);
        v.retry_game(65, cx);
    });
    cx.run_until_parked();
    assert_eq!(job_counts(), (0, 0, None));
    assert!(waiting_lookup.load(std::sync::atomic::Ordering::Relaxed));
    assert!(waiting_search.load(std::sync::atomic::Ordering::Relaxed));
    assert!(view.read_with(cx, |v, _| v.library.cache_clear == CacheClearState::Running));
    release.send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(job_counts(), (65, 1, Some(65)));
    assert!(view.read_with(cx, |v, _| v.library.cache_clear
        == CacheClearState::Finished));
}

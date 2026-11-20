use super::*;
use crate::desktop::tests::{click_control, desktop, press_keys};
use gpui::{
    Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext, TouchPhase,
    VisualTestContext, point, size,
};
use tempfile::TempDir;

fn collection(
    cx: &mut TestAppContext,
    count: u8,
) -> (TempDir, Entity<Desktop>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(&dir.path().join("library.db")).unwrap();
    let files = (1..=count)
        .map(|id| {
            let path = dir.path().join(format!("Game-{id:03}.iso"));
            std::fs::write(&path, vec![id; 2048 * 32]).unwrap();
            path
        })
        .collect::<Vec<_>>();
    db.scan_files(&files, &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |view, _| {
        view.library = Workspace::new(Some(db));
        view.preferences_path = Some(dir.path().join("preferences.json"));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    (dir, view, cx)
}

fn right_click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
}

fn cleared_collection(
    app: &mut TestAppContext,
    metadata_status: http::StatusCode,
) -> (TempDir, Entity<Desktop>, &mut VisualTestContext) {
    use std::io::Cursor;
    let (dir, view, cx) = collection(app, 2);
    let lib = view.read_with(cx, |v, _| v.library.store.clone().unwrap());
    lib.connect()
        .unwrap()
        .execute_batch(
            "UPDATE game_metadata SET disc_serial=printf('SLUS-%05d', game_id), disc_region='USA';",
        )
        .unwrap();
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    let rows = br#"{"results":{"bindings":[{"game":{"value":"https://www.wikidata.org/entity/Q1"},"title":{"value":"Recorded release"},"date":{"value":"+1998-05-01T00:00:00Z"},"precision":{"value":"11"}}]}}"#;
    let mut png = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(32, 32, image::Rgb([20, 60, 100]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let lib = lib.with_scrape_responses(
        http::Response::builder()
            .status(metadata_status)
            .body(rows.to_vec())
            .unwrap(),
        http::Response::new(png.into_inner()),
    );
    view.update(cx, |v, cx| {
        v.preferences.library.auto_covers = false;
        v.preferences.library.scrape_new_games = false;
        v.library = Workspace::new(Some(lib));
        v.reload_library(cx);
    });
    cx.run_until_parked();
    for selector in ["library-placeholder-1", "library-placeholder-2"] {
        assert!(cx.debug_bounds(selector).is_some());
    }
    (dir, view, cx)
}

fn assert_card_shows_cover(view: &Entity<Desktop>, cx: &mut VisualTestContext, id: i64) {
    let path = view.read_with(cx, |v, _| {
        assert!(!v.preferences.library.auto_covers);
        let card = v
            .library
            .entries
            .iter()
            .find(|entry| entry.game.id == id)
            .unwrap();
        let path = card
            .thumbnail
            .clone()
            .expect("scraping should publish the thumbnail");
        assert!(!card.cache_cleared);
        assert_eq!(
            card.serial.as_deref(),
            Some(format!("SLUS-{id:05}").as_str())
        );
        assert!(v.library.thumbnails.iter().any(|(key, _)| key == &path));
        path
    });
    assert_eq!(
        image::open(path).unwrap().to_rgb8().get_pixel(0, 0).0,
        [20, 60, 100]
    );
}

#[gpui::test]
fn card_and_details_scrapes_restore_only_the_selected_cover(app: &mut TestAppContext) {
    let (_dir, view, cx) = cleared_collection(app, http::StatusCode::OK);
    right_click(cx, "library-entry-2");
    click_control(cx, "lib-card-scrape");
    assert_card_shows_cover(&view, cx, 2);
    assert!(cx.debug_bounds("library-placeholder-1").is_some());
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-details");
    click_control(cx, "lib-refresh");
    click_control(cx, "lib-back");
    assert_card_shows_cover(&view, cx, 1);
    view.read_with(cx, |v, _| {
        assert!(
            v.library
                .entries
                .iter()
                .all(|e| e.title == "Recorded release")
        );
    });
}

#[gpui::test]
fn explicit_refreshes_show_covers_even_when_release_lookups_fail(app: &mut TestAppContext) {
    let (_dir, view, cx) = cleared_collection(app, http::StatusCode::SERVICE_UNAVAILABLE);
    right_click(cx, "library-entry-1");
    click_control(cx, "lib-card-scrape");
    assert_card_shows_cover(&view, cx, 1);
    assert!(cx.debug_bounds("library-placeholder-2").is_some());
    click_control(cx, "lib-refresh-all");
    for id in [1, 2] {
        assert_card_shows_cover(&view, cx, id);
    }
    view.read_with(cx, |v, _| {
        assert!(v.library.entries.iter().all(|e| e.release_title.is_none()));
    });
}

#[gpui::test]
fn choosing_an_edition_restores_its_cover_with_automatic_art_disabled(app: &mut TestAppContext) {
    let (_dir, view, cx) = cleared_collection(app, http::StatusCode::OK);
    let store = view.read_with(cx, |v, _| v.library.store.clone().unwrap());
    store
        .connect()
        .unwrap()
        .execute_batch(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date) VALUES
            ('Q1/SLUS-00001','SLUS-00001','USA','First edition','1998','',''),
            ('Q2/SLUS-00001','SLUS-00001','USA','Second edition','2000','','');",
        )
        .unwrap();
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-details");
    click_control(cx, "lib-match");
    click_control(cx, "lib-match-1");
    click_control(cx, "lib-back");
    assert_card_shows_cover(&view, cx, 1);
    view.read_with(cx, |v, _| {
        let card = v.library.entries.iter().find(|e| e.game.id == 1).unwrap();
        assert_eq!(card.title, "Second edition");
    });
}

#[gpui::test]
fn context_actions_target_the_right_clicked_game(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 3);
    right_click(cx, "library-entry-1");
    right_click(cx, "library-entry-3");
    assert!(cx.debug_bounds("lib-card-menu").is_some());
    click_control(cx, "lib-card-scrape");
    assert!(cx.debug_bounds("lib-card-menu").is_none());
    assert!(cx.debug_bounds("lib-card-metadata-status").is_some());
    view.read_with(cx, |v, _| {
        assert_eq!(v.library.selected, Some(3));
        assert_eq!(v.library.metadata_lookups.len(), 1);
        assert!(matches!(
            v.library.metadata_lookups.get(&3),
            Some(MetadataLookup::Finished(Text::LibraryMetadataNoSerial))
        ));
        assert!(!v.library.details);
    });

    right_click(cx, "library-entry-1");
    click_control(cx, "lib-card-details");
    assert!(cx.debug_bounds("library-details").is_some());
    assert_eq!(view.read_with(cx, |v, _| v.library.selected), Some(1));
    click_control(cx, "lib-back");
    click_control(cx, "lib-layout");
    right_click(cx, "library-entry-2");
    click_control(cx, "lib-card-remove");
    assert!(view.read_with(cx, |v, _| matches!(
        v.library.menu,
        Some(OverviewMenu::Remove(_))
    )));
    press_keys(cx, "escape");
    assert!(view.read_with(cx, |v, _| v.library.menu.is_none()
        && v.library.entries.len() == 3));
    right_click(cx, "library-entry-1");
    press_keys(cx, "escape");
    assert!(cx.debug_bounds("lib-card-menu").is_none());
    right_click(cx, "library-entry-3");
    click_control(cx, "library-entry-1");
    assert!(cx.debug_bounds("lib-card-menu").is_none());
    assert_eq!(view.read_with(cx, |v, _| v.library.selected), Some(1));
}

#[gpui::test]
fn card_menu_removal_in_batch_mode_only_removes_its_target(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 3);
    click_control(cx, "lib-check-1");
    right_click(cx, "library-entry-2");
    click_control(cx, "lib-card-remove");
    click_control(cx, "lib-confirm-removal");
    view.read_with(cx, |v, _| {
        let checked = &v.library.checked_games;
        assert_eq!(checked.iter().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(v.library.order.len(), 2);
        assert!(v.library.entries.iter().all(|entry| entry.game.id != 2));
    });
}

#[gpui::test]
fn double_click_launches_only_when_the_first_click_was_browsing(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 3);
    let double_click = |cx: &mut VisualTestContext| {
        let position = cx.debug_bounds("library-entry-1").unwrap().center();
        let button = MouseButton::Left;
        let modifiers = Modifiers::none();
        cx.simulate_click(position, modifiers);
        cx.run_until_parked();
        cx.simulate_event(gpui::MouseDownEvent {
            position,
            button,
            modifiers,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            position,
            button,
            modifiers,
            click_count: 2,
        });
        cx.run_until_parked();
    };
    click_control(cx, "library-entry-1");
    click_control(cx, "lib-check-1");
    double_click(cx);
    view.read_with(cx, |v, _| {
        assert!(v.library.checked_games.is_empty());
        assert_eq!(v.library.launch_generation, 0);
    });
    double_click(cx);
    view.read_with(cx, |v, _| assert_eq!(v.library.launch_generation, 1));
}

#[gpui::test]
fn keyboard_context_menu_moves_focus_within_the_actions(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 3);
    click_control(cx, "library-entry-2");
    press_keys(cx, "shift-f10 escape shift-f10");
    assert!(cx.debug_bounds("lib-card-menu").is_some());
    press_keys(cx, "shift-tab");
    assert!(view.read_with(cx, |v, _| {
        v.library.card_menu.as_ref().is_some_and(|m| m.active == 2)
    }));
    press_keys(cx, "up enter");
    assert!(cx.debug_bounds("library-details").is_some());
    assert_eq!(view.read_with(cx, |v, _| v.library.selected), Some(2));
}

fn drag(
    cx: &mut VisualTestContext,
    source: &'static str,
    target: &'static str,
    offset: Point<Pixels>,
) {
    let start = cx.debug_bounds(source).unwrap().center();
    let end = cx.debug_bounds(target).unwrap().center() + offset;
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        start + point(px(12.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn drag_orders_grid_and_list_by_the_drop_edge_without_launching(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 3);
    let order = |cx: &VisualTestContext| {
        view.read_with(cx, |v, _| {
            assert!(v.library.pending_bios.is_none() && !v.library.details);
            v.library
                .order
                .iter()
                .map(|&i| v.library.entries[i].game.id)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(order(cx), [1, 2, 3]);
    drag(cx, "lib-check-1", "library-entry-3", point(px(15.), px(0.)));
    assert_eq!(order(cx), [1, 2, 3]);
    assert!(cx.debug_bounds("lib-batch-bar").is_none());
    drag(
        cx,
        "library-entry-1",
        "library-entry-3",
        point(px(15.), px(0.)),
    );
    assert_eq!(order(cx), [2, 3, 1]);
    assert!(view.read_with(cx, |v, _| v.preferences.library.sort == Sort::Manual));
    click_control(cx, "lib-layout");
    drag(
        cx,
        "library-entry-1",
        "library-entry-2",
        point(px(0.), px(-12.)),
    );
    assert_eq!(order(cx), [1, 2, 3]);
}

#[gpui::test]
fn wheel_scroll_during_drag_reorders_across_virtualized_rows(cx: &mut TestAppContext) {
    let (_dir, view, cx) = collection(cx, 60);
    let start = cx.debug_bounds("library-entry-1").unwrap().center();
    let pointer = start + point(px(12.), px(0.));
    assert!(cx.debug_bounds("library-entry-60").is_none());

    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(pointer, MouseButton::Left, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: pointer,
        delta: ScrollDelta::Pixels(point(px(0.), px(-5000.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    assert!(cx.debug_bounds("library-entry-1").is_none());
    let last = cx.debug_bounds("library-entry-60").unwrap();
    let after_last = last.center() + point(px(15.), px(0.));
    cx.simulate_mouse_move(after_last, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(after_last, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    let order = view.read_with(cx, |v, _| {
        assert!(v.library.pending_bios.is_none());
        v.library
            .order
            .iter()
            .map(|&i| v.library.entries[i].game.id)
            .collect::<Vec<_>>()
    });
    assert_eq!(order, (2..=60).chain([1]).collect::<Vec<_>>());
}

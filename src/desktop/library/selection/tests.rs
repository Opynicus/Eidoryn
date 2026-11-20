use super::{Desktop, Workspace};
use crate::desktop::tests::{click_control, desktop, press_keys};
use eidoryn_psx::library::Library;
use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
use std::{path::Path, sync::atomic::AtomicBool};

fn open_library<'a>(
    root: &Path,
    cx: &'a mut TestAppContext,
) -> (Library, Entity<Desktop>, &'a mut VisualTestContext) {
    let db = Library::open(&root.join("library.db")).unwrap();
    let paths = ["Alpha A.iso", "Alpha B.iso", "Beta.iso"].map(|name| root.join(name));
    for (i, path) in paths.iter().enumerate() {
        std::fs::write(path, vec![i as u8; 2048 * 32]).unwrap();
    }
    db.scan_files(&paths, &AtomicBool::default(), |_| {})
        .unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = Some(root.join("preferences.json"));
        v.library = Workspace::new(Some(db.clone()));
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    click_control(cx, "nav-library");
    (db, view, cx)
}

fn checked(view: &Entity<Desktop>, cx: &VisualTestContext) -> Vec<i64> {
    view.read_with(cx, |v, _| v.library.checked_games.iter().copied().collect())
}

#[gpui::test]
fn grid_and_list_share_checks_and_only_remove_confirmed_search_results(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (db, view, cx) = open_library(root.path(), cx);
    assert!(cx.debug_bounds("lib-batch-bar").is_none());
    let art = cx.debug_bounds("library-art-1").unwrap();
    let checkbox = cx.debug_bounds("lib-check-1").unwrap();
    assert!(checkbox.top() >= art.top() && checkbox.top() < art.center().y);
    assert!(checkbox.right() <= art.right() && checkbox.left() > art.center().x);
    click_control(cx, "lib-check-1");
    assert_eq!(checked(&view, cx), [1]);
    assert!(cx.debug_bounds("lib-batch-bar").is_some());
    assert!(view.read_with(cx, |v, _| v.library.selected.is_none()));
    click_control(cx, "library-entry-3");
    assert_eq!(checked(&view, cx), [1, 3]);
    click_control(cx, "library-search");
    cx.simulate_input("Alpha");
    cx.run_until_parked();
    assert_eq!(checked(&view, cx), [1]);
    view.update(cx, Desktop::reload_library);
    cx.run_until_parked();
    assert_eq!(checked(&view, cx), [1]);
    click_control(cx, "lib-check-all");
    assert_eq!(checked(&view, cx), [1, 2]);
    click_control(cx, "lib-layout");
    click_control(cx, "lib-check-2");
    assert_eq!(checked(&view, cx), [1]);
    press_keys(cx, "enter");
    cx.run_until_parked();
    assert_eq!(checked(&view, cx), [1, 2]);
    click_control(cx, "lib-uncheck-all");
    assert_eq!(checked(&view, cx), [] as [i64; 0]);
    assert!(cx.debug_bounds("lib-batch-bar").is_none());
    assert!(cx.debug_bounds("lib-remove-checked").is_none());
    click_control(cx, "lib-check-2");
    press_keys(cx, "escape");
    assert_eq!(checked(&view, cx), [] as [i64; 0]);
    click_control(cx, "lib-check-1");
    click_control(cx, "lib-check-all");
    click_control(cx, "lib-remove-checked");
    click_control(cx, "lib-cancel-removal");
    assert_eq!(db.games().unwrap().len(), 3);
    assert_eq!(checked(&view, cx), [1, 2]);
    click_control(cx, "lib-remove-checked");
    click_control(cx, "lib-confirm-removal");
    let remaining = db.games().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].title, "Beta");
    assert!(root.path().join("Alpha A.iso").is_file());
    assert!(root.path().join("Alpha B.iso").is_file());
    assert!(view.read_with(cx, |v, _| v.library.checked_games.is_empty()
        && v.library.pending_bios.is_none()
        && v.snapshot.media_revision == 0));
}

#[gpui::test]
fn deselecting_or_filtering_out_the_last_check_restores_browsing(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (_db, view, cx) = open_library(root.path(), cx);
    click_control(cx, "lib-check-1");
    click_control(cx, "lib-check-1");
    assert!(cx.debug_bounds("lib-batch-bar").is_none());
    click_control(cx, "library-entry-2");
    assert!(view.read_with(cx, |v, _| v.library.selected == Some(2)));
    click_control(cx, "lib-check-1");
    click_control(cx, "library-search");
    cx.simulate_input("Beta");
    cx.run_until_parked();
    assert!(cx.debug_bounds("lib-batch-bar").is_none());
    assert!(cx.debug_bounds("lib-check-3").is_some());
    assert_eq!(checked(&view, cx), [] as [i64; 0]);
}

#[gpui::test]
fn failed_batch_removal_keeps_checks_and_can_be_retried(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (db, view, cx) = open_library(root.path(), cx);
    let connection = db.connect().unwrap();
    view.update(cx, |v, _| {
        v.preferences.library.clear_metadata_on_remove = true;
    });
    connection
        .execute("UPDATE game_metadata SET auto_date='1998'", [])
        .unwrap();
    let cached = || {
        connection
            .query_row("SELECT COUNT(auto_date) FROM game_metadata", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    connection
        .execute_batch(
            "CREATE TRIGGER fail_removal BEFORE UPDATE OF hidden ON games
         WHEN NEW.id = 2 BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
        )
        .unwrap();
    click_control(cx, "lib-check-1");
    click_control(cx, "lib-check-all");
    click_control(cx, "lib-remove-checked");
    press_keys(cx, "escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("lib-confirm-removal").is_none());
    click_control(cx, "lib-remove-checked");
    click_control(cx, "lib-confirm-removal");
    assert!(cx.debug_bounds("lib-removal-error").is_some());
    assert_eq!(db.games().unwrap().len(), 3);
    assert_eq!(cached(), 3);
    assert_eq!(checked(&view, cx), [1, 2, 3]);
    connection
        .execute_batch("DROP TRIGGER fail_removal")
        .unwrap();
    click_control(cx, "lib-confirm-removal");
    assert!(db.games().unwrap().is_empty());
    assert_eq!(cached(), 0);
    assert!(view.read_with(cx, |v, _| v.library.checked_games.is_empty()));
}

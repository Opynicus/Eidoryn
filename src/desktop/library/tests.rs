use super::*;
use crate::desktop::{
    WorkspacePage,
    tests::{click_control, desktop, press_keys},
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

#[gpui::test]
fn library_grid_virtualizes_scrolling_and_switches_to_a_compact_list(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Library::open(&tmp.path().join("library.db")).unwrap();
    let (view, cx) = cx.add_window_view(desktop);
    cx.simulate_resize(size(px(1280.), px(900.)));
    cx.run_until_parked();
    let image = image::RgbImage::from_pixel(192, 192, image::Rgb([10, 20, 30]));
    let original = tmp.path().join("cover.png");
    image.save(&original).unwrap();
    let entries = (1..=1000)
        .map(|id| {
            let mut entry = Entry::new(
                Game {
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
            let path = tmp.path().join(format!("thumb-{id}.png"));
            std::fs::copy(&original, &path).unwrap();
            entry.thumbnail = Some(path);
            entry
        })
        .collect();
    let before = bytes_read();
    let start = Instant::now();
    view.update(cx, |v, cx| {
        v.preferences_path = Some(tmp.path().join("preferences.json"));
        v.library = Workspace::new(Some(db));
        v.library.entries = entries;
        v.library.loaded = true;
        v.library.reorder();
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
    cx.simulate_event(ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-1500.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    let scroll = start.elapsed();
    let after_scroll = bytes_read();
    let scrolled = view.read_with(cx, |v, _| v.library.rendered_cells.clone());
    assert!(
        scrolled.start > first.start && scrolled.len() < 100,
        "{scrolled:?}"
    );
    assert!(cx.debug_bounds("library-entry-1").is_none());
    click_control(cx, "lib-layout");
    assert!(view.read_with(cx, |v, _| v.preferences.library.list));
    let list = view.read_with(cx, |v, _| v.library.rendered_cells.clone());
    assert!((1..25).contains(&list.len()), "{list:?}");
    println!(
        "1,000 games at 1280x900: grid {first:?} in {first_paint:?}, scroll {scrolled:?} in {scroll:?}, compact list {list:?}"
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
    cx.simulate_input("SLUS-01000");
    assert_eq!(view.read_with(cx, |v, _| v.library.order.len()), 1);
    assert!(cx.debug_bounds("library-entry-1000").is_some());
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
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        view.update(cx, Desktop::receive_snapshot);
        if view.read_with(cx, |v, _| v.page) == WorkspacePage::Game {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "library launch receipt timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        view.read_with(cx, |v, _| v.snapshot.disc_path.clone()),
        Some(path)
    );
}

#[test]
fn verified_copies_share_a_disc_number_in_the_card_badge() {
    let entry = Entry::new(
        Game {
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
    click_control(cx, Text::LibraryEditTitle.id());
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
    click_control(cx, Text::LibraryEditTitle.id());
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
    assert!(cx.debug_bounds("lib-template").is_none());
    click_control(cx, Text::LibraryCustomCoverSource.id());
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
    click_control(cx, "library-add-folder");
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories);
        Some(vec![folder.clone()])
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.library.pending_directory.clone()),
        Some(folder.clone())
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

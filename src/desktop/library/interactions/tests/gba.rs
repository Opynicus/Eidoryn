//! Exercise the same library controls players use to scrape a cartridge.
use super::{click_control, desktop, right_click};
use crate::desktop::{Desktop, library::Workspace};
use eidoryn_psx::library::Library;
use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
use std::{path::Path, sync::atomic::AtomicBool};

fn open_collection(
    db: Library,
    cx: &mut TestAppContext,
) -> (Entity<Desktop>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(desktop);
    view.update(cx, |v, _| {
        v.preferences_path = None;
        v.preferences.library.auto_covers = false;
        v.preferences.library.scrape_new_games = false;
        v.library = Workspace::new(Some(db));
    });
    cx.simulate_resize(size(px(1400.), px(1000.)));
    click_control(cx, "nav-library");
    (view, cx)
}

fn shown_edition(view: &Entity<Desktop>, cx: &VisualTestContext, id: i64) -> String {
    view.read_with(cx, |v, _| {
        let entry = v.library.entry(id).unwrap();
        assert!(!entry.pending);
        let thumbnail = entry.thumbnail.as_ref().unwrap_or_else(|| {
            panic!(
                "missing artwork for {id}: lookup={:?}, cover={:?}, notice={:?}",
                v.library.metadata_lookups.get(&id),
                entry.cover_status,
                v.notice
            )
        });
        let cover = image::open(thumbnail).expect("thumbnail should be readable");
        assert!(cover.width() > 0 && cover.height() > 0);
        entry.title.clone()
    })
}

#[gpui::test]
fn right_click_and_full_refresh_resolve_gba_art_and_allow_manual_resolution(
    app: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(&dir.path().join("library.db")).unwrap();
    let mut rom = [0; 256];
    rom[0xac..0xb0].copy_from_slice(b"AXXJ");
    let exact = crc32fast::hash(&rom);
    for name in ["Original.gba", "Modified.gba"] {
        let path = dir.path().join(name);
        std::fs::write(&path, rom).unwrap();
        db.scan_files(&[path], &AtomicBool::default(), |_| {})
            .unwrap();
        rom[255] = 1;
    }
    let dat = format!(
        "game (\n name \"Retail (Japan)\"\n region \"Japan\"\n serial \"AXXJ\"\n rom ( name \"Retail.gba\" size 256 crc {exact:08x} )\n)\n\
         game (\n name \"Reissue (Japan)\"\n region \"Japan\"\n serial \"AXXJ\"\n rom ( name \"Reissue.gba\" size 256 crc 00000000 )\n)"
    );
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(8, 8)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let (view, cx) = open_collection(
        db.with_scrape_responses(
            http::Response::new(dat.into_bytes()),
            http::Response::new(png.into_inner()),
        ),
        app,
    );

    right_click(cx, "library-entry-1");
    click_control(cx, "lib-card-scrape");
    assert_eq!(shown_edition(&view, cx, 1), "Retail (Japan)");
    click_control(cx, "lib-refresh-all");
    assert_eq!(shown_edition(&view, cx, 1), "Retail (Japan)");
    click_control(cx, "library-entry-2");
    view.read_with(cx, |v, _| {
        let entry = v.library.entry(2).unwrap();
        assert!(entry.pending);
        assert!(entry.thumbnail.is_none());
        assert_eq!(v.library.scraping.last.as_ref().unwrap().1.needs_choice, 1);
    });
    click_control(cx, "lib-choose-edition");
    assert!(cx.debug_bounds("lib-match-1").is_some());
    click_control(cx, "lib-match-0");
    let chosen = shown_edition(&view, cx, 2);
    assert!(matches!(
        chosen.as_str(),
        "Retail (Japan)" | "Reissue (Japan)"
    ));
    assert!(cx.debug_bounds("lib-choose-edition").is_none());
    assert!(cx.debug_bounds("library-details").is_none());
}

#[gpui::test]
#[ignore = "requires local Japanese and US Aria of Sorrow cartridges and online Libretro access"]
fn local_aria_cartridges_scrape_from_the_library(app: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let db = Library::open(&dir.path().join("library.db")).unwrap();
    let roms = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let jp = "Castlevania - Akatsuki no Minuet (Japan)";
    let us = "Castlevania - Aria of Sorrow (USA)";
    for title in [jp, us] {
        db.scan_files(
            &[roms.join(format!("{title}.gba"))],
            &AtomicBool::default(),
            |_| {},
        )
        .unwrap();
    }
    let (view, cx) = open_collection(db, app);
    right_click(cx, "library-entry-1");
    click_control(cx, "lib-card-scrape");
    assert_eq!(shown_edition(&view, cx, 1), jp);
    click_control(cx, "lib-refresh-all");
    assert_eq!(shown_edition(&view, cx, 1), jp);
    assert_eq!(shown_edition(&view, cx, 2), us);
}

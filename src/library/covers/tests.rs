use super::*;
use crate::library::metadata;
use image::{DynamicImage, RgbImage};
use std::fs;

fn fetch_url(
    lib: &Library,
    id: i64,
    art: Remote<'_>,
    url: &Url,
    cancel: &AtomicBool,
    request: impl FnOnce() -> Result<Option<http::Response<Vec<u8>>>, String>,
) -> Result<String, String> {
    super::fetch_url(lib, id, art, url, cancel, || {
        request().map(|r| r.map_or(CoverResponse::Missing, CoverResponse::Http))
    })
}

const SERIAL: &str = "SCUS-94900";
const MATCHING: Remote<'static> = Remote {
    serial: SERIAL,
    release: None,
    generation: 0,
    kind: RemoteKind::Matching(Mode::Incremental),
};
const REFRESH: Remote<'static> = Remote {
    kind: RemoteKind::Matching(Mode::Full),
    ..MATCHING
};

pub(super) fn fixture() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(&dir.path().join("library.db")).unwrap();
    let mut conn = lib.connect().unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO games(id,fallback_title) VALUES(1,'Test')", [])
        .unwrap();
    metadata::identify(&tx, 1, Some(SERIAL), Some("USA"), "Test").unwrap();
    tx.commit().unwrap();
    (dir, lib)
}

fn encoded(format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::new(500, 125));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn url(path: &str) -> Url {
    Url::parse(&format!("https://covers.example/{path}")).unwrap()
}

fn response(status: u16, body: Vec<u8>) -> http::Response<Vec<u8>> {
    http::Response::builder().status(status).body(body).unwrap()
}

#[cfg(feature = "ui-tests")]
#[test]
fn explicit_and_missing_gba_art_identify_the_rom_before_any_metadata_refresh() {
    let cancel = AtomicBool::default();
    let dat = br#"game (
        name "Box & Cart (USA)"
        region "USA"
        serial "TSTE"
        rom ( name "Box & Cart (USA).gba" size 512 crc 9DFA7B38 )
    )
    game (
        name "Box & Cart (USA) (Rev 1)"
        region "USA"
        serial "TSTE"
        rom ( name "Box & Cart (USA) (Rev 1).gba" size 512 crc FFFFFFFF )
    )"#;
    for fetch_art in [fetch, fetch_missing] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renamed.gba");
        let mut rom = [0; 512];
        rom[0xac..0xb0].copy_from_slice(b"TSTE");
        fs::write(&path, rom).unwrap();
        let lib = Library::open(&dir.path().join("library.db"))
            .unwrap()
            .with_scrape_responses(
                response(200, dat.to_vec()),
                response(200, encoded(ImageFormat::Png)),
            );
        lib.scan_files(&[path], &cancel, |_| {}).unwrap();
        let id = lib.games().unwrap().remove(0).id;
        assert_eq!(
            fetch_art(&lib, id, Provider::Libretro, "", &cancel).unwrap(),
            "ready"
        );
        let cards = metadata::list_by_id(&lib, &[id]).unwrap();
        assert!(cards[&id].thumbnail.as_ref().is_some_and(|p| p.is_file()));
        assert!(
            cards[&id]
                .cover_source
                .as_deref()
                .unwrap()
                .ends_with("/Box%20_%20Cart%20(USA).png")
        );
    }
}

#[cfg(feature = "ui-tests")]
#[test]
fn missing_ntsc_j_art_retries_legacy_miss_but_selected_japan_market_remains_exact() {
    let (_dir, lib) = fixture();
    let serial = "SCPS-45196";
    let lib = lib.with_scrape_responses(
        response(200, vec![]),
        response(200, encoded(ImageFormat::Png)),
    );
    let db = lib.connect().unwrap();
    db.execute(
        "UPDATE game_metadata SET disc_serial=?,disc_region='Japan' WHERE game_id=1",
        [serial],
    )
    .unwrap();
    let old_key = provider_key(Provider::Libretro, "", serial, "", "Japan").unwrap();
    record(
        &db,
        1,
        Remote { serial, ..MATCHING },
        old_key.as_str(),
        "not_found",
        i64::MAX,
    )
    .unwrap();
    let cancel = AtomicBool::default();
    assert_eq!(
        fetch_missing(&lib, 1, Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    assert!(
        metadata::list(&lib).unwrap()[&1]
            .thumbnail
            .as_ref()
            .is_some_and(|file| file.is_file())
    );
    db.execute(
        "UPDATE game_metadata SET release_serial=?,auto_region='Japan' WHERE game_id=1",
        [serial],
    )
    .unwrap();
    assert_eq!(
        fetch_missing(&lib, 1, Provider::Libretro, "", &cancel).unwrap(),
        "not_found"
    );
}

#[test]
fn cached_download_local_override_and_atomic_failures() {
    let (dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let remote = url("cover");
    let status = fetch_url(&lib, 1, MATCHING, &remote, &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert_eq!(status, "ready");
    let cached = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert_eq!(image::image_dimensions(&cached).unwrap(), (256, 64));
    // Positive cache also rebuilds a missing thumbnail without a new request.
    fs::remove_file(&cached).unwrap();
    fetch_url(&lib, 1, MATCHING, &remote, &cancel, || {
        panic!("cached cover contacted server")
    })
    .unwrap();
    let cached = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert!(cached.is_file());
    let local = dir.path().join("user-scan.jpeg");
    fs::write(&local, encoded(ImageFormat::Png)).unwrap();
    set_local(&lib, 1, &local).unwrap();
    let user = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert_ne!(user, cached);
    assert_eq!(
        metadata::list(&lib).unwrap()[&1].cover_source.as_deref(),
        Some("user-scan.jpeg")
    );
    fs::write(&local, b"not a PNG").unwrap();
    assert!(set_local(&lib, 1, &local).is_err());
    assert!(
        fetch_url(&lib, 1, MATCHING, &url("corrupt"), &cancel, || Ok(Some(
            response(200, vec![1, 2])
        )))
        .is_err()
    );
    assert!(
        fetch_url(&lib, 1, MATCHING, &url("offline"), &cancel, || Err(
            "offline".into()
        ))
        .is_err()
    );
    assert_eq!(
        metadata::list(&lib).unwrap()[&1].thumbnail.as_ref(),
        Some(&user)
    );
    fs::remove_file(&user).unwrap();
    regenerate(&lib, 1).unwrap();
    assert!(
        metadata::list(&lib).unwrap()[&1]
            .thumbnail
            .as_ref()
            .unwrap()
            .is_file()
    );
    reset_local(&lib, 1).unwrap();
    assert_eq!(
        metadata::list(&lib).unwrap()[&1].thumbnail.as_ref(),
        Some(&cached)
    );
    assert!(local.exists());
}

#[test]
fn negative_cache_and_server_backoff_apply_across_requests() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let missing = url("missing");
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &missing, &cancel, || Ok(Some(response(
            404,
            vec![]
        ))))
        .unwrap(),
        "not_found"
    );
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &missing, &cancel, || panic!(
            "retried 404"
        ))
        .unwrap(),
        "not_found"
    );
    let queued = url("limited");
    let after = 3600;
    let now = seconds(SystemTime::now());
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &queued, &cancel, || Ok(Some(
            http::Response::builder()
                .status(429)
                .header("Retry-After", after.to_string())
                .body(Vec::new())
                .unwrap()
        )))
        .unwrap(),
        "retry"
    );
    let db = lib.connect().unwrap();
    let until: i64 = db
        .query_row("SELECT retry_after FROM cover_rate_limits", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(until >= now + after);
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &url("another"), &cancel, || panic!(
            "retried throttled host"
        ))
        .unwrap(),
        "retry"
    );
    assert_eq!(
        retry_deadline(Some("Thu, 01 Jan 1970 00:01:40 GMT"), 0),
        100
    );
}

#[test]
#[cfg(feature = "ui-tests")]
fn cover_only_refresh_uses_the_chosen_editions_region_after_clear() {
    let (_dir, lib) = fixture(); // Local disc is a US edition.
    let lib = lib.with_scrape_responses(
        response(503, Vec::new()),
        response(200, encoded(ImageFormat::Png)),
    );
    let serial = "SLPS-00001";
    lib.connect()
        .unwrap()
        .execute(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
         VALUES('Q1/SLPS-00001',?,'Japan','Chosen edition','1997','','')",
            [serial],
        )
        .unwrap();
    metadata::select(&lib, 1, "Q1/SLPS-00001").unwrap();
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    let cancel = AtomicBool::default();
    assert_eq!(
        fetch(&lib, 1, Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    let source: String = lib
        .connect()
        .unwrap()
        .query_row(
            "SELECT url FROM cover_attempts WHERE game_id=1 AND serial=?",
            [serial],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        source,
        provider_key(Provider::Libretro, "", serial, "", "Japan")
            .unwrap()
            .as_str()
    );
    let metadata = metadata::list(&lib).unwrap();
    assert!(metadata[&1].release_title.is_none());
    assert!(metadata[&1].date.is_none());
}

#[test]
fn explicit_fetch_retries_a_cached_miss_and_observes_provider_backoff() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let template = "https://covers.example/${serial}.png";
    let key = provider_key(Provider::Custom, template, SERIAL, "", "USA").unwrap();
    let db = lib.connect().unwrap();
    record(&db, 1, MATCHING, key.as_str(), "not_found", i64::MAX).unwrap();
    db.execute(
        "INSERT INTO cover_rate_limits(host,retry_after) VALUES('covers.example',?)",
        [i64::MAX],
    )
    .unwrap();
    assert_eq!(
        fetch_missing(&lib, 1, Provider::Custom, template, &cancel).unwrap(),
        "not_found"
    );
    assert_eq!(
        fetch(&lib, 1, Provider::Custom, template, &cancel).unwrap(),
        "retry"
    );
}

#[cfg(feature = "ui-tests")]
#[test]
fn only_a_confirmed_gamedb_edition_resolves_shared_serial_and_market_art() {
    let (_dir, lib) = fixture();
    let db = lib.connect().unwrap();
    let cancel = AtomicBool::default();
    let id = 1;
    let choices = [
        ("gamedb/SCUS-94900", "Example (USA)"),
        ("gamedb/SCUS-94900-R1", "Example (USA) (Rev 1)"),
    ];
    db.execute("UPDATE libretro_catalog SET sync_time=unixepoch()", [])
        .unwrap();
    for (key, name) in choices {
        db.execute(
            "INSERT INTO libretro_boxart_names(serial,region,name) VALUES (?1,'USA',?2)",
            params![SERIAL, name],
        )
        .unwrap();
        db.execute("INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date,source) VALUES (?1,?2,'USA','Example','',?3,'','GameDB-PSX')",params![key,SERIAL,name]).unwrap();
    }
    // A unique automatic metadata match still isn't proof of the disc revision.
    db.execute("UPDATE game_metadata SET release_id=?1,aliases=?2,release_serial=disc_serial,auto_region='USA' WHERE game_id=1", params![choices[0].0, choices[0].1]).unwrap();
    assert_eq!(
        fetch_missing(&lib, id, Provider::Libretro, "", &cancel).unwrap(),
        "needs_choice"
    );
    let lib = lib.with_scrape_responses(
        response(200, vec![]),
        response(200, encoded(ImageFormat::Png)),
    );
    for (release, name) in choices {
        metadata::select(&lib, id, release).unwrap();
        assert_eq!(
            fetch_missing(&lib, id, Provider::Libretro, "", &cancel).unwrap(),
            "ready"
        );
        let source = metadata::list(&lib).unwrap()[&id]
            .cover_source
            .clone()
            .unwrap();
        let url = Url::parse(&source).unwrap();
        assert!(url.query_pairs().any(|(k, v)| k == "edition" && v == name));
        let Lookup::Image(asset) = libretro::resolve(
            &lib,
            transport::api().unwrap(),
            SERIAL,
            "USA",
            Some(name),
            &cancel,
        )
        .unwrap() else {
            panic!("confirmed {release} should resolve")
        };
        assert_eq!(asset, libretro::image_url(libretro::BOXART, name).unwrap());
    }
    let missed = libretro::resolve(
        &lib,
        transport::api().unwrap(),
        SERIAL,
        "USA",
        Some("Example (Europe)"),
        &cancel,
    )
    .unwrap();
    assert!(matches!(missed, Lookup::Missing));
}

#[cfg(feature = "ui-tests")]
#[test]
fn screenscraper_keys_disc_only_lookups_by_license_and_matched_editions_by_market() {
    let (_dir, lib) = fixture();
    let lib = lib.with_scrape_responses(response(200, vec![]), response(404, vec![]));
    let conn = lib.connect().unwrap();
    let cancel = AtomicBool::default();
    for (disc, license, edition) in [
        ("USA", "NTSC-U", "Canada"),
        ("Japan", "NTSC-J", "Taiwan"),
        ("Europe", "PAL", "Australia"),
    ] {
        conn.execute("UPDATE game_metadata SET disc_region=?,auto_region=NULL,release_serial=NULL WHERE game_id=1", [disc]).unwrap();
        for region in [license, edition] {
            if region == edition {
                conn.execute("UPDATE game_metadata SET auto_region=?,release_serial=disc_serial WHERE game_id=1", [edition]).unwrap();
            }
            assert_eq!(
                fetch_missing(&lib, 1, Provider::ScreenScraper, "", &cancel).unwrap(),
                "not_found"
            );
            let key = provider_key(Provider::ScreenScraper, "", SERIAL, "", region).unwrap();
            assert!(
                conn.prepare(
                    "SELECT 1 FROM cover_attempts WHERE game_id=1 AND url=? AND state='not_found'"
                )
                .unwrap()
                .exists([key.as_str()])
                .unwrap(),
                "{region}"
            );
        }
    }
}

#[cfg(feature = "ui-tests")]
#[test]
fn custom_cover_fetch_uses_the_selected_regional_editions_serial() {
    let (_dir, lib) = fixture();
    let lib = lib.with_scrape_responses(
        response(200, vec![]),
        response(200, encoded(ImageFormat::Png)),
    );
    let conn = lib.connect().unwrap();
    let template = "https://covers.example/${serial}.png?signature=test";
    for (serial, market) in [
        ("SLPM-86023", "Japan"),
        ("SCES-00001", "Europe"),
        ("SCPS-45196", "Asia"),
        ("SCUS-94900", "USA"),
        ("SLES-00002", "Europe / Australia"),
    ] {
        conn.execute("INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date) VALUES (?1,?1,?2,'Regional game','','','')", [serial, market]).unwrap();
        let selected = metadata::candidates(&lib, 1, serial).unwrap().remove(0);
        assert_eq!(
            fetch_release(
                &lib,
                1,
                Provider::Custom,
                template,
                &selected,
                &AtomicBool::default()
            )
            .unwrap(),
            "ready"
        );
        let (stored_serial, key): (String, String) = conn
            .query_row(
                "SELECT serial,source FROM cover_assets WHERE game_id=1 AND kind='user'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_serial, serial);
        assert_eq!(
            key,
            private_key(cover_url(template, serial).unwrap()).as_str()
        );
        assert!(!key.contains("signature"));
    }
}

#[test]
fn rejects_oversize_invalid_and_cancelled_work_before_committing() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    assert!(cover_url("http://source/${serial}", SERIAL).is_err());
    assert!(cover_url("https://example.invalid", SERIAL).is_err());
    assert!(cover_url(DEFAULT_TEMPLATE, "../../secret").is_err());
    assert!(read_bounded(std::io::repeat(0).take(MAX_BYTES as u64 + 1), &cancel).is_err());
    for response in [
        response(200, vec![0; MAX_BYTES + 1]),
        http::Response::builder()
            .header("Content-Length", MAX_BYTES + 1)
            .body(encoded(ImageFormat::Png))
            .unwrap(),
    ] {
        let err = fetch_url(&lib, 1, REFRESH, &url("large"), &cancel, || {
            Ok(Some(response))
        })
        .unwrap_err();
        assert!(err.contains("8 MiB"));
    }
    let image = DynamicImage::ImageLuma8(image::GrayImage::new(4097, 4096));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    assert!(
        store(
            &lib,
            1,
            Source::User {
                name: "oversize.png",
                revision: 0
            },
            bytes.get_ref(),
            &cancel
        )
        .unwrap_err()
        .contains("megapixel")
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        fetch_url(&lib, 1, MATCHING, &url("stopped"), &cancel, || panic!(
            "cancelled task sent request"
        ))
        .is_err()
    );
    assert!(metadata::list(&lib).unwrap()[&1].thumbnail.is_none());
}

#[test]
#[ignore = "live opt-in HTTPS smoke test; requires network"]
fn live_psx_covers_smoke() {
    let (_dir, lib) = fixture();
    let c = AtomicBool::new(false);
    assert_eq!(
        fetch(&lib, 1, Provider::Custom, DEFAULT_TEMPLATE, &c).unwrap(),
        "ready"
    );
    let m = metadata::list(&lib).unwrap();
    assert!(m[&1].thumbnail.as_ref().unwrap().is_file());
}

#[test]
#[ignore = "live Libretro catalog and cover download; requires network"]
fn live_libretro_cover_smoke() {
    let (dir, lib) = fixture();
    assert!(!dir.path().join("covers").exists());
    let cancel = AtomicBool::new(false);
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    for (id, serial) in [
        (2, "SLUS-00402"),
        (3, "SLUS-00067"),
        (4, "SLUS-00421"),
        (5, "SLUS-00592"),
    ] {
        tx.execute(
            "INSERT INTO games(id,fallback_title) VALUES(?,'Test')",
            [id],
        )
        .unwrap();
        metadata::identify(&tx, id, Some(serial), Some("USA"), "Test").unwrap();
    }
    tx.commit().unwrap();
    for id in 1..=5 {
        assert_eq!(
            fetch(&lib, id, Provider::Libretro, "", &cancel).unwrap(),
            "ready",
            "cover for fixture {id}"
        );
        let thumb = metadata::list(&lib).unwrap()[&id]
            .thumbnail
            .clone()
            .unwrap();
        assert!(thumb.starts_with(dir.path().join("covers/objects")));
        assert!(thumb.with_file_name("original.png").is_file());
        let (w, h) = image::image_dimensions(thumb).unwrap();
        assert!(w > 0 && h > 0 && w.max(h) <= THUMB_SIZE);
    }
}

#[test]
#[ignore = "requires the local USA Aria of Sorrow cartridge and live Libretro"]
fn live_gba_cartridge_fetches_its_own_boxart_before_metadata_refresh() {
    let rom =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/Castlevania - Aria of Sorrow (USA).gba");
    let cancel = AtomicBool::default();
    for fetch_art in [fetch, fetch_missing] {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("library.db")).unwrap();
        lib.scan_files(std::slice::from_ref(&rom), &cancel, |_| {})
            .unwrap();
        let game = lib.games().unwrap().remove(0);
        assert_eq!(game.serial.as_deref(), Some("A2CE"));
        assert_eq!(
            fetch_art(&lib, game.id, Provider::Libretro, "", &cancel).unwrap(),
            "ready"
        );
        let cards = metadata::list_by_id(&lib, &[game.id]).unwrap();
        let info = &cards[&game.id];
        assert!(image::open(info.thumbnail.as_ref().unwrap()).is_ok());
        assert!(
            info.cover_source
                .as_deref()
                .unwrap()
                .ends_with("/Castlevania%20-%20Aria%20of%20Sorrow%20(USA).png")
        );
    }
}

#[test]
#[ignore = "loopback integration fixture; requires local socket permission"]
fn loopback_http_response_uses_real_transport_and_negative_cache() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url::parse(&format!(
        "http://{}/SCUS-94900.jpg",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = BufReader::new(&stream);
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        assert_eq!(line, "GET /SCUS-94900.jpg HTTP/1.1\r\n");
        loop {
            line.clear();
            if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &url, &cancel, || {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            transport::cover(client.get(url.clone()), MAX_BYTES, &cancel)
                .map(Some)
                .map_err(|e| e.to_string())
        })
        .unwrap(),
        "retry"
    );
    server.join().unwrap();
    assert_eq!(
        fetch_url(&lib, 1, MATCHING, &url, &cancel, || panic!("throttled")).unwrap(),
        "retry"
    );
}

#[test]
fn user_selected_art_from_another_region_survives_background_refresh() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let chosen = Remote {
        serial: "SCPS-10031",
        release: None,
        generation: 0,
        kind: RemoteKind::Chosen {
            revision: reserve_selection(&lib.connect().unwrap(), 1).unwrap(),
        },
    };
    fetch_url(&lib, 1, chosen, &url("japan"), &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Png))))
    })
    .unwrap();
    let selected = metadata::list(&lib).unwrap()[&1].thumbnail.clone();
    fetch_url(&lib, 1, MATCHING, &url("us"), &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    let refreshed = metadata::list(&lib).unwrap();
    assert_eq!(refreshed[&1].thumbnail, selected);
    assert_eq!(refreshed[&1].region.as_deref(), Some("USA"));
    assert_eq!(
        refreshed[&1].cover_source.as_deref(),
        Some("https://covers.example/japan")
    );
    reset_local(&lib, 1).unwrap();
    assert_ne!(metadata::list(&lib).unwrap()[&1].thumbnail, selected);
}

#[test]
fn a_late_download_cannot_restore_a_superseded_user_selection() {
    let (dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let local = dir.path().join("latest.png");
    fs::write(&local, encoded(ImageFormat::Png)).unwrap();
    for reset in [false, true] {
        let chosen = Remote {
            serial: "SCPS-10031",
            release: None,
            generation: 0,
            kind: RemoteKind::Chosen {
                revision: reserve_selection(&lib.connect().unwrap(), 1).unwrap(),
            },
        };
        // Perform the next explicit user action while the older request is outstanding.
        let outcome = fetch_url(&lib, 1, chosen, &url("late"), &cancel, || {
            if reset {
                reset_local(&lib, 1).unwrap();
            } else {
                set_local(&lib, 1, &local).unwrap();
            }
            Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
        })
        .unwrap();
        assert_eq!(outcome, "superseded");
        assert_eq!(
            metadata::list(&lib).unwrap()[&1].cover_source.as_deref(),
            if reset { None } else { Some("latest.png") }
        );
    }
}

#[test]
fn an_older_gba_response_cannot_overwrite_the_selected_edition_with_the_same_code() {
    let (dir, lib) = fixture();
    let path = dir.path().join("game.gba");
    let mut rom = [0; 512];
    rom[0xac..0xb0].copy_from_slice(b"TSTE");
    fs::write(&path, rom).unwrap();
    let cancel = AtomicBool::default();
    lib.scan_files(&[path], &cancel, |_| {}).unwrap();
    let id = lib
        .games()
        .unwrap()
        .into_iter()
        .find(|g| g.platform == "GBA")
        .unwrap()
        .id;
    let names = ["Box (USA)", "Box (USA) (Rev 1)"];
    let keys = names.map(|name| metadata::no_intro::edition_id("TSTE", name));
    let db = lib.connect().unwrap();
    for (name, key) in names.iter().zip(&keys) {
        db.execute(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
             VALUES(?1,'TSTE','USA',?2,'','','')",
            params![key, name],
        )
        .unwrap();
    }
    let art = |release| Remote {
        serial: "TSTE",
        release: Some(release),
        ..REFRESH
    };
    metadata::select(&lib, id, &keys[0]).unwrap();
    let outcome = fetch_url(&lib, id, art(&keys[0]), &url("original"), &cancel, || {
        metadata::select(&lib, id, &keys[1])?;
        fetch_url(&lib, id, art(&keys[1]), &url("rev1"), &cancel, || {
            Ok(Some(response(200, encoded(ImageFormat::Png))))
        })?;
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert_eq!(outcome, "superseded");
    assert_eq!(
        metadata::list(&lib).unwrap()[&id].cover_source.as_deref(),
        Some("https://covers.example/rev1")
    );
    assert!(
        !db.prepare("SELECT 1 FROM cover_attempts WHERE game_id=? AND url=?")
            .unwrap()
            .exists(params![id, url("original").as_str()])
            .unwrap()
    );
}

#[test]
fn switching_editions_detaches_only_the_download_and_rejects_late_serial_art() {
    let (dir, lib) = fixture();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO games(id,fallback_title) VALUES(2,'Shared')",
        [],
    )
    .unwrap();
    metadata::identify(&tx, 2, None, None, "Shared").unwrap();
    for key in ["gamedb/disc", "gamedb/revision"] {
        tx.execute(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
             VALUES(?,?,'USA','Title','','','')",
            params![key, SERIAL],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let cancel = AtomicBool::default();
    // An initial lookup can already be in flight when the user chooses an edition.
    let state = fetch_url(&lib, 1, REFRESH, &url("before-choice"), &cancel, || {
        metadata::select(&lib, 1, "gamedb/disc")?;
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert_eq!(state, "superseded");

    let original = Remote {
        release: Some("gamedb/disc"),
        ..REFRESH
    };
    let png = encoded(ImageFormat::Png);
    fetch_url(&lib, 1, original, &url("original"), &cancel, || {
        Ok(Some(response(200, png.clone())))
    })
    .unwrap();
    let thumb = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    let local = dir.path().join("user.png");
    fs::write(&local, png).unwrap();
    set_local(&lib, 2, &local).unwrap();
    let state = fetch_url(&lib, 1, original, &url("late"), &cancel, || {
        metadata::select(&lib, 1, "gamedb/revision")?;
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert_eq!(state, "superseded");
    let revision = Remote {
        release: Some("gamedb/revision"),
        ..REFRESH
    };
    fetch_url(&lib, 1, revision, &url("missing"), &cancel, || {
        Ok(Some(response(404, Vec::new())))
    })
    .unwrap();
    let cards = metadata::list(&Library::open(&dir.path().join("library.db")).unwrap()).unwrap();
    assert!(cards[&1].thumbnail.is_none());
    assert_eq!(cards[&1].status.as_deref(), Some("not_found"));
    assert_eq!(collect_orphans(&lib).unwrap(), 0);
    assert_eq!(cards[&2].thumbnail.as_ref(), Some(&thumb));
    assert!(thumb.is_file());
    // Explicitly chosen art remains with its owner across metadata assignments.
    set_local(&lib, 1, &local).unwrap();
    metadata::select(&lib, 1, "gamedb/disc").unwrap();
    assert!(metadata::list(&lib).unwrap()[&1].manual_cover);
}

#[cfg(feature = "ui-tests")]
#[test]
fn refreshing_a_legacy_aamj_selection_detaches_its_other_markets_cover() {
    let (dir, lib) = fixture();
    let db = lib.connect().unwrap();
    let base = "Akumajou Dracula - Circle of the Moon";
    db.execute("UPDATE games SET platform='GBA' WHERE id=1", [])
        .unwrap();
    db.execute(
        "UPDATE game_metadata SET disc_serial='AAMJ',disc_region='Japan' WHERE game_id=1",
        [],
    )
    .unwrap();
    for market in ["Japan", "China"] {
        let title = format!("{base} ({market})");
        let key = metadata::no_intro::edition_id("AAMJ", &title);
        db.execute(
            "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
             VALUES(?,'AAMJ',?,?,'','','')",
            params![key, market, title],
        )
        .unwrap();
        if market == "China" {
            metadata::select(&lib, 1, &key).unwrap();
        }
    }
    let japan =
        libretro::image_url(metadata::no_intro::COVER_BASE, &format!("{base} (Japan)")).unwrap();
    // Simulate the pre-fix state on disk: selected China, downloaded art still Japan.
    db.execute(
        "INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb)
        VALUES(1,'download','AAMJ',?,'japan.png','japan.png')",
        [japan.as_str()],
    )
    .unwrap();
    fs::write(dir.path().join("japan.png"), encoded(ImageFormat::Png)).unwrap();
    let lib = Library::open(&dir.path().join("library.db"))
        .unwrap()
        .with_scrape_responses(response(200, Vec::new()), response(404, Vec::new()));
    assert!(metadata::list(&lib).unwrap()[&1].thumbnail.is_some());
    assert_eq!(
        fetch(&lib, 1, Provider::Libretro, "", &AtomicBool::default()).unwrap(),
        "not_found"
    );
    let cards = metadata::list(&lib).unwrap();
    assert_eq!(cards[&1].region.as_deref(), Some("China"));
    assert!(cards[&1].thumbnail.is_none());
}

#[test]
fn full_refresh_revalidates_cached_and_missing_art_but_keeps_good_files_on_failure_and_backoff() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let source = url("full");
    let fetch = |art, status, bytes| {
        fetch_url(&lib, 1, art, &source, &cancel, || {
            Ok(Some(response(status, bytes)))
        })
    };
    let cached = MATCHING;
    let full = REFRESH;
    assert_eq!(fetch(cached, 404, vec![]).unwrap(), "not_found");
    assert_eq!(
        fetch(full, 200, encoded(ImageFormat::Jpeg)).unwrap(),
        "ready"
    );
    let first = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert!(first.is_file());
    assert!(fetch(full, 200, vec![0]).is_err());
    assert!(first.is_file());
    assert_eq!(
        fetch(full, 200, encoded(ImageFormat::Png)).unwrap(),
        "ready"
    );
    let second = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert_ne!(second, first);
    assert_eq!(fetch(full, 429, vec![]).unwrap(), "retry");
    assert_eq!(
        fetch_url(&lib, 1, full, &source, &cancel, || panic!(
            "refresh ignored backoff"
        ))
        .unwrap(),
        "retry"
    );
    assert!(second.is_file());
    assert_eq!(
        metadata::list(&lib).unwrap()[&1].thumbnail.as_ref(),
        Some(&second)
    );
}

#[test]
#[ignore = "live batch using the Wikidata and Libretro services; requires network"]
fn live_library_batch_full_and_incremental_smoke() {
    use crate::library::scrape;

    let (directory, lib) = fixture();
    let cancel = AtomicBool::new(false);
    metadata::set_override(&lib, 1, metadata::Field::Title, Some("My copy")).unwrap();
    let run = |mode| {
        let (ok_before, failed_before) = transport::sample::totals();
        let report = scrape::run(&lib, &[1], mode, Provider::Libretro, "", &cancel, |_, _| {});
        let (ok_after, failed_after) = transport::sample::totals();
        let ok = ok_after - ok_before;
        let failed = failed_after - failed_before;
        println!(
            "HTTP {mode:?}: {} completed, {ok} successful, {failed} failed",
            ok + failed
        );
        assert_eq!(
            (report.finished, report.failed, report.skipped),
            (1, 0, 0),
            "{:?}",
            report.errors
        );
        let rows = metadata::list(&lib).unwrap();
        let game = &rows[&1];
        assert_eq!(game.title.as_deref(), Some("My copy"));
        assert_eq!(game.date.as_deref(), Some("1996-09-09"));
        assert_eq!(game.source.as_deref(), Some("Wikidata (online)"));
        let thumbnail = game.thumbnail.as_ref().unwrap();
        assert!(thumbnail.starts_with(directory.path().join("covers/objects")));
        assert!(thumbnail.with_file_name("original.png").is_file());
        assert!(image::image_dimensions(thumbnail).is_ok());
        thumbnail.clone()
    };
    let first = run(scrape::Mode::Full);
    let raw: Vec<u8> = lib
        .connect()
        .unwrap()
        .query_row(
            "SELECT payload FROM metadata_responses WHERE serial=?",
            [SERIAL],
            |row| row.get(0),
        )
        .unwrap();
    let response: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(
        response["results"]["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["title"]["value"] == "Crash Bandicoot")
    );
    assert_eq!(run(scrape::Mode::Incremental), first);
    let refreshed = run(scrape::Mode::Full);
    assert_eq!(refreshed, first); // Identical upstream image shares the same immutable files.
}

#[test]
fn identical_covers_share_files_and_collection_checks_references() {
    let (dir, lib) = fixture();
    let db = lib.connect().unwrap();
    db.execute(
        "INSERT INTO games(id,fallback_title) VALUES(2,'Second')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO game_metadata(game_id,filename) VALUES(2,'Second')",
        [],
    )
    .unwrap();
    let image = dir.path().join("front.png");
    fs::write(&image, encoded(ImageFormat::Png)).unwrap();
    set_local(&lib, 1, &image).unwrap();
    set_local(&lib, 2, &image).unwrap();
    let games = metadata::list(&lib).unwrap();
    let thumb = games[&1].thumbnail.as_ref().unwrap();
    assert_eq!(Some(thumb), games[&2].thumbnail.as_ref());
    assert_eq!(db.query_row("SELECT count(DISTINCT original) FROM cover_assets WHERE width=500 AND height=125 AND mime_type='image/png' AND length(content_hash)=64", [], |r| r.get::<_,i32>(0)).unwrap(), 1);
    reset_local(&lib, 1).unwrap();
    assert!(thumb.is_file());
    let leftover = dir.path().join("covers/objects").join("0".repeat(64));
    fs::create_dir(&leftover).unwrap();
    let unknown = dir.path().join("covers/objects/my-scans");
    fs::create_dir(&unknown).unwrap();
    assert_eq!(collect_orphans(&lib).unwrap(), 1);
    assert!(!leftover.exists() && thumb.is_file() && unknown.is_dir());
    reset_local(&lib, 2).unwrap();
    assert!(!thumb.exists() && image.is_file());
}

#[test]
fn artwork_recovers_from_crashes_before_and_after_commit() {
    const DB: &str = "EIDORYN_TEST_COVER_CRASH_DB";
    if let Some(path) = std::env::var_os(DB) {
        let lib = Library::open(Path::new(&path)).unwrap();
        set_local(&lib, 1, &lib.base_dir().join("user.png")).unwrap();
        unreachable!("child should terminate at the configured crash point");
    }
    for point in ["staged", "published", "committed"] {
        let (dir, _lib) = fixture();
        fs::write(dir.path().join("user.png"), encoded(ImageFormat::Png)).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "library::covers::tests::artwork_recovers_from_crashes_before_and_after_commit",
            ])
            .env(DB, dir.path().join("library.db"))
            .env("EIDORYN_TEST_COVER_CRASH", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73), "{point}");
        let reopened = Library::open(&dir.path().join("library.db")).unwrap();
        if point == "staged" {
            let objects = dir.path().join("covers/objects");
            let temp = fs::read_dir(objects).unwrap().next().unwrap().unwrap();
            assert!(temp.file_name().to_str().unwrap().starts_with(".incoming-"));
            assert_eq!(collect_orphans(&reopened).unwrap(), 0);
        }
        assert_eq!(
            assets::collect_at(&reopened, SystemTime::now() + Duration::from_hours(25)).unwrap(),
            usize::from(point != "committed"),
            "{point}"
        );
        let games = metadata::list(&reopened).unwrap();
        assert_eq!(
            games[&1]
                .thumbnail
                .as_ref()
                .is_some_and(|path| path.is_file()),
            point == "committed"
        );
    }
}

#[test]
fn migration_redacts_custom_urls_for_automatic_and_user_selected_art() {
    let (_dir, lib) = fixture();
    let mut conn = lib.connect().unwrap();
    let tx = conn.transaction().unwrap();
    let sensitive = "https://images.example/.eidoryn/cover/private-path/scan.png?token=secret";
    for kind in ["download", "user"] {
        tx.execute("INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb) VALUES(1,?1,?2,?3,'covers/a','covers/b')",params![kind,SERIAL,sensitive]).unwrap();
    }
    tx.execute("INSERT INTO games(id,fallback_title) VALUES(2,'Other')", [])
        .unwrap();
    tx.execute("INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb) VALUES(2,'user',NULL,'My cover.jpg','covers/c','covers/d')", []).unwrap();
    record(&tx, 1, MATCHING, sensitive, "ready", 0).unwrap();
    migrate_private_sources(&tx).unwrap();
    let expected = private_key(Url::parse(sensitive).unwrap());
    assert_eq!(expected.host_str(), Some("images.example"));
    assert!(expected.query().is_none());
    assert!(!expected.as_str().contains("private-path"));
    let count: u32 = tx
        .query_row(
            "SELECT count(*) FROM cover_assets WHERE source=?",
            [expected.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    assert!(
        tx.prepare("SELECT 1 FROM cover_attempts WHERE url=?")
            .unwrap()
            .exists([expected.as_str()])
            .unwrap()
    );
    let local: String = tx
        .query_row("SELECT source FROM cover_assets WHERE game_id=2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(local, "My cover.jpg");
}

#[test]
fn migration_backup_keeps_older_cover_paths_available_for_rollback() {
    let (dir, lib) = fixture();
    let original = dir
        .path()
        .join("covers/download/4f55ce75-daa9-4700-9bb7-825abefb2d3f/original.png");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, encoded(ImageFormat::Png)).unwrap();
    let backup = dir.path().join("library.db.v3.bak");
    lib.connect()
        .unwrap()
        .execute("VACUUM INTO ?", [backup.to_str().unwrap()])
        .unwrap();
    assert_eq!(collect_orphans(&lib).unwrap(), 0);
    assert!(original.is_file());
    let archived = dir
        .path()
        .join("library.db.v3.909e6d9d-1ff4-4176-be0c-ab5c4bbf66c8.bak");
    fs::rename(backup, &archived).unwrap();
    assert_eq!(collect_orphans(&lib).unwrap(), 0);
    fs::remove_file(archived).unwrap();
    assert_eq!(collect_orphans(&lib).unwrap(), 1);
}

#[test]
fn cover_download_validates_mime_against_the_encoded_image() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::default();
    let key = url("mime");
    for (header, format, valid) in [
        ("image/png; charset=binary", ImageFormat::Png, true),
        ("image/jpeg", ImageFormat::Jpeg, true),
        ("application/octet-stream", ImageFormat::Png, true),
        ("", ImageFormat::Jpeg, true),
        ("image/png", ImageFormat::Jpeg, false),
        ("text/html", ImageFormat::Png, false),
    ] {
        let previous = metadata::list(&lib).unwrap()[&1].thumbnail.clone();
        let result = fetch_url(&lib, 1, REFRESH, &key, &cancel, || {
            Ok(Some(
                http::Response::builder()
                    .status(200)
                    .header("Content-Type", header)
                    .body(encoded(format))
                    .unwrap(),
            ))
        });
        if valid {
            assert_eq!(result.as_deref(), Ok("ready"), "{header}");
        } else {
            assert!(result.unwrap_err().contains("Content-Type"));
            assert_eq!(metadata::list(&lib).unwrap()[&1].thumbnail, previous);
        }
    }
}

#[test]
fn a_late_response_from_the_replaced_provider_cannot_publish_an_asset() {
    use crate::library::jobs::{self, Stage};

    for in_backoff in [false, true] {
        let (_dir, lib) = fixture();
        let mut first = lib.connect().unwrap();
        jobs::prepare_targets(&mut first, &[1], Mode::Full, Provider::Igdb, false).unwrap();
        let old_meta = jobs::claim(&mut first, "old", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        old_meta.finish(&first, "updated").unwrap();
        let old = jobs::claim(&mut first, "old", Stage::Cover, 1)
            .unwrap()
            .pop()
            .unwrap();
        if in_backoff {
            let tx = first.transaction().unwrap();
            jobs::fail(
                &tx,
                &old,
                &jobs::Failure {
                    code: "limited",
                    message: "Provider rate limit",
                    retry: true,
                    deadline: Some(i64::MAX),
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        jobs::enqueue_games(&lib, &[1], Mode::Full, Provider::Libretro).unwrap();
        let mut next = lib.connect().unwrap();
        jobs::prepare_targets(&mut next, &[1], Mode::Full, Provider::Libretro, true).unwrap();
        let meta = jobs::claim(&mut next, "new", Stage::Metadata, 1)
            .unwrap()
            .pop()
            .unwrap();
        meta.finish(&next, "updated").unwrap();
        let new = jobs::claim(&mut next, "new", Stage::Cover, 1)
            .unwrap()
            .pop()
            .unwrap();
        let publish = |claim, provider, format| {
            store(
                &lib,
                1,
                Source::Download {
                    art: Remote {
                        kind: RemoteKind::Job { full: true, claim },
                        ..MATCHING
                    },
                    url: provider,
                },
                &encoded(format),
                &AtomicBool::new(false),
            )
        };
        assert!(publish(&new, "https://libretro.example/front", ImageFormat::Png).unwrap());
        assert!(publish(&old, "https://igdb.example/other", ImageFormat::Jpeg).is_err());
        let (source,mime,path):(String,String,String)=next.query_row("SELECT source,mime_type,original FROM cover_assets WHERE game_id=1 AND kind='download'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(source, "https://libretro.example/front");
        assert_eq!(mime, "image/png");
        assert_eq!(
            fs::read(lib.base_dir().join(path)).unwrap(),
            encoded(ImageFormat::Png)
        );
        assert_eq!(jobs::due_games(&lib).unwrap(), []);
        assert_eq!(jobs::recent_failures(&lib).unwrap(), []);
    }
}

#[test]
fn clearing_covers_removes_downloads_by_scope_and_keeps_shared_and_custom_files() {
    let (dir, lib) = fixture();
    let mut conn = lib.connect().unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO games(id,fallback_title) VALUES(2,'Copy')", [])
        .unwrap();
    metadata::identify(&tx, 2, Some(SERIAL), Some("USA"), "Copy").unwrap();
    tx.commit().unwrap();
    let cancel = AtomicBool::new(false);
    let source = url("shared");
    for id in [1, 2] {
        fetch_url(&lib, id, MATCHING, &source, &cancel, || {
            Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
        })
        .unwrap();
    }
    let downloaded = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    let external = dir.path().join("chosen.png");
    fs::write(&external, encoded(ImageFormat::Png)).unwrap();
    set_local(&lib, 1, &external).unwrap();
    let custom = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    let rows = |id| -> (usize, usize) {
        conn.query_row(
            "SELECT (SELECT count(*) FROM cover_assets WHERE game_id=?1 AND kind='download'),
                          (SELECT count(*) FROM cover_attempts WHERE game_id=?1)",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    lib.remove_games_and_clear_metadata(&[1]).unwrap();
    assert_eq!(rows(1), (0, 0));
    assert_eq!(rows(2), (1, 1));
    assert!(downloaded.is_file());
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    assert_eq!(rows(2), (0, 0));
    assert!(!downloaded.parent().unwrap().exists());
    assert!(external.is_file());
    assert!(custom.is_file());
    drop(conn);
    drop(lib);
    let lib = Library::open(&dir.path().join("library.db")).unwrap();
    // A queued browse request after restarting must skip before constructing a source URL.
    assert_eq!(
        fetch_missing(&lib, 2, Provider::Custom, "", &cancel).unwrap(),
        "skipped"
    );
    assert!(metadata::list(&lib).unwrap()[&2].cache_cleared);
    let art = Remote {
        generation: 1,
        ..MATCHING
    };
    fetch_url(&lib, 2, art, &source, &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert!(downloaded.is_file());
    assert!(!metadata::list(&lib).unwrap()[&2].cache_cleared);
}

#[test]
fn late_cover_responses_and_failures_cannot_refill_a_cleared_cache() {
    for status in [200, 404, 500] {
        let (_dir, lib) = fixture();
        let cancel = AtomicBool::new(false);
        let result = fetch_url(&lib, 1, MATCHING, &url("stale"), &cancel, || {
            lib.clear_metadata_cache(metadata::CacheScope::All)?;
            Ok(Some(response(status, encoded(ImageFormat::Png))))
        });
        if status == 200 {
            assert_eq!(result.unwrap(), "superseded");
        }
        let db = lib.connect().unwrap();
        let counts: (usize, usize) = db
            .query_row(
                "SELECT (SELECT count(*) FROM cover_assets), (SELECT count(*) FROM cover_attempts)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
        assert!(metadata::list(&lib).unwrap()[&1].cache_cleared);
    }
}

#[test]
fn staged_cover_job_cannot_run_after_its_clear() {
    use crate::library::jobs::{self, Stage};
    let (_dir, lib) = fixture();
    let mut db = lib.connect().unwrap();
    jobs::prepare_targets(&mut db, &[1], Mode::Full, Provider::Custom, false).unwrap();
    let meta = jobs::claim(&mut db, "worker", Stage::Metadata, 1)
        .unwrap()
        .pop()
        .unwrap();
    meta.finish(&db, "updated").unwrap();
    let cover = jobs::claim(&mut db, "worker", Stage::Cover, 1)
        .unwrap()
        .pop()
        .unwrap();
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    assert_eq!(
        scrape_claim(
            &lib,
            1,
            Provider::Custom,
            "",
            Mode::Full,
            &AtomicBool::new(false),
            &cover
        )
        .unwrap(),
        "superseded"
    );
    assert_eq!(
        db.query_row(
            "SELECT status FROM scrape_jobs WHERE id=?",
            [cover.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "cancelled"
    );
}

#[test]
fn detached_cover_with_a_damaged_object_path_does_not_undo_removal() {
    let (_dir, lib) = fixture();
    let directory = format!("covers/objects/{}", "1".repeat(64));
    let damaged = lib.base_dir().join(&directory);
    fs::create_dir_all(damaged.parent().unwrap()).unwrap();
    // A regular file left at the object's directory path makes remove_dir_all fail.
    fs::write(&damaged, b"damaged cover").unwrap();
    let db = lib.connect().unwrap();
    db.execute(
        "INSERT INTO cover_assets(game_id,kind,source,original,thumb) VALUES(1,'download','remote',?1,?2)",
        params![format!("{directory}/original.png"), format!("{directory}/thumb.png")],
    ).unwrap();
    lib.remove_games_and_clear_metadata(&[1]).unwrap();
    assert!(lib.games().unwrap().is_empty());
    assert_eq!(
        db.query_row("SELECT count(*) FROM cover_assets", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        0
    );
    assert!(damaged.is_file());
}

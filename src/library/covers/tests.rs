use super::*;
use crate::library::metadata;
use image::{DynamicImage, RgbImage};

const SERIAL: &str = "SCUS-94900";

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

fn response(status: u16, body: Vec<u8>) -> reqwest::blocking::Response {
    http::Response::builder()
        .status(status)
        .body(body)
        .unwrap()
        .into()
}

#[test]
fn cached_download_local_override_and_atomic_failures() {
    let (dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let remote = url("cover");
    let status = fetch_url(&lib, 1, Remote::Matching(SERIAL), &remote, &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Jpeg))))
    })
    .unwrap();
    assert_eq!(status, "ready");
    let cached = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert_eq!(image::image_dimensions(&cached).unwrap(), (256, 64));
    // Positive cache also rebuilds a missing thumbnail without a new request.
    fs::remove_file(&cached).unwrap();
    fetch_url(&lib, 1, Remote::Matching(SERIAL), &remote, &cancel, || {
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
        fetch_url(
            &lib,
            1,
            Remote::Matching(SERIAL),
            &url("corrupt"),
            &cancel,
            || Ok(Some(response(200, vec![1, 2])))
        )
        .is_err()
    );
    assert!(
        fetch_url(
            &lib,
            1,
            Remote::Matching(SERIAL),
            &url("offline"),
            &cancel,
            || Err("offline".into())
        )
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
        fetch_url(&lib, 1, Remote::Matching(SERIAL), &missing, &cancel, || Ok(
            Some(response(404, vec![]))
        ))
        .unwrap(),
        "not_found"
    );
    assert_eq!(
        fetch_url(
            &lib,
            1,
            Remote::Matching(SERIAL),
            &missing,
            &cancel,
            || panic!("retried 404")
        )
        .unwrap(),
        "not_found"
    );
    let queued = url("limited");
    let after = 3600;
    let now = seconds(SystemTime::now());
    assert_eq!(
        fetch_url(&lib, 1, Remote::Matching(SERIAL), &queued, &cancel, || Ok(
            Some(
                http::Response::builder()
                    .status(429)
                    .header("Retry-After", after.to_string())
                    .body(Vec::new())
                    .unwrap()
                    .into()
            )
        ))
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
        fetch_url(
            &lib,
            1,
            Remote::Matching(SERIAL),
            &url("another"),
            &cancel,
            || panic!("retried throttled host")
        )
        .unwrap(),
        "retry"
    );
    assert_eq!(
        retry_deadline(Some("Thu, 01 Jan 1970 00:01:40 GMT"), 0),
        100
    );
}

#[test]
fn rejects_oversize_invalid_and_cancelled_work_before_committing() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    assert!(cover_url("http://source/${serial}", SERIAL).is_err());
    assert!(cover_url("https://example.invalid", SERIAL).is_err());
    assert!(cover_url(DEFAULT_TEMPLATE, "../../secret").is_err());
    assert!(read_bounded(std::io::repeat(0).take(MAX_BYTES as u64 + 1), &cancel).is_err());
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
        fetch_url(
            &lib,
            1,
            Remote::Matching(SERIAL),
            &url("stopped"),
            &cancel,
            || panic!("cancelled task sent request")
        )
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
    assert_eq!(
        fetch(&lib, 1, Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    let thumb = metadata::list(&lib).unwrap()[&1].thumbnail.clone().unwrap();
    assert!(thumb.starts_with(dir.path().join("covers/download")));
    assert!(thumb.with_file_name("original.png").is_file());
    let (w, h) = image::image_dimensions(thumb).unwrap();
    assert!(w > 0 && h > 0 && w.max(h) <= THUMB_SIZE);
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
        fetch_url(&lib, 1, Remote::Matching(SERIAL), &url, &cancel, || {
            Client::new()
                .get(url.clone())
                .send()
                .map(Some)
                .map_err(|e| e.to_string())
        })
        .unwrap(),
        "retry"
    );
    server.join().unwrap();
    assert_eq!(
        fetch_url(&lib, 1, Remote::Matching(SERIAL), &url, &cancel, || panic!(
            "throttled"
        ))
        .unwrap(),
        "retry"
    );
}

#[test]
fn user_selected_art_from_another_region_survives_background_refresh() {
    let (_dir, lib) = fixture();
    let cancel = AtomicBool::new(false);
    let chosen = Remote::Chosen {
        serial: "SCPS-10031",
        revision: reserve_selection(&lib.connect().unwrap(), 1).unwrap(),
    };
    fetch_url(&lib, 1, chosen, &url("japan"), &cancel, || {
        Ok(Some(response(200, encoded(ImageFormat::Png))))
    })
    .unwrap();
    let selected = metadata::list(&lib).unwrap()[&1].thumbnail.clone();
    fetch_url(
        &lib,
        1,
        Remote::Matching(SERIAL),
        &url("us"),
        &cancel,
        || Ok(Some(response(200, encoded(ImageFormat::Jpeg)))),
    )
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
        let chosen = Remote::Chosen {
            serial: "SCPS-10031",
            revision: reserve_selection(&lib.connect().unwrap(), 1).unwrap(),
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

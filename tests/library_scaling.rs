//! Reproducible local scanner and warm scraper measurements with no remote service dependency.
#![cfg(feature = "desktop")]

use eidoryn_psx::library::{Library, covers::Provider, scrape};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

fn measure(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    assert!(!samples.is_empty(), "{label}");
    println!(
        "{label} p50_us={} p95_us={}",
        samples[(samples.len() - 1) / 2].as_micros(),
        samples[(samples.len() * 95).div_ceil(100) - 1].as_micros()
    );
}

// Run each cold start in a fresh process. The OS filesystem cache stays warm.
fn measure_reopening(path: &Path, games: usize) {
    let binary = std::env::current_exe().unwrap();
    let mut samples = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let output = Command::new(&binary)
            .args(["--exact", "open_existing_library", "--ignored"])
            .env("EIDORYN_BENCH_EXISTING_DB", path)
            .env("EIDORYN_BENCH_GAMES", games.to_string())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        samples.push(start.elapsed());
    }
    measure("process_reopen_and_list", samples);
}

#[test]
#[ignore = "invoked in a fresh process by library_scaling"]
fn open_existing_library() {
    let Some(path) = std::env::var_os("EIDORYN_BENCH_EXISTING_DB") else {
        return;
    };
    let expected: usize = std::env::var("EIDORYN_BENCH_GAMES")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        Library::open(Path::new(&path))
            .unwrap()
            .games()
            .unwrap()
            .len(),
        expected
    );
}

fn measure_warm_scraping(db: &Library, ids: &[i64]) {
    let mut samples = Vec::new();
    let start = Instant::now();
    let mut last = start;
    let result = scrape::run(
        db,
        ids,
        scrape::Mode::Incremental,
        Provider::Libretro,
        "",
        &AtomicBool::new(false),
        |n, _| {
            if n > samples.len() {
                let now = Instant::now();
                samples.push(now - last);
                last = now;
            }
        },
    );
    println!(
        "warm_scrape_ms={:.2} finished={} failed={}",
        start.elapsed().as_secs_f64() * 1000.,
        result.finished,
        result.failed
    );
    assert_eq!(
        (result.finished, result.failed, result.skipped),
        (ids.len(), 0, 0),
        "{:?}",
        result.errors
    );
    println!(
        "peak_queued_jobs={} max_in_flight={} writer_batches={} max_batch={}",
        2 * ids.len(),
        result.max_in_flight,
        result.writer_batches,
        result.max_batch
    );
    measure(
        "writer_commit",
        result
            .commit_micros
            .iter()
            .copied()
            .map(Duration::from_micros)
            .collect(),
    );
    measure("scrape_completion_interval", samples);
}

#[test]
#[ignore = "library performance fixture for 100, 500 and 2,000 local games"]
fn library_scaling() {
    let count: usize = std::env::var("EIDORYN_BENCH_GAMES")
        .ok()
        .map_or(500, |s| s.parse().unwrap());
    assert!((100..=2000).contains(&count));
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    fs::create_dir(&media).unwrap();
    for i in 0..count {
        let mut bytes = [0; 10240];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        fs::write(media.join(format!("Disc-{i:04}.iso")), bytes).unwrap();
    }
    let path = tmp.path().join("library.db");
    let start = Instant::now();
    let db = Library::open(&path).unwrap();
    println!(
        "games={count} cold_open_ms={:.2}",
        start.elapsed().as_secs_f64() * 1000.
    );
    let cancel = AtomicBool::new(false);
    for mode in ["cold_scan", "warm_scan"] {
        let start = Instant::now();
        let mut last = start;
        let mut samples = Vec::new();
        let scan = db
            .scan_directory(&media, false, &cancel, |r| {
                if r.added + r.existing > samples.len() {
                    let now = Instant::now();
                    samples.push(now - last);
                    last = now;
                }
            })
            .unwrap();
        assert_eq!(scan.added + scan.existing, count);
        assert_eq!(scan.failed, 0);
        println!("{mode}_ms={:.2}", start.elapsed().as_secs_f64() * 1000.);
        measure(mode, samples);
    }
    let mut samples = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        assert_eq!(db.games().unwrap().len(), count);
        samples.push(start.elapsed());
    }
    measure("read_library", samples);
    let mut conn = db.connect().unwrap();
    let ids: Vec<_> = db.games().unwrap().iter().map(|g| g.id).collect();
    let thumb = tmp.path().join("thumb.png");
    image::RgbImage::new(120, 160).save(&thumb).unwrap();
    conn.execute_batch("INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
        VALUES('Q1/SCUS-10000','SCUS-10000','USA','Performance fixture','1996','','');
        UPDATE game_metadata SET disc_serial='SCUS-10000',disc_region='USA';
        INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb)
        SELECT id,'download','SCUS-10000','https://raw.githubusercontent.com/libretro-thumbnails/Sony_-_PlayStation/master/Named_Boxarts/?serial=SCUS-10000&region=USA','thumb.png','thumb.png' FROM games;").unwrap();
    measure_warm_scraping(&db, &ids);
    for size in [1, 16, 32, 64] {
        let mut samples = Vec::new();
        for batch in ids.chunks(size) {
            let start = Instant::now();
            let tx = conn.transaction().unwrap();
            {
                let mut q = tx
                    .prepare_cached(
                        "UPDATE game_metadata SET refreshed=unixepoch() WHERE game_id=?",
                    )
                    .unwrap();
                for id in batch {
                    q.execute([id]).unwrap();
                }
            }
            tx.commit().unwrap();
            samples.push(start.elapsed());
        }
        measure(&format!("write_transaction_{size}"), samples);
    }
    let start = Instant::now();
    drop(conn);
    drop(db);
    println!("close_us={}", start.elapsed().as_micros());
    measure_reopening(&path, count);
    if let Ok(s) = fs::read_to_string("/proc/self/status")
        && let Some(rss) = s.lines().find(|l| l.starts_with("VmHWM:"))
    {
        println!("{rss}");
    }
}

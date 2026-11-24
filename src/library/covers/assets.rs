//! Immutable, content-addressed artwork shared by local and remote selections.
use super::{Library, MAX_PIXELS, RemoteKind, Source, THUMB_SIZE, check_cancel, record};
use crate::library::http_retry::RequestLimit;
use image::{ImageFormat, ImageReader};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Cursor,
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime},
};
use uuid::Uuid;

pub(super) struct Decoded {
    ext: &'static str,
    pub(super) thumb: image::DynamicImage,
    width: u32,
    height: u32,
}

// Downloads can overlap, but only two full-resolution images are decoded at a time.
static DECODERS: RequestLimit = RequestLimit::new(2, Duration::ZERO);

pub(super) fn decode_thumbnail(bytes: &[u8], cancel: &AtomicBool) -> Result<Decoded, String> {
    let _permit = DECODERS.acquire(cancel)?;
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    let ext = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        _ => return Err("Choose a JPEG or PNG cover".into()),
    };
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("Cover exceeds the 16 megapixel limit".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let thumb = reader
        .decode()
        .map_err(|e| e.to_string())?
        .thumbnail(THUMB_SIZE, THUMB_SIZE);
    Ok(Decoded {
        ext,
        thumb,
        width: w,
        height: h,
    })
}

pub(super) fn store(
    lib: &Library,
    id: i64,
    source: Source<'_>,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<bool, String> {
    check_cancel(cancel)?;
    let Decoded {
        ext,
        thumb,
        width,
        height,
    } = decode_thumbnail(bytes, cancel)?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let (kind, serial, attribution) = match source {
        Source::User { name, .. } => ("user", None, name),
        Source::Download { art, url } => (art.asset_kind(), Some(art.serial), url),
    };
    let folder = lib.base_dir().join("covers/objects");
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let tmp = tempfile::Builder::new()
        .prefix(".incoming-")
        .tempdir_in(&folder)
        .map_err(|e| e.to_string())?;
    let name = format!("original.{ext}");
    fs::write(tmp.path().join(&name), bytes).map_err(|e| e.to_string())?;
    thumb
        .save_with_format(tmp.path().join("thumb.png"), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    let relative = format!("covers/objects/{hash}");
    #[cfg(test)]
    interrupt_test_process("staged");
    let generation = lib.base_dir().join(&relative);
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if !still_selected(&tx, id, source).map_err(|e| e.to_string())? {
        return Ok(false);
    }
    crate::library::metadata::restore_display(&tx, id).map_err(|e| e.to_string())?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT original FROM cover_assets WHERE game_id=?1 AND kind=?2",
            params![id, kind],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb,content_hash,mime_type,width,height,downloaded_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,unixepoch())
        ON CONFLICT(game_id,kind) DO UPDATE SET serial=excluded.serial,source=excluded.source,
        original=excluded.original,thumb=excluded.thumb,content_hash=excluded.content_hash,
        mime_type=excluded.mime_type,width=excluded.width,height=excluded.height,downloaded_at=excluded.downloaded_at",
        params![id,kind,serial,attribution,format!("{relative}/{name}"),format!("{relative}/thumb.png"),
            hash, if ext == "png" {"image/png"} else {"image/jpeg"},width,height]).map_err(|e|e.to_string())?;
    if let Source::Download { art, url } = source {
        record(&tx, id, art, url, "ready", 0).map_err(|e| e.to_string())?;
    }
    check_cancel(cancel)?;
    if let Source::Download { art, .. } = source
        && let RemoteKind::Job { claim, .. } = art.kind
    {
        claim.finish_cover(&tx)?;
    }
    // The SQLite writer lock serializes publication with reference checks and garbage collection.
    // Only short renames occur here. Decode, hash and image writes precede this transaction.
    if generation.is_dir() {
        for file in [name.as_str(), "thumb.png"] {
            if !generation.join(file).is_file() {
                fs::rename(tmp.path().join(file), generation.join(file))
                    .map_err(|e| e.to_string())?;
            }
        }
    } else {
        fs::rename(tmp.path(), &generation).map_err(|e| e.to_string())?;
    }
    #[cfg(test)]
    interrupt_test_process("published");
    tx.commit().map_err(|e| e.to_string())?;
    #[cfg(test)]
    interrupt_test_process("committed");
    if let Some(original) = previous
        && !original.starts_with(&format!("{relative}/"))
    {
        remove_unreferenced(lib, &original)?;
    }
    Ok(true)
}

// The same write transaction verifies the selection and publishes the reference.
fn still_selected(
    db: &rusqlite::Connection,
    id: i64,
    source: Source<'_>,
) -> rusqlite::Result<bool> {
    match source {
        Source::User { revision, .. } => db.query_row(
            "SELECT cover_revision=?2 FROM game_metadata WHERE game_id=?1",
            params![id, revision],
            |r| r.get(0),
        ),
        Source::Download { art, .. } => {
            let revision = match art.kind {
                RemoteKind::Chosen { revision } => Some(revision),
                _ => None,
            };
            db.query_row(
                "SELECT cache_generation=?2 AND (?5 IS NULL OR release_id IS NULL OR release_id=?5)
                 AND CASE WHEN ?3 IS NULL THEN
                    COALESCE(release_serial,disc_serial) IS ?4 ELSE cover_revision=?3 END
                 FROM game_metadata WHERE game_id=?1",
                params![id, art.generation, revision, art.serial, art.release],
                |r| r.get(0),
            )
        }
    }
}

#[cfg(test)]
fn interrupt_test_process(point: &str) {
    if std::env::var("EIDORYN_TEST_COVER_CRASH").as_deref() == Ok(point) {
        std::process::exit(73);
    }
}

fn managed_directory(parent: &Path) -> bool {
    let Some(name) = parent.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match parent.parent().and_then(Path::to_str) {
        Some("covers/objects") => name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()),
        Some("covers/user" | "covers/download") => Uuid::parse_str(name).is_ok(),
        _ => false,
    }
}

// Earlier schemas referenced UUID directories directly. A pre-upgrade database
// backup must continue to resolve those paths even after a new full refresh.
fn has_legacy_backup(lib: &Library) -> std::io::Result<bool> {
    let Some(name) = lib.path.file_name() else {
        return Ok(false);
    };
    for entry in fs::read_dir(lib.base_dir())? {
        let entry = entry?;
        let candidate = entry.file_name();
        let suffix = candidate
            .as_encoded_bytes()
            .strip_prefix(name.as_encoded_bytes())
            .and_then(|s| std::str::from_utf8(s).ok())
            .and_then(|s| s.strip_prefix(".v"))
            .and_then(|s| s.strip_suffix(".bak"));
        if let Some(suffix) = suffix {
            let (version, archived) = suffix
                .split_once('.')
                .map_or((suffix, None), |(v, id)| (v, Some(id)));
            if matches!(version, "1" | "2" | "3" | "4")
                && archived.is_none_or(|id| Uuid::parse_str(id).is_ok())
                && entry.file_type()?.is_file()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn remove_unreferenced(lib: &Library, original: &str) -> Result<bool, String> {
    remove_unreferenced_with(lib, &mut lib.connect()?, original)
}

fn remove_unreferenced_with(
    lib: &Library,
    db: &mut Connection,
    original: &str,
) -> Result<bool, String> {
    let Some(parent) = Path::new(original)
        .parent()
        .filter(|p| managed_directory(p))
    else {
        return Ok(false);
    };
    if !parent.starts_with("covers/objects") && has_legacy_backup(lib).map_err(|e| e.to_string())? {
        return Ok(false);
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let referenced = tx
        .prepare_cached("SELECT 1 FROM cover_assets WHERE original IN (?1,?2)")
        .and_then(|mut q| {
            q.exists(params![
                parent
                    .join("original.jpg")
                    .to_string_lossy()
                    .replace('\\', "/"),
                parent
                    .join("original.png")
                    .to_string_lossy()
                    .replace('\\', "/")
            ])
        })
        .map_err(|e| e.to_string())?;
    if referenced {
        return Ok(false);
    }
    match fs::remove_dir_all(lib.base_dir().join(parent)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Release only artwork detached by this operation. A concurrent or shared reference wins.
pub(in crate::library) fn collect_released(
    lib: &Library,
    db: &mut Connection,
    originals: &std::collections::BTreeSet<String>,
) {
    for original in originals {
        // The database commit already detached the artwork. Orphan collection retries
        // on startup if the operating system temporarily prevents deleting the files.
        let _ = remove_unreferenced_with(lib, db, original);
    }
}

/// Remove abandoned artwork after a crash. Unknown or referenced paths are preserved.
pub fn collect_orphans(lib: &Library) -> Result<usize, String> {
    collect_at(lib, SystemTime::now())
}

pub(super) fn collect_at(lib: &Library, now: SystemTime) -> Result<usize, String> {
    let conn = lib.connect()?;
    let referenced = conn
        .prepare("SELECT DISTINCT original FROM cover_assets")
        .and_then(|mut q| {
            q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|p| p.rsplit_once('/').map(|(dir, _)| dir.to_owned()))
        .collect::<std::collections::HashSet<_>>();
    let mut removed = 0;
    for folder in ["objects", "user", "download"] {
        let relative = format!("covers/{folder}");
        let entries = match fs::read_dir(lib.base_dir().join(&relative)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(".incoming-") && folder == "objects" {
                // A live process may still be staging a download outside a transaction.
                if entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .is_some_and(|age| age >= Duration::from_hours(24))
                {
                    fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
                    removed += 1;
                }
            } else if !referenced.contains(&format!("{relative}/{name}"))
                && remove_unreferenced(lib, &format!("{relative}/{name}/original.png"))?
            {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

//! Opt-in cover retrieval and atomic local storage. The UI schedules this work off-thread.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

pub use assets::collect_orphans;
use assets::{decode_thumbnail, remove_unreferenced, store};
use image::ImageFormat;
use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, params};

#[cfg(test)]
use std::io::Cursor;

use super::{
    Library,
    http_retry::{RETRY_SECONDS, RequestLimit, retry_deadline, seconds},
    scrape::Mode,
    transport,
};

mod assets;
mod igdb;
mod libretro;
mod screenscraper;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Libretro,
    ScreenScraper,
    Igdb,
    Custom,
}

enum Lookup {
    Image(Url),
    Http(http::Response<Vec<u8>>),
    Missing,
}

pub const DEFAULT_TEMPLATE: &str =
    "https://raw.githubusercontent.com/xlenore/psx-covers/main/covers/default/${serial}.jpg";

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const THUMB_SIZE: u32 = 256;
const NOT_FOUND_SECONDS: i64 = 86_400;

// ScreenScraper's standard account allowance is shared by batch and individual lookups.
static SCREENSCRAPER_REQUESTS: RequestLimit = RequestLimit::new(1, Duration::ZERO);

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Cancelled".into())
    } else {
        Ok(())
    }
}

fn read_bounded(mut reader: impl Read, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut buffer = [0; 16 * 1024];
    let mut bytes = Vec::new();
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(bytes);
        }
        if count > MAX_BYTES - bytes.len() {
            return Err("Cover exceeds the 8 MiB limit".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

#[derive(Clone, Copy)]
enum Remote<'a> {
    Matching(&'a str),
    Refresh(&'a str),
    Job {
        serial: &'a str,
        full: bool,
        claim: &'a super::scrape::CoverClaim,
    },
    Chosen {
        serial: &'a str,
        revision: i64,
    },
}

impl<'a> Remote<'a> {
    fn serial(self) -> &'a str {
        match self {
            Self::Matching(serial)
            | Self::Refresh(serial)
            | Self::Job { serial, .. }
            | Self::Chosen { serial, .. } => serial,
        }
    }

    fn full(self) -> bool {
        matches!(self, Self::Refresh(_) | Self::Job { full: true, .. })
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Matching(_) | Self::Refresh(_) | Self::Job { .. } => "download",
            Self::Chosen { .. } => "user",
        }
    }
}

#[derive(Clone, Copy)]
enum Source<'a> {
    User { name: &'a str, revision: i64 },
    Download { art: Remote<'a>, url: &'a str },
}

fn reserve_selection(conn: &Connection, id: i64) -> Result<i64, String> {
    conn.query_row(
        "UPDATE game_metadata SET cover_revision=cover_revision+1
        WHERE game_id=?1 RETURNING cover_revision",
        [id],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn set_local(lib: &Library, id: i64, path: &Path) -> Result<(), String> {
    let cancel = AtomicBool::new(false);
    let revision = reserve_selection(&lib.connect()?, id)?;
    let bytes = read_bounded(File::open(path).map_err(|e| e.to_string())?, &cancel)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    store(
        lib,
        id,
        Source::User {
            name: &name,
            revision,
        },
        &bytes,
        &cancel,
    )
    .map(drop)
}

pub fn reset_local(lib: &Library, id: i64) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    reserve_selection(&tx, id)?;
    let previous: Option<String> = tx
        .query_row(
            "DELETE FROM cover_assets WHERE game_id=?1 AND kind='user' RETURNING original",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    if let Some(previous) = previous {
        remove_unreferenced(lib, &previous)?;
    }
    Ok(())
}

/// Rebuild a display thumbnail from its original, including while offline.
pub fn regenerate(lib: &Library, id: i64) -> Result<(), String> {
    let cached = lib.connect()?.query_row(
        "SELECT original,thumb FROM cover_assets WHERE game_id=?1
         AND (kind='user' OR serial=(SELECT COALESCE(release_serial,disc_serial) FROM game_metadata WHERE game_id=?1))
         ORDER BY kind='user' DESC LIMIT 1", [id],
        |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?))
    ).optional().map_err(|e|e.to_string())?;
    if let Some((original, thumb)) = cached {
        restore_thumbnail(lib, &original, &thumb, &AtomicBool::new(false))?;
    }
    Ok(())
}

// Regenerate in the immutable asset directory. Concurrent replacement of the
// selection cannot restore stale art or turn downloaded art into a user override.
fn restore_thumbnail(
    lib: &Library,
    original: &str,
    thumb: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let thumb = lib.base_dir().join(thumb);
    if thumb.is_file() {
        return Ok(());
    }
    let bytes = read_bounded(
        File::open(lib.base_dir().join(original)).map_err(|e| e.to_string())?,
        cancel,
    )?;
    let image = decode_thumbnail(&bytes, cancel)?.thumb;
    let mut tmp = tempfile::NamedTempFile::new_in(thumb.parent().ok_or("Missing asset directory")?)
        .map_err(|e| e.to_string())?;
    image
        .write_to(tmp.as_file_mut(), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    tmp.persist(&thumb).map_err(|e| e.to_string())?;
    Ok(())
}

/// Check a user-supplied template with the same URL rules as a cover download.
pub fn valid_template(template: &str) -> bool {
    cover_url(template, "SLUS-00000").is_ok()
}

fn cover_url(template: &str, serial: &str) -> Result<Url, String> {
    if !template.contains("${serial}")
        || serial.is_empty()
        || !serial
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err("Use an HTTPS cover URL containing ${serial}".into());
    }
    let url = Url::parse(&template.replace("${serial}", serial)).map_err(|e| e.to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.password().is_some()
        || !url.username().is_empty()
    {
        return Err("Use an HTTPS cover URL without credentials".into());
    }
    Ok(url)
}

fn record(
    conn: &Connection,
    id: i64,
    serial: &str,
    url: &str,
    state: &str,
    retry: i64,
) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO cover_attempts(game_id,serial,url,state,retry_after,updated) VALUES(?1,?2,?3,?4,?5,unixepoch())
        ON CONFLICT(game_id,serial,url) DO UPDATE SET state=excluded.state,retry_after=excluded.retry_after,updated=excluded.updated",
        params![id,serial,url,state,retry])?;
    Ok(())
}

pub fn fetch(
    lib: &Library,
    id: i64,
    provider: Provider,
    template: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    fetch_edition(
        lib,
        id,
        provider,
        template,
        None,
        Mode::Incremental,
        cancel,
        None,
    )
}

/// Assign art from a user-selected edition while retaining the game's metadata match.
pub fn fetch_release(
    lib: &Library,
    id: i64,
    provider: Provider,
    template: &str,
    release: &super::metadata::Release,
    cancel: &AtomicBool,
) -> Result<String, String> {
    fetch_edition(
        lib,
        id,
        provider,
        template,
        Some(release),
        Mode::Incremental,
        cancel,
        None,
    )
}

pub(super) fn scrape_claim(
    lib: &Library,
    id: i64,
    provider: Provider,
    template: &str,
    mode: Mode,
    cancel: &AtomicBool,
    claim: &super::scrape::CoverClaim,
) -> Result<String, String> {
    fetch_edition(lib, id, provider, template, None, mode, cancel, Some(claim))
}

#[allow(clippy::too_many_arguments)]
fn fetch_edition(
    lib: &Library,
    id: i64,
    provider: Provider,
    template: &str,
    chosen: Option<&super::metadata::Release>,
    mode: Mode,
    cancel: &AtomicBool,
    claim: Option<&super::scrape::CoverClaim>,
) -> Result<String, String> {
    check_cancel(cancel)?;
    let conn = lib.worker_connection()?;
    // A local filename and manual display title are private. Only send a catalog
    // title to a title-based service, or a verified disc serial to a serial-based one.
    let (serial, title, region) = conn.query_row(
        "SELECT COALESCE(release_serial,disc_serial),auto_title,COALESCE(auto_region,disc_region)
         FROM game_metadata WHERE game_id=?",
        [id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
    ).map_err(|e|e.to_string())?;
    let title = chosen
        .map(|r| r.title.as_str())
        .or(title.as_deref())
        .unwrap_or("");
    let region = chosen
        .map(|r| r.region.as_str())
        .or(region.as_deref())
        .unwrap_or("");
    let serial = chosen
        .map(|r| r.serial.as_str())
        .or(serial.as_deref())
        .ok_or("A disc serial or a selected release is needed to get cover art")?;
    if provider == Provider::Igdb && title.is_empty() {
        return Err("Choose a release or fetch its release details before searching IGDB".into());
    }
    let art = if chosen.is_some() {
        Remote::Chosen {
            serial,
            revision: reserve_selection(&conn, id)?,
        }
    } else if let Some(claim) = claim {
        Remote::Job {
            serial,
            full: mode == Mode::Full,
            claim,
        }
    } else if mode == Mode::Full {
        Remote::Refresh(serial)
    } else {
        Remote::Matching(serial)
    };
    drop(conn);
    let key = provider_key(provider, template, serial, title, region)?;
    let _permit = (provider == Provider::ScreenScraper)
        .then(|| SCREENSCRAPER_REQUESTS.acquire(cancel))
        .transpose()?;
    fetch_url(lib, id, art, &key, cancel, || {
        let client = transport::api().map_err(|e| e.to_string())?;
        let result = match provider {
            Provider::Libretro => libretro::resolve(lib, client, serial, region, cancel)?,
            Provider::ScreenScraper => screenscraper::resolve(client, serial, region, cancel)?,
            Provider::Igdb => igdb::resolve(client, title, region, cancel)?,
            Provider::Custom => Lookup::Image(cover_url(template, serial)?),
        };
        check_cancel(cancel)?;
        match result {
            Lookup::Missing => Ok(None),
            Lookup::Http(response) => Ok(Some(response)),
            Lookup::Image(image) if provider == Provider::ScreenScraper => {
                screenscraper::fetch_image(client, image, cancel)
            }
            Lookup::Image(image) => {
                let client = transport::art().map_err(|e| e.to_string())?;
                transport::cover(client.get(image), MAX_BYTES, cancel)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
        }
    })
}

// Include the complete URL when identifying an asset, while persisting only its
// origin and digest. CDN signatures and credentials may occur in paths or queries.
fn private_key(mut url: Url) -> Url {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(url.as_str().as_bytes());
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_path(&format!("/.eidoryn/cover/{digest:x}"));
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Remove old custom source URLs from active cache records when upgrading.
pub(super) fn migrate_private_sources(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare(
        "SELECT source FROM cover_assets WHERE kind='download' OR serial IS NOT NULL UNION SELECT url FROM cover_attempts"
    )?;
    let old = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for source in old {
        let Ok(url) = Url::parse(&source) else {
            continue;
        };
        let private = url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url
                .path()
                .strip_prefix("/.eidoryn/cover/")
                .is_some_and(|hash| {
                    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                });
        if private
            || url.fragment().is_none()
                && [
                    libretro::BOXART,
                    "https://api.screenscraper.fr/api2/jeuInfos.php",
                    "https://api.igdb.com/v4/games",
                ]
                .iter()
                .any(|endpoint| {
                    source
                        .split_once('?')
                        .is_some_and(|(base, _)| base == *endpoint)
                        && url
                            .query_pairs()
                            .all(|(k, _)| matches!(k.as_ref(), "serial" | "region" | "title"))
                })
        {
            continue;
        }
        let key = private_key(url);
        tx.execute(
            "UPDATE cover_assets SET source=?1 WHERE source=?2 AND (kind='download' OR serial IS NOT NULL)",
            [key.as_str(), &source],
        )?;
        tx.execute(
            "UPDATE OR IGNORE cover_attempts SET url=?1 WHERE url=?2",
            [key.as_str(), &source],
        )?;
        tx.execute("DELETE FROM cover_attempts WHERE url=?", [&source])?;
    }
    Ok(())
}

// The database key identifies the public source and edition, never a signed media URL.
pub(super) fn provider_key(
    provider: Provider,
    template: &str,
    serial: &str,
    title: &str,
    region: &str,
) -> Result<Url, String> {
    let base = match provider {
        Provider::Libretro => libretro::BOXART,
        Provider::ScreenScraper => "https://api.screenscraper.fr/api2/jeuInfos.php",
        Provider::Igdb => "https://api.igdb.com/v4/games",
        Provider::Custom => return Ok(private_key(cover_url(template, serial)?)),
    };
    let mut url = Url::parse(base).map_err(|e| e.to_string())?;
    let mut query = url.query_pairs_mut();
    query
        .append_pair("serial", serial)
        .append_pair("region", region);
    if provider == Provider::Igdb {
        query.append_pair("title", title);
    }
    drop(query);
    Ok(url)
}

fn fetch_url(
    lib: &Library,
    id: i64,
    art: Remote<'_>,
    url: &Url,
    cancel: &AtomicBool,
    request: impl FnOnce() -> Result<Option<http::Response<Vec<u8>>>, String>,
) -> Result<String, String> {
    let serial = art.serial();
    let conn = lib.worker_connection()?;
    let now = seconds(SystemTime::now());
    if !art.full() {
        let cached = conn
            .query_row(
                "SELECT original,thumb FROM cover_assets
             WHERE game_id=?1 AND kind=?4 AND serial=?2 AND source=?3",
                params![id, serial, url.as_str(), art.kind()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((original, thumb)) = cached {
            if lib.base_dir().join(&thumb).is_file() {
                return Ok("ready".into());
            }
            if lib.base_dir().join(&original).is_file() {
                restore_thumbnail(lib, &original, &thumb, cancel)?;
                return Ok("ready".into());
            }
        }
        let state: Option<String> = conn
            .query_row(
                "SELECT state FROM cover_attempts
             WHERE game_id=?1 AND serial=?2 AND url=?3 AND retry_after>?4",
                params![id, serial, url.as_str(), now],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(state) = state
            && !(matches!(art, Remote::Job { .. }) && state == "failed")
        {
            // The durable scheduler has already honored its own retry deadline.
            return Ok(state);
        }
    }
    let host = url.host_str().ok_or("Missing cover source host")?;
    let limited: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cover_rate_limits WHERE host=?1 AND retry_after>?2)",
            params![host, now],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if limited {
        record(&conn, id, serial, url.as_str(), "retry", 0).map_err(|e| e.to_string())?;
        return Ok("retry".into());
    }
    check_cancel(cancel)?;
    let result = request().and_then(|resolved| {
        if let Some(response) = resolved {
            download(lib, id, art, url, &response, cancel, &conn)
        } else {
            record(
                &conn,
                id,
                serial,
                url.as_str(),
                "not_found",
                now + NOT_FOUND_SECONDS,
            )
            .map_err(|e| e.to_string())?;
            Ok("not_found".into())
        }
    });
    if result.is_err() && !cancel.load(Ordering::Relaxed) {
        record(
            &conn,
            id,
            serial,
            url.as_str(),
            "failed",
            seconds(SystemTime::now()).saturating_add(RETRY_SECONDS),
        )
        .map_err(|e| e.to_string())?;
    }
    result
}

fn download(
    lib: &Library,
    id: i64,
    art: Remote<'_>,
    url: &Url,
    response: &http::Response<Vec<u8>>,
    cancel: &AtomicBool,
    conn: &Connection,
) -> Result<String, String> {
    check_cancel(cancel)?;
    let now = seconds(SystemTime::now());
    let serial = art.serial();
    match response.status().as_u16() {
        404 | 410 => {
            record(
                conn,
                id,
                serial,
                url.as_str(),
                "not_found",
                now + NOT_FOUND_SECONDS,
            )
            .map_err(|e| e.to_string())?;
            Ok("not_found".into())
        }
        _ if super::http_retry::should_defer(response.status(), response.headers()) => {
            let until = retry_deadline(
                response
                    .headers()
                    .get("Retry-After")
                    .and_then(|h| h.to_str().ok()),
                now,
            );
            let host = url.host_str().ok_or("Missing cover source host")?;
            conn.execute("INSERT INTO cover_rate_limits(host,retry_after) VALUES(?1,?2) ON CONFLICT(host) DO UPDATE SET retry_after=MAX(retry_after,excluded.retry_after)",params![host,until]).map_err(|e|e.to_string())?;
            record(conn, id, serial, url.as_str(), "retry", until).map_err(|e| e.to_string())?;
            Ok("retry".into())
        }
        status if !(200..300).contains(&status) => {
            Err(format!("Cover source returned HTTP {status}"))
        }
        _ => {
            if response.body().len() > MAX_BYTES
                || response
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
                    .is_some_and(|n| n > MAX_BYTES as u64)
            {
                return Err("Cover exceeds the 8 MiB limit".into());
            }
            let declared = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .map(|value| {
                    value
                        .to_str()
                        .unwrap_or("invalid")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase()
                });
            let bytes = response.body();
            if let Some(mime) = declared
                && !mime.is_empty()
                && mime != "application/octet-stream"
                && !matches!(
                    (mime.as_str(), image::guess_format(bytes)),
                    ("image/png", Ok(ImageFormat::Png)) | ("image/jpeg", Ok(ImageFormat::Jpeg))
                )
            {
                return Err("Cover Content-Type does not match a JPEG or PNG image".into());
            }
            let applied = store(
                lib,
                id,
                Source::Download {
                    art,
                    url: url.as_str(),
                },
                bytes,
                cancel,
            )?;
            Ok(if applied { "ready" } else { "superseded" }.into())
        }
    }
}

#[cfg(test)]
mod tests;

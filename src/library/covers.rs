//! Opt-in cover retrieval and atomic local storage. The UI schedules this work off-thread.

use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::{ImageFormat, ImageReader};
use reqwest::Url;
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::Library;

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
    Http(reqwest::blocking::Response),
    Missing,
}

pub const DEFAULT_TEMPLATE: &str =
    "https://raw.githubusercontent.com/xlenore/psx-covers/main/covers/default/${serial}.jpg";

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const THUMB_SIZE: u32 = 256;
const NOT_FOUND_SECONDS: i64 = 86_400;
const RETRY_SECONDS: i64 = 300;

fn seconds(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

fn retry_deadline(header: Option<&str>, now: i64) -> i64 {
    header
        .and_then(|h| {
            h.parse::<u64>()
                .ok()
                .map(|s| now.saturating_add(i64::try_from(s).unwrap_or(i64::MAX)))
                .or_else(|| httpdate::parse_http_date(h).ok().map(seconds))
        })
        .unwrap_or_else(|| now.saturating_add(RETRY_SECONDS))
        .max(now)
}

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
    Chosen { serial: &'a str, revision: i64 },
}

impl<'a> Remote<'a> {
    fn serial(self) -> &'a str {
        match self {
            Self::Matching(serial) | Self::Chosen { serial, .. } => serial,
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Matching(_) => "download",
            Self::Chosen { .. } => "user",
        }
    }
}

#[derive(Clone, Copy)]
enum Source<'a> {
    User { name: &'a str, revision: i64 },
    Download { art: Remote<'a>, url: &'a str },
}

fn decode_thumbnail(bytes: &[u8]) -> Result<(&'static str, image::DynamicImage), String> {
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
    Ok((ext, thumb))
}

fn store(
    lib: &Library,
    id: i64,
    source: Source<'_>,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<bool, String> {
    check_cancel(cancel)?;
    let (ext, thumb) = decode_thumbnail(bytes)?;
    let (kind, serial, attribution) = match source {
        Source::User { name, .. } => ("user", None, name),
        Source::Download { art, url } => (art.kind(), Some(art.serial()), url),
    };
    let folder = lib.base_dir().join("covers").join(kind);
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let tmp = tempfile::tempdir_in(&folder).map_err(|e| e.to_string())?;
    let name = format!("original.{ext}");
    fs::write(tmp.path().join(&name), bytes).map_err(|e| e.to_string())?;
    thumb
        .save_with_format(tmp.path().join("thumb.png"), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    // Immutable generation names let in-flight readers finish while a replacement is prepared.
    let relative = format!("covers/{kind}/{}", Uuid::new_v4());
    let generation = lib.base_dir().join(&relative);
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    // Only the most recent explicit selection may replace the user asset.
    // The write transaction keeps the check and the metadata swap indivisible.
    if let Source::User { revision, .. }
    | Source::Download {
        art: Remote::Chosen { revision, .. },
        ..
    } = source
        && !tx
            .query_row(
                "SELECT cover_revision=?2 FROM game_metadata WHERE game_id=?1",
                params![id, revision],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?
    {
        return Ok(false);
    }
    let previous: Option<String> = tx
        .query_row(
            "SELECT original FROM cover_assets WHERE game_id=?1 AND kind=?2",
            params![id, kind],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO cover_assets(game_id,kind,serial,source,original,thumb) VALUES(?1,?2,?3,?4,?5,?6)
        ON CONFLICT(game_id,kind) DO UPDATE SET serial=excluded.serial,source=excluded.source, original=excluded.original,thumb=excluded.thumb",
        params![id,kind,serial,attribution,format!("{relative}/{name}"),format!("{relative}/thumb.png")]).map_err(|e|e.to_string())?;
    if let Source::Download { art, url } = source {
        record(&tx, id, art.serial(), url, "ready", 0).map_err(|e| e.to_string())?;
    }
    check_cancel(cancel)?;
    fs::rename(tmp.path(), &generation).map_err(|e| e.to_string())?;
    if let Err(e) = tx.commit() {
        let _ = fs::remove_dir_all(&generation);
        return Err(e.to_string());
    }
    if let Some(previous) = previous {
        remove_generation(lib, kind, &previous);
    }
    Ok(true)
}

fn remove_generation(lib: &Library, kind: &str, original: &str) {
    // Only remove a UUID directory we generated, even when a database was restored by hand.
    let Some(parent) = Path::new(original).parent() else {
        return;
    };
    if parent.parent() == Some(Path::new("covers").join(kind).as_path())
        && parent
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|v| Uuid::parse_str(v).is_ok())
    {
        let _ = fs::remove_dir_all(lib.base_dir().join(parent));
    }
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
        remove_generation(lib, "user", &previous);
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
    let (_, image) = decode_thumbnail(&bytes)?;
    let mut tmp = tempfile::NamedTempFile::new_in(thumb.parent().ok_or("Missing asset directory")?)
        .map_err(|e| e.to_string())?;
    image
        .write_to(tmp.as_file_mut(), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    tmp.persist(&thumb).map_err(|e| e.to_string())?;
    Ok(())
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
    fetch_edition(lib, id, provider, template, None, cancel)
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
    fetch_edition(lib, id, provider, template, Some(release), cancel)
}

fn fetch_edition(
    lib: &Library,
    id: i64,
    provider: Provider,
    template: &str,
    chosen: Option<&super::metadata::Release>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    check_cancel(cancel)?;
    let conn = lib.connect()?;
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
        return Err(
            "Choose a release or fetch its title from Wikidata before searching IGDB".into(),
        );
    }
    let art = if chosen.is_some() {
        Remote::Chosen {
            serial,
            revision: reserve_selection(&conn, id)?,
        }
    } else {
        Remote::Matching(serial)
    };
    let key = provider_key(provider, template, serial, title, region)?;
    fetch_url(lib, id, art, &key, cancel, || {
        let client = http_client(reqwest::redirect::Policy::none())?;
        let result = match provider {
            Provider::Libretro => libretro::resolve(lib, &client, serial, region, cancel)?,
            Provider::ScreenScraper => screenscraper::resolve(&client, serial, region)?,
            Provider::Igdb => igdb::resolve(&client, title, region)?,
            Provider::Custom => Lookup::Image(key.clone()),
        };
        check_cancel(cancel)?;
        match result {
            Lookup::Missing => Ok(None),
            Lookup::Http(response) => Ok(Some(response)),
            Lookup::Image(image) if provider == Provider::ScreenScraper => {
                screenscraper::fetch_image(&client, image)
            }
            Lookup::Image(image) => http_client(reqwest::redirect::Policy::limited(5))?
                .get(image)
                .send()
                .map(Some)
                .map_err(|e| e.without_url().to_string()),
        }
    })
}

fn http_client(redirect: reqwest::redirect::Policy) -> Result<Client, String> {
    Client::builder()
        .https_only(true)
        .referer(false)
        .redirect(redirect)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(25))
        .user_agent(concat!("Eidoryn/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

// The database key identifies the public source and edition, never a signed media URL.
fn provider_key(
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
        Provider::Custom => return cover_url(template, serial),
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
    request: impl FnOnce() -> Result<Option<reqwest::blocking::Response>, String>,
) -> Result<String, String> {
    let serial = art.serial();
    let conn = lib.connect()?;
    let cached = conn.query_row(
        "SELECT original,thumb FROM cover_assets WHERE game_id=?1 AND kind=?4 AND serial=?2 AND source=?3",
        params![id, serial, url.as_str(), art.kind()], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))
    ).optional().map_err(|e|e.to_string())?;
    if let Some((original, thumb)) = cached {
        if lib.base_dir().join(&thumb).is_file() {
            return Ok("ready".into());
        }
        if lib.base_dir().join(&original).is_file() {
            restore_thumbnail(lib, &original, &thumb, cancel)?;
            return Ok("ready".into());
        }
    }
    let now = seconds(SystemTime::now());
    let state:Option<String>=conn.query_row("SELECT state FROM cover_attempts WHERE game_id=?1 AND serial=?2 AND url=?3 AND retry_after>?4",
        params![id,serial,url.as_str(),now],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if let Some(state) = state {
        return Ok(state);
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
            download(lib, id, art, url, response, cancel, &conn)
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
    response: reqwest::blocking::Response,
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
        429 => {
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
            if response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
            {
                return Err("Cover exceeds the 8 MiB limit".into());
            }
            let bytes = read_bounded(response, cancel)?;
            let applied = store(
                lib,
                id,
                Source::Download {
                    art,
                    url: url.as_str(),
                },
                &bytes,
                cancel,
            )?;
            Ok(if applied { "ready" } else { "superseded" }.into())
        }
    }
}

#[cfg(test)]
mod tests;

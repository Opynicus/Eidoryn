//! Libretro's thumbnails use playlist names. Resolve those names from the upstream
//! Redump DAT, indexed by serial in the user's library on the first cover request.
use std::sync::{Mutex, atomic::AtomicBool};
use std::time::SystemTime;

use crate::library::transport;
use reqwest::{Client, Url};
use rusqlite::{Connection, params};

use super::{Library, Lookup, MAX_BYTES, RETRY_SECONDS, check_cancel, seconds};

const DAT: &str = "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/redump/Sony%20-%20PlayStation.dat";
pub(super) const BOXART: &str = "https://raw.githubusercontent.com/libretro-thumbnails/Sony_-_PlayStation/master/Named_Boxarts/";
const REFRESH_SECONDS: i64 = 30 * 86_400;
static REFRESH: Mutex<()> = Mutex::new(());

pub(super) fn resolve(
    lib: &Library,
    client: &Client,
    serial: &str,
    region: &str,
    cancel: &AtomicBool,
) -> Result<Lookup, String> {
    let mut db = lib.connect()?;
    let now = seconds(SystemTime::now());
    // Serialize cold starts. Other downloads can use the index while the image is in flight.
    let guard = REFRESH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (last, retry): (Option<i64>, i64) = db
        .query_row(
            "SELECT sync_time,retry_after FROM libretro_catalog WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    if retry > now {
        return match cached(&db, serial, region)? {
            Lookup::Missing => {
                let mut retry_response = http::Response::new(Vec::<u8>::new());
                *retry_response.status_mut() = reqwest::StatusCode::SERVICE_UNAVAILABLE;
                retry_response
                    .headers_mut()
                    .insert(reqwest::header::RETRY_AFTER, (retry - now).into());
                Ok(Lookup::Http(retry_response))
            }
            hit => Ok(hit),
        };
    }
    if last.is_none_or(|last| now.saturating_sub(last) >= REFRESH_SECONDS) {
        let refresh = (|| {
            let response = transport::request(client.get(DAT), MAX_BYTES, cancel)
                .map_err(|e| e.to_string())?;
            if crate::library::http_retry::should_defer(response.status(), response.headers()) {
                return Ok(Some(response)); // The common download path persists Retry-After for the host.
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Libretro catalog returned HTTP {}",
                    response.status().as_u16()
                ));
            }
            let bytes = response.into_body();
            let dat = std::str::from_utf8(&bytes).map_err(|_| "Invalid Libretro DAT encoding")?;
            replace_index(&mut db, dat, now, cancel)?;
            Ok(None)
        })();
        match refresh {
            Ok(Some(limited)) => return Ok(Lookup::Http(limited)),
            Ok(None) => {}
            Err(error) => {
                check_cancel(cancel)?;
                db.execute(
                    "UPDATE libretro_catalog SET retry_after=?1 WHERE id=1",
                    [now + RETRY_SECONDS],
                )
                .map_err(|e| e.to_string())?;
                return cached_after_failure(&db, serial, region, error);
            }
        }
    }
    drop(guard);
    cached(&db, serial, region)
}

// A stale positive match still identifies the asset. A stale miss is inconclusive.
fn cached_after_failure(
    db: &Connection,
    serial: &str,
    region: &str,
    error: String,
) -> Result<Lookup, String> {
    match cached(db, serial, region)? {
        Lookup::Missing => Err(error),
        hit => Ok(hit),
    }
}

fn field<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix('"')
}

fn replace_index(
    db: &mut Connection,
    dat: &str,
    now: i64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM libretro_boxart_names", [])
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    let mut in_game = false;
    let mut name = "";
    let mut serial = "";
    let mut region = "";
    {
        let mut insert = tx
            .prepare(
                "INSERT OR IGNORE INTO libretro_boxart_names(serial,region,name) VALUES(?1,?2,?3)",
            )
            .map_err(|e| e.to_string())?;
        for line in dat.lines().map(str::trim) {
            if line == "game (" {
                if in_game {
                    return Err("Incomplete Libretro DAT entry".into());
                }
                check_cancel(cancel)?;
                in_game = true;
                (name, serial, region) = ("", "", "");
            } else if in_game {
                if line == ")" {
                    if !name.is_empty() && !serial.is_empty() {
                        count += insert
                            .execute(params![serial, region, name])
                            .map_err(|e| e.to_string())?;
                    }
                    in_game = false;
                } else if let Some(value) = field(line, "name \"") {
                    name = value;
                } else if let Some(value) = field(line, "serial \"") {
                    serial = value;
                } else if let Some(value) = field(line, "region \"") {
                    region = value;
                }
            }
        }
    }
    if count == 0 || in_game {
        return Err("Empty or incomplete Libretro PS1 catalog".into());
    }
    check_cancel(cancel)?;
    tx.execute(
        "UPDATE libretro_catalog SET sync_time=?1,retry_after=0 WHERE id=1",
        [now],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn matches_region(dat: &str, region: &str) -> bool {
    region.is_empty()
        || dat.split(" / ").any(|part| {
            part == region || region == "Europe" && crate::library::european_market(part)
        })
}

fn cached(db: &Connection, serial: &str, region: &str) -> Result<Lookup, String> {
    let mut stmt = db
        .prepare("SELECT name,region FROM libretro_boxart_names WHERE serial=?1 ORDER BY name")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([serial]).map_err(|e| e.to_string())?;
    let mut selected = None;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let territory: String = row.get(1).map_err(|e| e.to_string())?;
        if !matches_region(&territory, region) {
            continue;
        }
        let name: String = row.get(0).map_err(|e| e.to_string())?;
        if selected.as_ref().is_some_and(|previous| previous != &name) {
            return Err("Multiple Libretro editions match this disc. Choose a different cover source or select a local image.".into());
        }
        selected = Some(name);
    }
    let Some(name) = selected else {
        return Ok(Lookup::Missing);
    };
    let filename = name.replace(
        ['&', '*', '/', ':', '`', '<', '>', '?', '\\', '|', '"'],
        "_",
    );
    let mut url = Url::parse(BOXART).map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "Invalid Libretro thumbnail endpoint")?
        .pop_if_empty()
        .push(&format!("{filename}.png"));
    Ok(Lookup::Image(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_only_disc_fields_and_never_guesses_between_editions() {
        let (_tmp, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        let cancel = AtomicBool::new(false);
        let one = r#"game (
            name "A & B: Demo (France)"
            region "France"
            serial "TEST-12345"
            rom ( name "unrelated" serial "TEST-99999" )
        )"#;
        replace_index(&mut db, one, 1, &cancel).unwrap();
        let Lookup::Image(url) = cached(&db, "TEST-12345", "Europe").unwrap() else {
            panic!("missing regional art")
        };
        assert!(url.path().ends_with("/A%20_%20B_%20Demo%20(France).png"));
        assert!(matches!(
            cached(&db, "TEST-12345", "USA").unwrap(),
            Lookup::Missing
        ));
        assert!(matches!(
            cached(&db, "TEST-99999", "").unwrap(),
            Lookup::Missing
        ));
        assert!(replace_index(&mut db, &format!("{one}\ngame ("), 2, &cancel).is_err());
        assert!(matches!(
            cached(&db, "TEST-12345", "Europe").unwrap(),
            Lookup::Image(_)
        ));
        db.execute(
            "UPDATE libretro_catalog SET retry_after=? WHERE id=1",
            [i64::MAX],
        )
        .unwrap();
        let client = transport::api().unwrap();
        assert!(matches!(
            resolve(&lib, client, "TEST-12345", "Europe", &cancel).unwrap(),
            Lookup::Image(_)
        ));
        assert!(
            matches!(resolve(&lib, client, "TEST-00000", "Europe", &cancel).unwrap(), Lookup::Http(response) if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE)
        );
        replace_index(
            &mut db,
            &format!("{one}\n{}", one.replace("Demo", "Demo (Rev 1)")),
            3,
            &cancel,
        )
        .unwrap();
        assert!(cached(&db, "TEST-12345", "Europe").is_err());
    }
}

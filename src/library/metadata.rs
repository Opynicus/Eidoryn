//! PlayStation release metadata in the user library and per-field user corrections.
mod wikidata;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::{Library, scrape::Mode};

pub(super) const SCHEMA: &str = include_str!("metadata/schema.sql");

const LIVE: &str = "Wikidata (online)";

#[derive(Clone, Debug)]
pub struct Release {
    pub id: String,
    pub serial: String,
    pub region: String,
    pub title: String,
    pub date: String,
    aliases: String,
    work_date: String,
}

#[derive(Debug)]
pub struct Metadata {
    pub title: Option<String>,
    pub region: Option<String>,
    pub date: Option<String>,
    pub aliases: Vec<String>,
    pub release_id: Option<String>,
    pub serial: Option<String>,
    pub thumbnail: Option<PathBuf>,
    pub cover_source: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub refreshed: Option<i64>,
    pub pending: bool,
    /// Sole work publication date, shown separately from an edition date.
    pub work_date: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum Field {
    Title,
    Region,
    Date,
}

fn matching_serial<'a>(
    rows: &'a [Release],
    serial: Option<&str>,
    region: Option<&str>,
) -> Option<&'a Release> {
    let serial = serial?;
    let mut exact = rows
        .iter()
        .filter(|r| r.serial == serial && region.is_none_or(|v| v == r.region));
    let found = exact.next()?;
    exact.next().is_none().then_some(found)
}

fn title_hint(title: &str) -> String {
    title
        .split(['(', '['])
        .next()
        .unwrap_or(title)
        .trim()
        .to_lowercase()
}

impl Release {
    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.title.as_str())
            .chain(self.aliases.split('|').filter(|name| !name.is_empty()))
    }

    fn suggested(&self, serial: Option<&str>, normalized_hint: &str) -> bool {
        Some(self.serial.as_str()) == serial
            || (!normalized_hint.is_empty()
                && self
                    .names()
                    .any(|name| name.to_lowercase() == normalized_hint))
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            serial: row.get(1)?,
            region: row.get(2)?,
            title: row.get(3)?,
            date: row.get(4)?,
            aliases: row.get(5)?,
            work_date: row.get(6)?,
        })
    }
}

fn cached_for(conn: &Connection, serial: &str) -> rusqlite::Result<Vec<Release>> {
    conn.prepare_cached(
        "SELECT id,serial,region,title,date,aliases,work_date
                         FROM fetched_releases WHERE serial=? ORDER BY id",
    )?
    .query_map([serial], Release::from_row)?
    .collect()
}

/// Seed metadata with evidence from the first disc. Grouping another disc must not
/// replace an edition the user has already identified.
pub(super) fn identify(
    tx: &Transaction<'_>,
    game_id: i64,
    serial: Option<&str>,
    region: Option<&str>,
    title: &str,
) -> rusqlite::Result<()> {
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO game_metadata(game_id,disc_serial,disc_region,filename)
         VALUES(?1,?2,?3,?4)",
        params![game_id, serial, region, title],
    )?;
    if inserted != 0 {
        match_evidence(tx, game_id, serial, region, title)?;
    }
    Ok(())
}

fn match_evidence(
    conn: &Connection,
    id: i64,
    serial: Option<&str>,
    region: Option<&str>,
    title: &str,
) -> rusqlite::Result<()> {
    let cached = serial
        .map(|s| cached_for(conn, s))
        .transpose()?
        .unwrap_or_default();
    if let Some(release) = matching_serial(&cached, serial, region) {
        assign(conn, id, &release.id, false)?;
    } else {
        // A title or alias is only a suggestion. Never assign an edition from a filename.
        let pending = !cached.is_empty() || has_title_candidate(conn, &title_hint(title))?;
        conn.execute(
            "UPDATE game_metadata SET pending=? WHERE game_id=?",
            params![pending, id],
        )?;
    }
    Ok(())
}

fn has_title_candidate(conn: &Connection, hint: &str) -> rusqlite::Result<bool> {
    if hint.is_empty() {
        return Ok(false);
    }
    let mut stmt = conn.prepare_cached("SELECT title,aliases FROM fetched_releases")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let title = row.get_ref(0)?.as_str()?;
        let aliases = row.get_ref(1)?.as_str()?;
        if std::iter::once(title)
            .chain(aliases.split('|'))
            .any(|n| n.to_lowercase() == hint)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn migrate_local_cache(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(include_str!("metadata/local_cache.sql"))?;
    // Unassigned name-only matches in older libraries could refer to an edition
    // the user never fetched or saved. Re-evaluate them against their own cache.
    let mut stmt = tx.prepare(
        "SELECT game_id,disc_serial,disc_region,filename FROM game_metadata
         WHERE pending AND NOT confirmed AND release_id IS NULL",
    )?;
    let pending = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, serial, region, title) in pending {
        match_evidence(tx, id, serial.as_deref(), region.as_deref(), &title)?;
    }
    Ok(())
}

pub(super) fn needs_match(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM game_metadata WHERE game_id=?1 AND pending)",
        [id],
        |r| r.get(0),
    )
}

fn assign(
    conn: &Connection,
    id: i64,
    release_id: &str,
    confirmed: bool,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE game_metadata SET release_id=r.id, release_serial=r.serial,
            auto_title=r.title, auto_region=r.region, auto_date=NULLIF(r.date,''),
            aliases=r.aliases, source=r.source, refreshed=unixepoch(), confirmed=?3,
            work_date=NULLIF(r.work_date,''), pending=0
         FROM fetched_releases AS r WHERE game_metadata.game_id=?1 AND r.id=?2",
        params![id, release_id, confirmed],
    )
}

struct Evidence {
    serial: Option<String>,
    region: Option<String>,
    selected: Option<String>,
    filename: String,
}

fn evidence(conn: &Connection, id: i64) -> rusqlite::Result<Evidence> {
    conn.query_row(
        "SELECT CASE WHEN confirmed THEN release_serial ELSE disc_serial END,
                CASE WHEN confirmed THEN auto_region ELSE disc_region END,
                CASE WHEN confirmed THEN release_id END, filename
         FROM game_metadata WHERE game_id=?",
        [id],
        |r| {
            Ok(Evidence {
                serial: r.get(0)?,
                region: r.get(1)?,
                selected: r.get(2)?,
                filename: r.get(3)?,
            })
        },
    )
}

pub fn refresh(lib: &Library, id: i64) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let evidence = evidence(&tx, id).map_err(|e| e.to_string())?;
    if let Some(key) = evidence.selected {
        assign(&tx, id, &key, true).map_err(|e| e.to_string())?;
    } else {
        match_evidence(
            &tx,
            id,
            evidence.serial.as_deref(),
            evidence.region.as_deref(),
            &evidence.filename,
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    Updated,
    Candidates,
    NotFound,
    NoSerial,
}

/// Fetch current release information only after an explicit online request.
pub fn refresh_online(lib: &Library, id: i64) -> Result<RefreshOutcome, String> {
    refresh_with(lib, id, wikidata::fetch)
}

fn refresh_with(
    lib: &Library,
    id: i64,
    fetch: impl FnOnce(&str) -> Result<wikidata::Download, String>,
) -> Result<RefreshOutcome, String> {
    apply_lookup(lib, id, |conn, serial| download_serial(conn, serial, fetch))
}

// A serial identifies the upstream lookup. Different game IDs can legitimately
// share it, while each keeps its own manual selection and region evidence.
type SerialLookup = OnceLock<Result<bool, String>>;

#[derive(Default)]
pub(super) struct Batch {
    lookups: Mutex<HashMap<String, Arc<SerialLookup>>>,
}

impl Batch {
    pub(super) fn refresh(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
    ) -> Result<RefreshOutcome, String> {
        self.refresh_with(lib, id, mode, cancel, |serial| {
            wikidata::fetch_cancellable(serial, cancel)
        })
    }

    fn refresh_with(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
        fetch: impl FnOnce(&str) -> Result<wikidata::Download, String>,
    ) -> Result<RefreshOutcome, String> {
        apply_lookup(lib, id, |conn, serial| {
            let lookup = self
                .lookups
                .lock()
                .map_err(|e| e.to_string())?
                .entry(serial.to_owned())
                .or_default()
                .clone();
            let found = lookup.get_or_init(|| {
                if mode == Mode::Incremental
                    && conn
                        .prepare_cached(
                            "SELECT 1 FROM fetched_releases WHERE serial=?1 AND source=?2",
                        )
                        .and_then(|mut q| q.exists([serial, LIVE]))
                        .map_err(|e| e.to_string())?
                {
                    return Ok(true);
                }
                download_serial(conn, serial, |s| {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Cancelled".into());
                    }
                    let releases = fetch(s)?;
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Cancelled".into());
                    }
                    Ok(releases)
                })
            });
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            found.clone()
        })
    }
}

fn download_serial(
    conn: &mut Connection,
    serial: &str,
    fetch: impl FnOnce(&str) -> Result<wikidata::Download, String>,
) -> Result<bool, String> {
    let revision: i64 = conn
        .query_row(
            "INSERT INTO metadata_requests(serial,revision) VALUES(?,1)
         ON CONFLICT(serial) DO UPDATE SET revision=revision+1 RETURNING revision",
            [serial],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let download = fetch(serial)?;
    if download.releases.is_empty() {
        return Ok(false);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let current: i64 = tx
        .query_row(
            "SELECT revision FROM metadata_requests WHERE serial=?",
            [serial],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if current != revision {
        return Err("A newer metadata request superseded this one".into());
    }
    cache_releases(&tx, serial, &download.releases).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO metadata_responses(serial,payload,fetched_at) VALUES(?1,?2,unixepoch())
         ON CONFLICT(serial) DO UPDATE SET payload=excluded.payload,fetched_at=excluded.fetched_at",
        params![serial, download.raw],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

fn apply_lookup(
    lib: &Library,
    id: i64,
    lookup: impl FnOnce(&mut Connection, &str) -> Result<bool, String>,
) -> Result<RefreshOutcome, String> {
    let mut conn = lib.connect()?;
    let Some(serial) = evidence(&conn, id).map_err(|e| e.to_string())?.serial else {
        return Ok(RefreshOutcome::NoSerial);
    };
    if !lookup(&mut conn, &serial)? {
        return Ok(RefreshOutcome::NotFound);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    // Another request may already have committed a newer version of these releases.
    let releases = cached_for(&tx, &serial).map_err(|e| e.to_string())?;
    // The user can correct the selected edition while the request is in flight.
    let evidence = evidence(&tx, id).map_err(|e| e.to_string())?;
    if evidence.serial.as_deref() != Some(&serial) {
        return Err("The selected edition changed during the metadata request".into());
    }
    let release = if let Some(key) = &evidence.selected {
        releases.iter().find(|r| &r.id == key)
    } else {
        matching_serial(&releases, Some(&serial), evidence.region.as_deref())
    };
    let outcome = if let Some(release) = release {
        assign(&tx, id, &release.id, evidence.selected.is_some()).map_err(|e| e.to_string())?;
        RefreshOutcome::Updated
    } else {
        tx.execute("UPDATE game_metadata SET pending=1 WHERE game_id=?", [id])
            .map_err(|e| e.to_string())?;
        RefreshOutcome::Candidates
    };
    tx.commit().map_err(|e| e.to_string())?;
    Ok(outcome)
}

fn cache_releases(
    tx: &Transaction<'_>,
    serial: &str,
    releases: &[Release],
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM fetched_releases WHERE serial=?", [serial])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO fetched_releases(id,serial,region,title,date,aliases,work_date)
                                        VALUES(?,?,?,?,?,?,?)",
    )?;
    for release in releases {
        insert.execute(params![
            release.id,
            serial,
            release.region,
            release.title,
            release.date,
            release.aliases,
            release.work_date
        ])?;
    }
    Ok(())
}

pub fn candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    let conn = lib.connect()?;
    let evidence = evidence(&conn, id).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id,serial,region,title,date,aliases,work_date FROM fetched_releases ORDER BY id")
        .map_err(|e| e.to_string())?;
    let needle = query.trim().to_lowercase();
    let hint = title_hint(&evidence.filename);
    let mut candidates = Vec::new();
    for row in stmt
        .query_map([], Release::from_row)
        .map_err(|e| e.to_string())?
    {
        let release = row.map_err(|e| e.to_string())?;
        let matches = if needle.is_empty() {
            release.suggested(evidence.serial.as_deref(), &hint)
        } else {
            release.serial.to_lowercase().contains(&needle)
                || release
                    .names()
                    .any(|name| name.to_lowercase().contains(&needle))
        };
        if matches {
            candidates.push(release);
        }
    }
    Ok(candidates)
}

/// Search the local release cache, querying Wikidata for an uncached, complete disc serial.
pub fn search_candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    search_with(lib, id, query, wikidata::fetch)
}

fn search_with(
    lib: &Library,
    id: i64,
    query: &str,
    fetch: impl FnOnce(&str) -> Result<wikidata::Download, String>,
) -> Result<Vec<Release>, String> {
    let serial = query.trim().to_ascii_uppercase();
    if wikidata::is_supported_serial(&serial) {
        let mut conn = lib.connect()?;
        let cached = conn
            .prepare_cached("SELECT 1 FROM fetched_releases WHERE serial=?")
            .and_then(|mut stmt| stmt.exists([&serial]))
            .map_err(|e| e.to_string())?;
        if !cached {
            download_serial(&mut conn, &serial, fetch)?;
        }
    }
    candidates(lib, id, query)
}

pub fn select(lib: &Library, id: i64, release_id: &str) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if assign(&tx, id, release_id, true).map_err(|e| e.to_string())? == 0 {
        return Err("Unknown game or cached release".into());
    }
    tx.commit().map_err(|e| e.to_string())
}

fn valid_date(date: &str) -> bool {
    if !date.is_ascii() {
        return false;
    }
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let fields: Vec<_> = date.split('-').collect();
    match fields.as_slice() {
        [year] => year.len() == 4 && digits(year) && *year != "0000",
        [year, month] => {
            valid_date(year)
                && month.len() == 2
                && digits(month)
                && month.parse::<u32>().is_ok_and(|m| (1..=12).contains(&m))
        }
        [year, month, day] => {
            year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && digits(year)
                && *year != "0000"
                && digits(month)
                && digits(day)
                && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
        }
        _ => false,
    }
}

pub fn set_override(
    lib: &Library,
    id: i64,
    field: Field,
    value: Option<&str>,
) -> Result<(), String> {
    let value = value.map(str::trim);
    if let Some(value) = value {
        if value.is_empty() || value.len() > 1024 {
            return Err("Enter 1 to 1024 bytes".into());
        }
        if matches!(field, Field::Date) && !valid_date(value) {
            return Err("Use YYYY, YYYY-MM or YYYY-MM-DD".into());
        }
    }
    let column = match field {
        Field::Title => "user_title",
        Field::Region => "user_region",
        Field::Date => "user_date",
    };
    lib.connect()?
        .execute(
            &format!("UPDATE game_metadata SET {column}=?2 WHERE game_id=?1"),
            params![id, value],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list(lib: &Library) -> Result<HashMap<i64, Metadata>, String> {
    let conn = lib.connect()?;
    let mut stmt=conn.prepare("SELECT m.game_id, COALESCE(user_title,auto_title), COALESCE(user_region,auto_region,disc_region), COALESCE(user_date,auto_date),
        release_id,COALESCE(release_serial,disc_serial),auto_title,aliases,COALESCE(u.thumb,c.thumb),COALESCE(u.source,c.source),
        COALESCE(a.state,CASE WHEN c.thumb IS NOT NULL THEN 'ready' END),m.source,m.refreshed,m.pending,m.work_date
        FROM game_metadata m LEFT JOIN cover_assets u ON u.game_id=m.game_id AND u.kind='user'
        LEFT JOIN cover_assets c ON c.game_id=m.game_id AND c.kind='download' AND c.serial=COALESCE(release_serial,disc_serial)
        LEFT JOIN cover_attempts a ON a.game_id=m.game_id AND a.serial=COALESCE(release_serial,disc_serial)
        AND a.rowid=(SELECT rowid FROM cover_attempts WHERE game_id=m.game_id AND serial=COALESCE(m.release_serial,m.disc_serial)
        ORDER BY updated DESC,rowid DESC LIMIT 1)").map_err(|e|e.to_string())?;
    stmt.query_map([], |r| {
        let mut aliases = Vec::new();
        if let Some(original) = r.get::<_, Option<String>>(6)? {
            aliases.push(original);
        }
        if let Some(names) = r.get::<_, Option<String>>(7)? {
            aliases.extend(
                names
                    .split('|')
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned),
            );
        }
        Ok((
            r.get(0)?,
            Metadata {
                title: r.get(1)?,
                region: r.get(2)?,
                date: r.get(3)?,
                release_id: r.get(4)?,
                serial: r.get(5)?,
                aliases,
                thumbnail: r
                    .get::<_, Option<String>>(8)?
                    .map(|p| lib.base_dir().join(p)),
                cover_source: r.get(9)?,
                status: r.get(10)?,
                source: r.get(11)?,
                refreshed: r.get(12)?,
                pending: r.get(13)?,
                work_date: r.get(14)?,
            },
        ))
    })
    .and_then(Iterator::collect)
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

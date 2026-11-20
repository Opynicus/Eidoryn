//! PlayStation release metadata in the user library and per-field user corrections.
use super::transport;
mod cache;
mod gamedb;
mod matching;
mod providers;
mod refresh;
mod wikidata;
pub(crate) use super::transport::ProviderError;
pub use cache::CacheScope;
pub(super) use cache::clear_in_transaction;
pub use matching::candidates;
use matching::{has_title_candidate, index_names, matching_serial, title_hint};
use providers::{is_release_serial, lookup_release};
use refresh::download_serial;
pub(super) use refresh::{Batch, Prepared};
pub use refresh::{RefreshOutcome, refresh_online, refresh_online_cancellable};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::Library;

pub(super) const SCHEMA: &str = include_str!("metadata/schema.sql");

const LIVE: &str = "Wikidata (online)";

#[derive(Clone, Copy, Debug)]
struct Provenance {
    provider: &'static str,
    payload_version: u32,
}

#[derive(Debug)]
struct SourceResponse {
    source: Provenance,
    releases: Vec<Release>,
    raw: Vec<u8>,
}

/// Returns complete release candidates for a trusted disc identifier. A source can
/// return several editions, which require an explicit choice.
trait MetadataProvider {
    const SOURCE: Provenance;

    fn search(&self, serial: &str, cancel: &AtomicBool) -> Result<SourceResponse, ProviderError>;
}

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
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent persisted release, cover and job states"
)]
pub struct Metadata {
    /// Hide retained user data and suppress automatic lookups after a cache clear.
    pub cache_cleared: bool,
    pub title: Option<String>,
    pub region: Option<String>,
    pub date: Option<String>,
    pub aliases: Vec<String>,
    pub release_id: Option<String>,
    pub release_title: Option<String>,
    /// Overrides in title, region, release date order.
    pub manual_fields: [bool; 3],
    pub manual_cover: bool,
    pub serial: Option<String>,
    pub thumbnail: Option<PathBuf>,
    pub cover_source: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub refreshed: Option<i64>,
    pub pending: bool,
    /// The user can cancel a durable queued, running, or deferred job for this game.
    pub scrape_active: bool,
    /// Sole work publication date, shown separately from an edition date.
    pub work_date: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum Field {
    Title,
    Region,
    Date,
}

impl Release {
    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.title.as_str())
            .chain(self.aliases.split('|').filter(|name| !name.is_empty()))
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
        "SELECT f.id,f.serial,f.region,f.title,f.date,f.aliases,f.work_date
         FROM fetched_releases f LEFT JOIN metadata_responses r ON r.serial=f.serial
         WHERE f.serial=? AND (r.provider IS NULL OR f.source=r.provider) ORDER BY f.id",
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

pub(super) fn migrate_local_cache(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(include_str!("metadata/local_cache.sql"))
}

pub(super) fn migrate_provenance(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(include_str!("metadata/provenance.sql"))?;
    let mut existing =
        tx.prepare("SELECT id,serial,region,title,date,aliases,work_date FROM fetched_releases")?;
    for row in existing.query_map([], Release::from_row)? {
        index_names(tx, &row?)?;
    }
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
            source_revision=COALESCE(
                (SELECT revision FROM metadata_responses WHERE serial=r.serial AND provider=r.source),
                (SELECT m.source_revision FROM game_metadata m
                 WHERE m.release_serial=r.serial AND m.release_id=r.id AND m.source=r.source
                   AND m.auto_title=r.title AND m.auto_region=r.region AND m.auto_date IS NULLIF(r.date,'')
                   AND m.work_date IS NULLIF(r.work_date,'') AND COALESCE(m.aliases,'')=r.aliases
                 ORDER BY m.source_revision DESC LIMIT 1)),
            work_date=NULLIF(r.work_date,''), pending=0
         FROM fetched_releases AS r WHERE game_metadata.game_id=?1 AND r.id=?2",
        params![id, release_id, confirmed],
    )
}

struct Evidence {
    cache_generation: i64,
    cache_cleared: bool,
    serial: Option<String>,
    region: Option<String>,
    selected: Option<String>,
    filename: String,
}

fn evidence(conn: &Connection, id: i64) -> rusqlite::Result<Evidence> {
    conn.query_row(
        "SELECT CASE WHEN confirmed THEN release_serial ELSE disc_serial END,
                CASE WHEN confirmed THEN auto_region ELSE disc_region END,
                CASE WHEN confirmed THEN release_id END, filename, cache_generation, cache_cleared
         FROM game_metadata WHERE game_id=?",
        [id],
        |r| {
            Ok(Evidence {
                serial: r.get(0)?,
                region: r.get(1)?,
                selected: r.get(2)?,
                filename: r.get(3)?,
                cache_generation: r.get(4)?,
                cache_cleared: r.get(5)?,
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
    if evidence.cache_cleared {
        return Ok(());
    }
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

/// Search cached names locally or query release providers for an uncached PS1 serial.
pub fn search_candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    search_with(lib, id, query, |serial| {
        lookup_release(serial, &AtomicBool::new(false)).map_err(|err| err.to_string())
    })
}

fn search_with(
    lib: &Library,
    id: i64,
    query: &str,
    fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
) -> Result<Vec<Release>, String> {
    let serial = query.trim().to_ascii_uppercase();
    if is_release_serial(&serial) {
        let mut conn = lib.connect()?;
        let cached = conn
            .prepare_cached("SELECT 1 FROM fetched_releases WHERE serial=?")
            .and_then(|mut stmt| stmt.exists([&serial]))
            .map_err(|e| e.to_string())?;
        if !cached {
            download_serial(&mut conn, id, &serial, fetch)?;
        }
    }
    candidates(lib, id, query)
}

pub fn select(lib: &Library, id: i64, release_id: &str) -> Result<(), String> {
    select_cancellable(lib, id, release_id, &AtomicBool::new(false))
}

/// Confirm an edition unless a later clear or selection has superseded the request.
pub fn select_cancellable(
    lib: &Library,
    id: i64,
    release_id: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ProviderError::Cancelled.to_string());
    }
    if assign(&tx, id, release_id, true).map_err(|e| e.to_string())? == 0 {
        return Err("Unknown game or cached release".into());
    }
    restore_display(&tx, id).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// Publish explicitly restored release details or artwork inside the transaction that saved them.
pub(super) fn restore_display(db: &Connection, id: i64) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE game_metadata SET cache_cleared=0 WHERE game_id=? AND cache_cleared",
        [id],
    )?;
    Ok(())
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
    set_overrides(lib, id, &[(field, value)])
}

impl Field {
    pub fn accepts(self, value: &str) -> bool {
        let value = value.trim();
        !value.is_empty()
            && value.len() <= 1024
            && (!matches!(self, Self::Date) || valid_date(value))
    }
}

/// Commit just the edited fields together, retaining automatic updates to untouched fields.
pub fn set_overrides(
    lib: &Library,
    id: i64,
    changes: &[(Field, Option<&str>)],
) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for &(field, value) in changes {
        let value = value.map(str::trim);
        if value.is_some_and(|v| !field.accepts(v)) {
            return Err(match field {
                Field::Date => "Use YYYY, YYYY-MM or YYYY-MM-DD",
                _ => "Enter 1 to 1024 bytes",
            }
            .into());
        }
        let column = match field {
            Field::Title => "user_title",
            Field::Region => "user_region",
            Field::Date => "user_date",
        };
        tx.execute(
            &format!("UPDATE game_metadata SET {column}=?2 WHERE game_id=?1"),
            params![id, value],
        )
        .map_err(|e| e.to_string())?;
    }
    if changes.iter().any(|(_, value)| value.is_some()) {
        restore_display(&tx, id).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn list(lib: &Library) -> Result<HashMap<i64, Metadata>, String> {
    read_metadata(lib, None)
}

/// Read metadata and cover references for only the affected cards.
pub fn list_by_id(lib: &Library, ids: &[i64]) -> Result<HashMap<i64, Metadata>, String> {
    let keys = serde_json::to_string(ids).map_err(|error| error.to_string())?;
    read_metadata(lib, Some(&keys))
}

fn read_metadata(lib: &Library, ids: Option<&str>) -> Result<HashMap<i64, Metadata>, String> {
    let conn = lib.connect()?;
    let filter = if ids.is_some() {
        "WHERE m.game_id IN (SELECT value FROM json_each(?1))"
    } else {
        ""
    };
    let mut stmt=conn.prepare(&format!("SELECT m.game_id, COALESCE(user_title,auto_title), COALESCE(user_region,auto_region,disc_region), COALESCE(user_date,auto_date),
        release_id,COALESCE(release_serial,disc_serial),auto_title,aliases,COALESCE(u.thumb,c.thumb),COALESCE(u.source,c.source),
        COALESCE(a.state,CASE WHEN c.thumb IS NOT NULL THEN 'ready' END),m.source,m.refreshed,m.pending,m.work_date,
        EXISTS(SELECT 1 FROM scrape_jobs j WHERE j.game_id=m.game_id AND j.status IN ('pending','running','retry_wait')),
        CASE WHEN release_id IS NOT NULL THEN auto_title || COALESCE(' · ' || NULLIF(auto_region,''),'') END,
        user_title IS NOT NULL,user_region IS NOT NULL,user_date IS NOT NULL,u.game_id IS NOT NULL,m.cache_cleared
        FROM game_metadata m LEFT JOIN cover_assets u ON u.game_id=m.game_id AND u.kind='user'
        LEFT JOIN cover_assets c ON c.game_id=m.game_id AND c.kind='download' AND c.serial=COALESCE(release_serial,disc_serial)
        LEFT JOIN cover_attempts a ON a.game_id=m.game_id AND a.serial=COALESCE(release_serial,disc_serial)
        AND a.rowid=(SELECT rowid FROM cover_attempts WHERE game_id=m.game_id AND serial=COALESCE(m.release_serial,m.disc_serial)
        ORDER BY updated DESC,rowid DESC LIMIT 1) {filter}")).map_err(|e|e.to_string())?;
    stmt.query_map(rusqlite::params_from_iter(ids), |r| {
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
                cache_cleared: r.get(21)?,
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
                scrape_active: r.get(15)?,
                release_title: r.get(16)?,
                manual_fields: [r.get(17)?, r.get(18)?, r.get(19)?],
                manual_cover: r.get(20)?,
            },
        ))
    })
    .and_then(Iterator::collect)
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

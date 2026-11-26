//! Platform-specific release metadata in the user library and per-field user corrections.
use super::transport;
mod cache;
mod gamedb;
mod matching;
pub(super) mod no_intro;
mod providers;
mod refresh;
mod wikidata;
pub(crate) use super::transport::ProviderError;
pub use cache::CacheScope;
pub(super) use cache::{clear_in_transaction, migrate_release_preferences};
pub(super) use gamedb::{license_matches_market, named_markets};
pub use matching::candidates;
pub(super) use matching::{disc_market_filter, region_matches};
use matching::{has_title_candidate, index_names, matching_serial, title_hint};
pub(super) use providers::is_release_serial;
use providers::lookup_release;
use refresh::download_serial;
pub(super) use refresh::{Batch, Prepared, identify_cartridge_for_art};
pub use refresh::{RefreshOutcome, refresh_online, refresh_online_cancellable};
pub(super) use wikidata::serial_region;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::{GamePlatform, Library};

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
    /// GameDB provides the edition's original playlist name as its single alias.
    pub(in crate::library) fn gamedb_release_name(&self) -> Option<&str> {
        (self.id.starts_with("gamedb/") && !self.aliases.is_empty()).then_some(&self.aliases)
    }

    /// Cartridge dumps identified by a DAT digest have no catalog product code.
    pub fn product_code(&self) -> Option<&str> {
        no_intro::product_code(&self.serial)
    }

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

fn cached_for(
    conn: &Connection,
    serial: &str,
    platform: GamePlatform,
) -> rusqlite::Result<Vec<Release>> {
    conn.prepare_cached(
        "SELECT f.id,f.serial,f.region,f.title,f.date,f.aliases,f.work_date
         FROM fetched_releases f LEFT JOIN metadata_responses r ON r.serial=f.serial
         WHERE f.serial=?1 AND (f.id GLOB 'libretro-gba/*')=(?2='GBA') AND (f.id GLOB 'libretro-nds/*')=(?2='NDS') AND (r.provider IS NULL OR f.source=r.provider) ORDER BY f.id",
    )?
    .query_map(params![serial, platform.as_str()], Release::from_row)?
    .collect()
}

pub(super) fn game_platform(db: &Connection, game: i64) -> rusqlite::Result<GamePlatform> {
    db.query_row("SELECT platform FROM games WHERE id=?", [game], |row| {
        GamePlatform::from_id(row.get_ref(0)?.as_str()?).ok_or(rusqlite::Error::InvalidQuery)
    })
}

pub(super) fn release_platform(id: &str) -> GamePlatform {
    match id.split('/').next() {
        Some("libretro-gba") => GamePlatform::Gba,
        Some("libretro-nds") => GamePlatform::Nds,
        _ => GamePlatform::Ps1,
    }
}

pub(super) fn catalog_available(db: &Connection, game: i64) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT platform IN ('PS1','GBA','NDS') FROM games WHERE id=?",
        [game],
        |r| r.get(0),
    )
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
    let supported = catalog_available(tx, game_id)?;
    let platform = if supported {
        Some(game_platform(tx, game_id)?)
    } else {
        None
    };
    let serial = serial.map(|code| no_intro::cache_key(platform, code));
    let serial = serial.as_deref();
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO game_metadata(game_id,disc_serial,disc_region,filename)
         VALUES(?1,?2,?3,?4)",
        params![game_id, serial, region, title],
    )?;
    if inserted != 0
        && let Some(platform) = platform
    {
        match_evidence(tx, game_id, serial, region, title, platform)?;
    }
    Ok(())
}

fn match_evidence(
    conn: &Transaction<'_>,
    id: i64,
    serial: Option<&str>,
    region: Option<&str>,
    title: &str,
    platform: GamePlatform,
) -> rusqlite::Result<()> {
    let cached = serial
        .map(|s| cached_for(conn, s, platform))
        .transpose()?
        .unwrap_or_default();
    if let Some(release) = matching_serial(&cached, serial, region, platform.cartridge()) {
        assign(conn, id, &release.id, false)?;
    } else {
        // A title or alias is only a suggestion. Never assign an edition from a filename.
        let pending =
            !cached.is_empty() || has_title_candidate(conn, &title_hint(title), platform)?;
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
    // Version 3 libraries predate GBA and the games.platform column.
    for (id, serial, region, title) in pending {
        match_evidence(
            tx,
            id,
            serial.as_deref(),
            region.as_deref(),
            &title,
            GamePlatform::Ps1,
        )?;
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
    conn: &Transaction<'_>,
    id: i64,
    release_id: &str,
    confirmed: bool,
) -> rusqlite::Result<usize> {
    let changed = conn
        .prepare_cached(
            "SELECT 1 FROM game_metadata m JOIN fetched_releases r ON r.id=?2
         WHERE m.game_id=?1 AND (m.release_id IS NOT r.id OR m.auto_region IS NOT r.region)",
        )?
        .exists(params![id, release_id])?;
    if changed {
        detach_edition_art(conn, id)?;
    }
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

fn detach_edition_art(tx: &Transaction<'_>, id: i64) -> rusqlite::Result<()> {
    // The object collector checks every other card before deleting a shared file.
    tx.execute(
        "DELETE FROM cover_assets WHERE game_id=? AND kind='download'",
        [id],
    )?;
    // Before a first match, misses are already keyed by the source URL and remain
    // valid when the newly identified release resolves to the same request.
    tx.execute(
        "DELETE FROM cover_attempts WHERE game_id=?1 AND (state='ready' OR
         EXISTS(SELECT 1 FROM game_metadata WHERE game_id=?1 AND release_id IS NOT NULL))",
        [id],
    )?;
    Ok(())
}

struct Evidence {
    platform: GamePlatform,
    cache_generation: i64,
    cache_cleared: bool,
    serial: Option<String>,
    region: Option<String>,
    selected: Option<String>,
    filename: String,
}

fn evidence(conn: &Connection, id: i64) -> rusqlite::Result<Evidence> {
    conn.query_row(
        "SELECT CASE WHEN preferred_release_id IS NOT NULL OR (release_serial GLOB 'gba-rom:*' OR release_serial GLOB 'nds-rom:*') THEN release_serial ELSE disc_serial END,
                CASE WHEN confirmed THEN auto_region ELSE disc_region END,
                preferred_release_id, filename, cache_generation, cache_cleared, platform
         FROM game_metadata JOIN games ON games.id=game_id WHERE game_id=?",
        [id],
        |r| {
            Ok(Evidence {
                platform: GamePlatform::from_id(r.get_ref(6)?.as_str()?).ok_or(rusqlite::Error::InvalidQuery)?,
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
    if !catalog_available(&tx, id).map_err(|e| e.to_string())? {
        return Ok(());
    }
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
            evidence.platform,
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Search cached names locally or query release providers for an uncached PS1 serial.
pub fn search_candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    search_candidates_cancellable(lib, id, query, &AtomicBool::new(false))
}

/// Search for editions until the caller closes the picker or clears its cache.
pub fn search_candidates_cancellable(
    lib: &Library,
    id: i64,
    query: &str,
    cancel: &AtomicBool,
) -> Result<Vec<Release>, String> {
    search_with(lib, id, query, cancel, |serial| {
        lookup_release(serial, cancel).map_err(|err| err.to_string())
    })
}

fn search_with(
    lib: &Library,
    id: i64,
    query: &str,
    cancel: &AtomicBool,
    fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
) -> Result<Vec<Release>, String> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ProviderError::Cancelled.to_string());
    }
    let serial = query.trim().to_ascii_uppercase();
    let mut conn = lib.connect()?;
    if !catalog_available(&conn, id).map_err(|e| e.to_string())? {
        return Ok(Vec::new());
    }
    let platform = game_platform(&conn, id).map_err(|e| e.to_string())?;
    let valid_serial = if platform.cartridge() {
        no_intro::is_code(&serial)
    } else {
        is_release_serial(&serial)
    };
    if valid_serial {
        let serial = no_intro::cache_key(Some(platform), &serial);
        let cached = conn
            .prepare_cached(
                "SELECT 1 FROM fetched_releases WHERE serial=?1 AND (id GLOB 'libretro-gba/*')=(?2='GBA') AND (id GLOB 'libretro-nds/*')=(?2='NDS')",
            )
            .and_then(|mut stmt| stmt.exists(params![serial, platform.as_str()]))
            .map_err(|e| e.to_string())?;
        if !cached {
            download_serial(&mut conn, id, &serial, cancel, |s| {
                if platform.cartridge() {
                    no_intro::lookup(lib, s, cancel).map_err(|err| err.to_string())
                } else {
                    fetch(s)
                }
            })?;
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
    if !catalog_available(&tx, id).map_err(|e| e.to_string())?
        || game_platform(&tx, id).map_err(|e| e.to_string())? != release_platform(release_id)
    {
        return Err("Release lookup is unavailable for this platform".into());
    }
    if assign(&tx, id, release_id, true).map_err(|e| e.to_string())? == 0 {
        return Err("Unknown game or cached release".into());
    }
    tx.execute(
        "UPDATE game_metadata SET preferred_release_id=release_id, cache_cleared=0 WHERE game_id=?",
        [id],
    )
    .map_err(|e| e.to_string())?;
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
        release_id,CASE WHEN (release_serial GLOB 'gba-rom:*' OR release_serial GLOB 'nds-rom:*') THEN disc_serial ELSE COALESCE(release_serial,disc_serial) END,auto_title,aliases,COALESCE(u.thumb,c.thumb),COALESCE(u.source,c.source),
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
                serial: r
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .and_then(no_intro::product_code)
                    .map(str::to_owned),
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

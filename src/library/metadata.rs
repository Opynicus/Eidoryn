//! Offline PlayStation releases and per-field user corrections.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use rusqlite::{Connection, Transaction, params};

use super::Library;

pub(super) const SCHEMA: &str = include_str!("metadata/schema.sql");
const SNAPSHOT: &str = "Wikidata/PS1/2026-11-20";

#[derive(Clone, Copy, Debug)]
pub struct Release {
    pub id: &'static str,
    pub serial: &'static str,
    pub region: &'static str,
    pub title: &'static str,
    pub date: &'static str,
    aliases: &'static str,
    work_date: &'static str,
}

// Every line is validated by the catalog test, including IDs, date precision and aliases.
static CATALOG: LazyLock<Vec<Release>> = LazyLock::new(|| {
    include_str!("metadata/ps1.tsv")
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let mut next = || fields.next().expect("bundled release has seven fields");
            Release {
                id: next(),
                serial: next(),
                region: next(),
                title: next(),
                date: next(),
                aliases: next(),
                work_date: next(),
            }
        })
        .collect()
});

impl Release {
    fn names(self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.title).chain(self.aliases.split('|').filter(|name| !name.is_empty()))
    }
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

fn matching_serial(serial: Option<&str>, region: Option<&str>) -> Option<&'static Release> {
    let serial = serial?;
    let mut exact = CATALOG
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

fn suggestions(serial: Option<&str>, hint: &str) -> impl Iterator<Item = &'static Release> {
    let hint = title_hint(hint);
    CATALOG.iter().filter(move |r| {
        Some(r.serial) == serial
            || (!hint.is_empty() && r.names().any(|name| name.to_lowercase() == hint))
    })
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
    let release = matching_serial(serial, region);
    assign(conn, id, release, false)?;
    if release.is_none() && suggestions(serial, title).next().is_some() {
        conn.execute("UPDATE game_metadata SET pending=1 WHERE game_id=?1", [id])?;
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
    release: Option<&Release>,
    confirmed: bool,
) -> rusqlite::Result<()> {
    conn.execute("UPDATE game_metadata SET release_id=?2, release_serial=?3, auto_title=?4, auto_region=?5,
        auto_date=?6, aliases=?7, source=?8, refreshed=unixepoch(), confirmed=?9, work_date=?10, pending=0 WHERE game_id=?1",
        params![id,release.map(|r|r.id),release.map(|r|r.serial),release.map(|r|r.title),
            release.map(|r|r.region),release.map(|r|r.date).filter(|s|!s.is_empty()),release.map(|r|r.aliases),
            release.map(|_| SNAPSHOT),confirmed,release.map(|r|r.work_date).filter(|s|!s.is_empty())])?;
    Ok(())
}

pub fn refresh(lib: &Library, id: i64) -> Result<(), String> {
    let mut conn = lib.connect()?;
    let mut work = || -> rusqlite::Result<()> {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (serial,region,selected,title)=tx.query_row(
            "SELECT disc_serial,disc_region,CASE WHEN confirmed THEN release_id END,filename FROM game_metadata WHERE game_id=?1",
            [id],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?)))?;
        if let Some(selected) = selected {
            // Preserve the last confirmed snapshot even if a newer catalog omits its edition.
            if let Some(release) = CATALOG.iter().find(|r| r.id == selected) {
                assign(&tx, id, Some(release), true)?;
            }
        } else {
            match_evidence(&tx, id, serial.as_deref(), region.as_deref(), &title)?;
        }
        tx.commit()
    };
    work().map_err(|e| e.to_string())
}

pub fn candidates(lib: &Library, id: i64, query: &str) -> Result<Vec<Release>, String> {
    let conn = lib.connect()?;
    if query.trim().is_empty() {
        let (serial, title) = conn
            .query_row(
                "SELECT disc_serial,filename FROM game_metadata WHERE game_id=?1",
                [id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(|e| e.to_string())?;
        return Ok(suggestions(serial.as_deref(), &title).copied().collect());
    }
    let needle = query.trim().to_lowercase();
    Ok(CATALOG
        .iter()
        .filter(|r| {
            r.serial.to_lowercase().contains(&needle)
                || r.names().any(|n| n.to_lowercase().contains(&needle))
        })
        .copied()
        .collect())
}

pub fn select(lib: &Library, id: i64, release_id: &str) -> Result<(), String> {
    let release = CATALOG
        .iter()
        .find(|r| r.id == release_id)
        .ok_or("Unknown release")?;
    assign(&lib.connect()?, id, Some(release), true).map_err(|e| e.to_string())
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

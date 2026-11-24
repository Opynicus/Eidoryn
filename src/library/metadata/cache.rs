//! Transactional cleanup of automatic release metadata and its shared source records.
use super::{Library, gamedb, title_hint};
use crate::library::covers;
use rusqlite::{Transaction, TransactionBehavior, params};
use std::collections::BTreeSet;

/// Games whose automatically scraped release metadata should be cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    /// Only games explicitly removed from the library.
    Removed,
    /// All library entries and unassigned release searches.
    All,
}

impl Library {
    /// Clear downloaded release data and artwork, retaining only user edits and edition preferences.
    pub fn clear_metadata_cache(&self, scope: CacheScope) -> Result<(), String> {
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let originals = clear_in_transaction(&tx, scope, None).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        if scope == CacheScope::All {
            gamedb::clear_cache();
            super::gba::clear_cache();
        }
        covers::collect_released(self, &mut db, &originals);
        Ok(())
    }
}

pub(in crate::library) fn clear_in_transaction(
    tx: &Transaction<'_>,
    scope: CacheScope,
    removed_ids: Option<&str>,
) -> rusqlite::Result<BTreeSet<String>> {
    create_targets(tx)?;
    tx.execute(
        "INSERT INTO clear_games SELECT id FROM games WHERE (?1 OR hidden)
         AND (?2 IS NULL OR id IN (SELECT value FROM json_each(?2)))",
        params![scope == CacheScope::All, removed_ids],
    )?;
    if scope == CacheScope::All {
        tx.execute_batch(
            "INSERT INTO clear_serials
             SELECT serial FROM metadata_requests UNION SELECT serial FROM fetched_releases
             UNION SELECT serial FROM metadata_responses UNION SELECT serial FROM referenced_metadata_responses;",
        )?;
    } else {
        collect_game_serials(tx)?;
    }
    let originals = tx
        .prepare("DELETE FROM cover_assets WHERE kind='download' AND game_id IN clear_games RETURNING original")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    tx.execute_batch(
        "UPDATE scrape_jobs SET status='cancelled',owner=NULL,lease_until=NULL,updated_at=unixepoch()
         WHERE status IN ('pending','running','retry_wait') AND game_id IN clear_games;
         DELETE FROM cover_attempts WHERE game_id IN clear_games;
         UPDATE game_metadata SET cache_generation=cache_generation+1,cover_revision=cover_revision+1,
             cache_cleared=1 WHERE game_id IN clear_games;",
    )?;
    tx.execute_batch(include_str!("clear_cache.sql"))?;
    Ok(originals)
}

fn create_targets(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE TEMP TABLE clear_games(id INTEGER PRIMARY KEY);
         CREATE TEMP TABLE clear_serials(serial TEXT PRIMARY KEY) WITHOUT ROWID;",
    )
}

fn collect_game_serials(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "INSERT INTO clear_serials
         SELECT disc_serial FROM game_metadata WHERE game_id IN clear_games AND disc_serial IS NOT NULL
         UNION SELECT release_serial FROM game_metadata WHERE game_id IN clear_games AND release_serial IS NOT NULL
         UNION SELECT serial FROM discs WHERE game_id IN clear_games AND serial IS NOT NULL;",
    )?;
    // A still-unmatched image can use the cached edition by title or alias
    // even when its own boot serial is missing or differs.
    let mut filenames = tx.prepare(
        "SELECT DISTINCT filename FROM game_metadata WHERE pending AND game_id NOT IN clear_games",
    )?;
    let mut retain = tx.prepare(
        "DELETE FROM clear_serials WHERE serial IN (
             SELECT f.serial FROM release_names n JOIN fetched_releases f ON f.id=n.release_id
             WHERE n.name=?)",
    )?;
    let mut rows = filenames.query([])?;
    while let Some(row) = rows.next()? {
        retain.execute([title_hint(row.get_ref(0)?.as_str()?)])?;
    }
    Ok(())
}

/// Repair v8 rows whose downloaded release and source records survived an earlier clear.
pub(in crate::library) fn migrate_release_preferences(
    tx: &Transaction<'_>,
) -> rusqlite::Result<()> {
    tx.execute_batch(
        "ALTER TABLE game_metadata ADD COLUMN preferred_release_id TEXT;
         UPDATE game_metadata SET preferred_release_id=release_id WHERE confirmed;",
    )?;
    if !tx
        .prepare("SELECT 1 FROM game_metadata WHERE cache_cleared AND confirmed")?
        .exists([])?
    {
        return Ok(());
    }
    create_targets(tx)?;
    tx.execute_batch(
        "INSERT INTO clear_games SELECT game_id FROM game_metadata WHERE cache_cleared AND confirmed",
    )?;
    collect_game_serials(tx)?;
    tx.execute_batch(include_str!("clear_cache.sql"))
}

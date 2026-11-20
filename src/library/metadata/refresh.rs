//! Acquire source data outside transactions and publish each release with the active job.
use super::{
    Evidence, LIVE, Library, ProviderError, Release, SourceResponse, assign, cached_for, evidence,
    matching::{index_names, matching_serial},
};
use crate::library::scrape::Mode;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    Updated,
    Candidates,
    NotFound,
    NoSerial,
}

/// Fetch the selected edition, preserving local corrections made while the request runs.
pub fn refresh_online(lib: &Library, id: i64) -> Result<RefreshOutcome, String> {
    let cancel = AtomicBool::new(false);
    let batch = Batch::default();
    let prepared = batch
        .prepare(lib, id, Mode::Full, &cancel)
        .map_err(|e| e.to_string())?;
    commit_one(lib, id, prepared)
}

pub(super) fn commit_one(
    lib: &Library,
    id: i64,
    prepared: Prepared,
) -> Result<RefreshOutcome, String> {
    let mut db = lib.connect()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let outcome = prepared.commit(&tx, id)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(outcome)
}

#[cfg(test)]
pub(super) fn refresh_with(
    lib: &Library,
    id: i64,
    fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
) -> Result<RefreshOutcome, String> {
    Batch::default().refresh_with(lib, id, Mode::Full, &AtomicBool::new(false), fetch)
}

#[derive(Default)]
pub(in crate::library) struct Batch {
    lookups: Mutex<HashMap<String, Arc<Mutex<SerialLookup>>>>,
}

#[derive(Default)]
enum SerialLookup {
    #[default]
    New,
    Retained,
    Missing,
    Failed(ProviderError),
    // Only jobs awaiting the writer hold a strong reference to the HTTP body.
    // A batch with thousands of releases retains just the revisions after commit.
    Fetched(i64, Weak<Fetched>),
}

struct Fetched {
    revision: i64,
    download: SourceResponse,
}

enum Hit {
    Missing,
    Cached,
    Fetched(Arc<Fetched>),
}

pub(in crate::library) struct Prepared {
    serial: Option<String>,
    hit: Hit,
}

impl Batch {
    pub(in crate::library) fn prepare(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
    ) -> Result<Prepared, ProviderError> {
        self.prepare_with(lib, id, mode, cancel, |serial| {
            super::lookup_release(serial, cancel)
        })
    }

    #[cfg(test)]
    pub(super) fn refresh_with(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
        fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
    ) -> Result<RefreshOutcome, String> {
        let prepared = self
            .prepare_with(lib, id, mode, cancel, |s| {
                fetch(s).map_err(ProviderError::Invalid)
            })
            .map_err(|e| e.to_string())?;
        commit_one(lib, id, prepared)
    }

    pub(super) fn prepare_with(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
        fetch: impl FnOnce(&str) -> Result<SourceResponse, ProviderError>,
    ) -> Result<Prepared, ProviderError> {
        let conn = lib.worker_connection().map_err(ProviderError::Database)?;
        let evidence = evidence(&conn, id)?;
        let Some(key) = evidence.serial.as_deref() else {
            return Ok(Prepared {
                serial: None,
                hit: Hit::Missing,
            });
        };
        let entry = self
            .lookups
            .lock()
            .map_err(|e| ProviderError::Database(e.to_string()))?
            .entry(key.to_owned())
            .or_default()
            .clone();
        let mut current = entry
            .lock()
            .map_err(|e| ProviderError::Database(e.to_string()))?;
        if cancel.load(Ordering::Relaxed) {
            return Err(ProviderError::Cancelled);
        }
        let hit = match &*current {
            SerialLookup::New => None,
            SerialLookup::Retained => Some(Hit::Cached),
            SerialLookup::Missing => Some(Hit::Missing),
            SerialLookup::Failed(error) => return Err(error.clone()),
            SerialLookup::Fetched(revision, weak) => {
                if let Some(body) = weak.upgrade() {
                    Some(Hit::Fetched(body))
                } else if committed(&conn, key, *revision)? {
                    Some(Hit::Cached)
                } else {
                    None // A failed writer transaction released the last copy.
                }
            }
        };
        let hit = if let Some(hit) = hit {
            hit
        } else if mode == Mode::Incremental && complete_cached_edition(&conn, key, &evidence)? {
            Hit::Cached
        } else {
            let revision = reserve(&conn, key)?;
            let result = fetch(key);
            if cancel.load(Ordering::Relaxed) {
                return Err(ProviderError::Cancelled);
            }
            match result {
                Ok(download) if download.releases.is_empty() => {
                    *current = SerialLookup::Missing;
                    Hit::Missing
                }
                Ok(download) if retains_dated_edition(&conn, key, &download)? => {
                    *current = SerialLookup::Retained;
                    Hit::Cached
                }
                Ok(download) => {
                    let fetched = Arc::new(Fetched { revision, download });
                    *current = SerialLookup::Fetched(revision, Arc::downgrade(&fetched));
                    Hit::Fetched(fetched)
                }
                Err(e) => {
                    if !e.retryable() && e != ProviderError::Cancelled {
                        *current = SerialLookup::Failed(e.clone());
                    }
                    return Err(e);
                }
            }
        };
        Ok(Prepared {
            serial: evidence.serial,
            hit,
        })
    }
}

// A cached work date or a candidate from a different region cannot complete a
// disc edition. The old cache had no raw response for some online Wikidata rows.
fn complete_cached_edition(
    conn: &Connection,
    serial: &str,
    evidence: &Evidence,
) -> rusqlite::Result<bool> {
    let releases = cached_for(conn, serial)?;
    let edition = if let Some(id) = &evidence.selected {
        releases.iter().find(|r| &r.id == id)
    } else {
        matching_serial(&releases, Some(serial), evidence.region.as_deref())
    };
    let Some(edition) = edition.filter(|r| !r.date.is_empty() && !r.title.is_empty()) else {
        return Ok(false);
    };
    conn.prepare_cached(
        "SELECT 1 FROM fetched_releases f LEFT JOIN metadata_responses r
          ON r.serial=f.serial AND r.provider=f.source
         WHERE f.id=?1 AND (f.source=?2 OR r.serial IS NOT NULL)",
    )?
    .exists(params![edition.id, LIVE])
}

// An online source can temporarily lose the regional date or its fallback
// can be offline. Retain the complete previous response with its original
// provenance until this same edition can be refreshed with a date.
fn retains_dated_edition(
    conn: &Connection,
    serial: &str,
    response: &SourceResponse,
) -> rusqlite::Result<bool> {
    let [release] = response.releases.as_slice() else {
        return Ok(false);
    };
    if !release.date.is_empty() || release.serial != serial {
        return Ok(false);
    }
    conn.prepare_cached(
        "SELECT 1 FROM fetched_releases f JOIN metadata_responses r
                ON f.serial=r.serial AND f.source=r.provider
         WHERE f.serial=?1 AND (f.id=?2 OR f.source<>?4) AND f.region=?3 AND f.date<>'' AND f.title<>''
           AND NOT EXISTS (SELECT 1 FROM fetched_releases other WHERE other.serial=f.serial
                           AND other.source=f.source AND other.id<>f.id)",
    )?
    .exists(params![serial, release.id, release.region, response.source.provider])
}

fn reserve(conn: &Connection, serial: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO metadata_requests(serial,revision) VALUES(?,1)
         ON CONFLICT(serial) DO UPDATE SET revision=revision+1 RETURNING revision",
        [serial],
        |r| r.get(0),
    )
}

fn committed(conn: &Connection, serial: &str, revision: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached("SELECT 1 FROM metadata_responses WHERE serial=? AND revision>=?")?
        .exists(params![serial, revision])
}

impl Fetched {
    fn store(&self, tx: &Transaction<'_>, serial: &str) -> Result<bool, String> {
        let current: i64 = tx
            .query_row(
                "SELECT revision FROM metadata_requests WHERE serial=?",
                [serial],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if current != self.revision {
            return Ok(false);
        }
        if committed(tx, serial, self.revision).map_err(|e| e.to_string())? {
            return Ok(true);
        }
        cache_releases(
            tx,
            serial,
            &self.download.releases,
            self.download.source.provider,
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO metadata_responses(serial,payload,fetched_at,revision,provider,payload_version) VALUES(?1,?2,unixepoch(),?3,?4,?5)
             ON CONFLICT(serial) DO UPDATE SET payload=excluded.payload,fetched_at=excluded.fetched_at,revision=excluded.revision,provider=excluded.provider,payload_version=excluded.payload_version",
            params![serial, self.download.raw, self.revision, self.download.source.provider,self.download.source.payload_version],
        ).map_err(|e| e.to_string())?;
        Ok(true)
    }
}

impl Prepared {
    pub(super) fn commit(self, tx: &Transaction<'_>, id: i64) -> Result<RefreshOutcome, String> {
        self.commit_claim(tx, id)?
            .ok_or_else(|| "A newer metadata request or selection superseded this one".to_owned())
    }

    /// Commit the source response, normalized candidates and game assignment inside
    /// the same transaction as the scheduler's conditional lease completion. A stale
    /// result makes no changes and does not fail other jobs in the writer's batch.
    pub(in crate::library) fn commit_claim(
        self,
        tx: &Transaction<'_>,
        id: i64,
    ) -> Result<Option<RefreshOutcome>, String> {
        let ev = evidence(tx, id).map_err(|e| e.to_string())?;
        if ev.serial != self.serial {
            return Ok(None);
        }
        let Some(serial) = self.serial else {
            return Ok(Some(RefreshOutcome::NoSerial));
        };
        match self.hit {
            Hit::Missing => return Ok(Some(RefreshOutcome::NotFound)),
            Hit::Cached => {}
            Hit::Fetched(fetched) => {
                if !fetched.store(tx, &serial)? {
                    return Ok(None);
                }
            }
        }
        let releases = cached_for(tx, &serial).map_err(|e| e.to_string())?;
        let release = if let Some(key) = &ev.selected {
            releases.iter().find(|r| &r.id == key)
        } else {
            matching_serial(&releases, Some(&serial), ev.region.as_deref())
        };
        if let Some(release) = release {
            assign(tx, id, &release.id, ev.selected.is_some()).map_err(|e| e.to_string())?;
            Ok(Some(RefreshOutcome::Updated))
        } else if ev.selected.is_some() {
            // This source has no update for the edition the user chose. Keep its
            // fields and source revision, even if the serial now yields another edition.
            Ok(Some(RefreshOutcome::NotFound))
        } else {
            tx.execute("UPDATE game_metadata SET pending=1 WHERE game_id=?", [id])
                .map_err(|e| e.to_string())?;
            Ok(Some(RefreshOutcome::Candidates))
        }
    }
}

/// Manual search populates candidates without automatically assigning the game.
pub(super) fn download_serial(
    conn: &mut Connection,
    serial: &str,
    fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
) -> Result<bool, String> {
    let revision = reserve(conn, serial).map_err(|e| e.to_string())?;
    let download = fetch(serial)?;
    if download.releases.is_empty() {
        return Ok(false);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if !(Fetched { revision, download }).store(&tx, serial)? {
        return Err("A newer metadata request superseded this one".into());
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

pub(super) fn cache_releases(
    tx: &Transaction<'_>,
    serial: &str,
    releases: &[Release],
    provider: &str,
) -> rusqlite::Result<()> {
    // Another provider can return a different ID for the same disc. Keep any
    // manually selected editions from the previous provider available offline.
    tx.execute(
        "DELETE FROM fetched_releases AS f WHERE serial=?1 AND (source=?2 OR NOT EXISTS(
            SELECT 1 FROM game_metadata m WHERE m.release_id=f.id AND m.confirmed))",
        params![serial, provider],
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT OR REPLACE INTO fetched_releases(id,serial,region,title,date,aliases,work_date,source)
                                        VALUES(?,?,?,?,?,?,?,?)",
    )?;
    for release in releases {
        insert.execute(params![
            release.id,
            serial,
            release.region,
            release.title,
            release.date,
            release.aliases,
            release.work_date,
            provider
        ])?;
        index_names(tx, release)?;
    }
    Ok(())
}

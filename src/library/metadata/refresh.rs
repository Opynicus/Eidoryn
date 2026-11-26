//! Acquire source data outside transactions and publish each release with the active job.
use super::{
    Evidence, LIVE, Library, ProviderError, Release, SourceResponse, assign, cached_for, evidence,
    matching::{index_names, matching_serial},
};
use crate::library::scrape::Mode;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::{
    borrow::Cow,
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
    refresh_online_cancellable(lib, id, &AtomicBool::new(false))
}

/// Refresh one game while allowing a later cache clear to cancel the request.
pub fn refresh_online_cancellable(
    lib: &Library,
    id: i64,
    cancel: &AtomicBool,
) -> Result<RefreshOutcome, String> {
    if !super::catalog_available(&lib.connect()?, id).map_err(|e| e.to_string())? {
        return Ok(RefreshOutcome::NotFound);
    }
    let batch = Batch::default();
    let prepared = batch
        .prepare(lib, id, Mode::Full, cancel)
        .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(ProviderError::Cancelled.to_string());
    }
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

impl Hit {
    fn match_cartridge(
        &self,
        conn: &Connection,
        id: i64,
        key: &str,
        region: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<CartridgeMatch, ProviderError> {
        let raw = match self {
            Self::Fetched(fetched)
                if fetched.download.source.payload_version == super::no_intro::PAYLOAD_VERSION =>
            {
                Some(Cow::Borrowed(fetched.download.raw.as_slice()))
            }
            Self::Cached => super::no_intro::cached_editions(conn, key)?.map(Cow::Owned),
            _ => None,
        };
        let Some(raw) = raw else {
            return Ok(CartridgeMatch::Unchecked);
        };
        Ok(
            super::no_intro::matching_title(conn, id, key, region, &raw, cancel)?
                .map_or(CartridgeMatch::NeedsChoice, |name| {
                    CartridgeMatch::Edition(super::no_intro::edition_id(key, &name))
                }),
        )
    }
}

pub(in crate::library) struct Prepared {
    cache_generation: i64,
    serial: Option<String>,
    lookup_key: Option<String>,
    cartridge: CartridgeMatch,
    hit: Hit,
}

enum CartridgeMatch {
    Unchecked,
    NeedsChoice,
    Edition(String),
}

impl Batch {
    pub(in crate::library) fn prepare(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
    ) -> Result<Prepared, ProviderError> {
        let platform = super::game_platform(
            &*lib.worker_connection().map_err(ProviderError::Database)?,
            id,
        )?;
        let first = self.prepare_with(lib, id, mode, cancel, |serial| {
            if platform.cartridge() {
                return super::no_intro::lookup(lib, serial, cancel);
            }
            #[cfg(feature = "ui-tests")]
            if let Some(responses) = &lib.scrape_responses {
                return super::wikidata::recorded_response(responses.metadata.clone(), serial);
            }
            super::lookup_release(serial, cancel)
        })?;
        if !platform.cartridge()
            || matches!(first.cartridge, CartridgeMatch::Edition(_))
            || first
                .serial
                .as_deref()
                .is_some_and(super::no_intro::is_signature)
        {
            return Ok(first);
        }
        let conn = lib.worker_connection().map_err(ProviderError::Database)?;
        let ev = evidence(&conn, id)?;
        if ev.selected.is_some() {
            return Ok(first);
        }
        let Some(key) = super::no_intro::unnumbered_key(&conn, id, platform, cancel)? else {
            return Ok(first);
        };
        // The DAT has no cartridge code for some regional releases. Only the
        // size and SHA-1 of a local file may identify one of these editions.
        let by_rom = self.prepare_with_key(lib, id, mode, cancel, Some(&key), |s| {
            super::no_intro::lookup(lib, s, cancel)
        })?;
        if first.serial != by_rom.serial || first.cache_generation != by_rom.cache_generation {
            return Err(ProviderError::Cancelled);
        }
        Ok(if matches!(by_rom.hit, Hit::Missing) {
            first
        } else {
            by_rom
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
        self.prepare_with_key(lib, id, mode, cancel, None, fetch)
    }

    fn prepare_with_key(
        &self,
        lib: &Library,
        id: i64,
        mode: Mode,
        cancel: &AtomicBool,
        lookup_key: Option<&str>,
        fetch: impl FnOnce(&str) -> Result<SourceResponse, ProviderError>,
    ) -> Result<Prepared, ProviderError> {
        let conn = lib.worker_connection().map_err(ProviderError::Database)?;
        let evidence = evidence(&conn, id)?;
        if mode == Mode::Incremental && evidence.cache_cleared {
            return Err(ProviderError::Cancelled);
        }
        let Some(key) = lookup_key.or(evidence.serial.as_deref()) else {
            return Ok(Prepared {
                cache_generation: evidence.cache_generation,
                serial: None,
                lookup_key: None,
                cartridge: CartridgeMatch::Unchecked,
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
        check_cancel(cancel)?;
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
            check_cancel(cancel)?;
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
        drop(current);
        // Only the provider response is shared by serial. Two ROMs with the same
        // header code can be different revisions and must be matched separately.
        let cartridge = if evidence.platform.cartridge() && evidence.selected.is_none() {
            // An incremental cache hit reuses the identity already verified for this
            // imported cartridge. A fresh source response can also verify a present ROM.
            if super::no_intro::is_signature(key)
                && lookup_key.is_none()
                && !matches!(hit, Hit::Cached)
                && let Some(observed) =
                    super::no_intro::unnumbered_key(&conn, id, evidence.platform, cancel)?
                && observed != key
            {
                return Err(ProviderError::Invalid(
                    "The cartridge no longer matches its catalog digest".into(),
                ));
            }
            hit.match_cartridge(&conn, id, key, evidence.region.as_deref(), cancel)?
        } else {
            CartridgeMatch::Unchecked
        };
        Ok(Prepared {
            cache_generation: evidence.cache_generation,
            serial: evidence.serial,
            lookup_key: lookup_key.map(str::to_owned),
            cartridge,
            hit,
        })
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ProviderError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ProviderError::Cancelled)
    } else {
        Ok(())
    }
}

/// Direct artwork requests identify an unselected cartridge the same way as a batch job.
pub(in crate::library) fn identify_cartridge_for_art(
    lib: &Library,
    id: i64,
    mode: Mode,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let db = lib.worker_connection()?;
    let needs_lookup: bool = db.query_row(
        "SELECT release_id IS NULL AND (NOT cache_cleared OR ?2) FROM game_metadata WHERE game_id=?1",
        params![id, mode == Mode::Full],
        |r| r.get(0),
    ).map_err(|e| e.to_string())?;
    if !needs_lookup {
        return Ok(true);
    }
    let prepared = Batch::default()
        .prepare(lib, id, mode, cancel)
        .map_err(|e| e.to_string())?;
    check_cancel(cancel).map_err(|e| e.to_string())?;
    Ok(commit_one(lib, id, prepared)? == RefreshOutcome::Updated)
}

// A cached work date or a candidate from a different region cannot complete a
// disc edition. The old cache had no raw response for some online Wikidata rows.
fn complete_cached_edition(
    conn: &Connection,
    serial: &str,
    evidence: &Evidence,
) -> rusqlite::Result<bool> {
    if evidence.platform.cartridge()
        && evidence.selected.is_none()
        && super::no_intro::cached_editions(conn, serial)?.is_some()
    {
        return Ok(true);
    }
    let releases = cached_for(conn, serial, evidence.platform)?;
    let edition = if let Some(id) = &evidence.selected {
        releases.iter().find(|r| &r.id == id)
    } else {
        matching_serial(
            &releases,
            Some(serial),
            evidence.region.as_deref(),
            evidence.platform.cartridge(),
        )
    };
    let Some(edition) = edition
        .filter(|r| (evidence.platform.cartridge() || !r.date.is_empty()) && !r.title.is_empty())
    else {
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
        if ev.serial != self.serial || ev.cache_generation != self.cache_generation {
            return Ok(None);
        }
        let Some(serial) = self.lookup_key.or(self.serial) else {
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
        let releases = cached_for(tx, &serial, ev.platform).map_err(|e| e.to_string())?;
        let release = if let Some(key) = ev.selected.as_ref() {
            releases.iter().find(|r| &r.id == key)
        } else {
            match self.cartridge {
                CartridgeMatch::Edition(key) => releases.iter().find(|r| r.id == key),
                CartridgeMatch::NeedsChoice => None,
                CartridgeMatch::Unchecked => matching_serial(
                    &releases,
                    Some(&serial),
                    ev.region.as_deref(),
                    ev.platform.cartridge(),
                ),
            }
        };
        if let Some(release) = release {
            assign(tx, id, &release.id, ev.selected.is_some()).map_err(|e| e.to_string())?;
            if ev.cache_cleared {
                super::restore_display(tx, id).map_err(|e| e.to_string())?;
            }
            Ok(Some(RefreshOutcome::Updated))
        } else if ev.selected.is_some() {
            // This source has no update for the edition the user chose. Keep its
            // fields and source revision, even if the serial now yields another edition.
            Ok(Some(RefreshOutcome::NotFound))
        } else {
            if tx
                .prepare_cached(
                    "SELECT 1 FROM game_metadata WHERE game_id=? AND release_id IS NOT NULL",
                )
                .and_then(|mut q| q.exists([id]))
                .map_err(|e| e.to_string())?
            {
                super::detach_edition_art(tx, id).map_err(|e| e.to_string())?;
            }
            tx.execute(
                "UPDATE game_metadata SET pending=1,release_id=NULL,
                   release_serial=CASE WHEN ?2 GLOB 'gba-rom:*' OR ?2 GLOB 'nds-rom:*' THEN ?2 END,
                   auto_title=NULL,auto_region=NULL,auto_date=NULL,aliases=NULL,work_date=NULL,
                   source=NULL,refreshed=NULL,source_revision=NULL WHERE game_id=?1",
                params![id, serial],
            )
            .map_err(|e| e.to_string())?;
            Ok(Some(RefreshOutcome::Candidates))
        }
    }
}

/// Manual search populates candidates without automatically assigning the game.
pub(super) fn download_serial(
    conn: &mut Connection,
    id: i64,
    serial: &str,
    cancel: &AtomicBool,
    fetch: impl FnOnce(&str) -> Result<SourceResponse, String>,
) -> Result<bool, String> {
    let generation = |db: &Connection| {
        db.query_row(
            "SELECT cache_generation FROM game_metadata WHERE game_id=?",
            [id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())
    };
    let before = generation(conn)?;
    let revision = reserve(conn, serial).map_err(|e| e.to_string())?;
    let download = fetch(serial)?;
    if download.releases.is_empty() {
        return Ok(false);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(ProviderError::Cancelled.to_string());
    }
    if generation(&tx)? != before || !(Fetched { revision, download }).store(&tx, serial)? {
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

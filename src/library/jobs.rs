//! Durable job transitions. Only the worker that owns the current lease may publish results.
use super::{Library, covers::Provider, runtime, scrape::Mode};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::time::SystemTime;

pub(super) const LEASE_SECONDS: i64 = 120;
const UPGRADE: &str = include_str!("jobs/upgrade.sql");
const ENQUEUE: &str = include_str!("jobs/enqueue.sql");

pub(super) fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Libretro => "libretro",
        Provider::ScreenScraper => "screenscraper",
        Provider::Igdb => "igdb",
        Provider::Custom => "custom",
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Full => "full",
        Mode::Incremental => "incremental",
    }
}

/// Taken inside the import transaction as well as for explicit refreshes.
pub(super) fn enqueue(
    tx: &Transaction<'_>,
    id: i64,
    provider: Provider,
    mode: Mode,
) -> rusqlite::Result<()> {
    if mode == Mode::Full {
        tx.prepare_cached(UPGRADE)?
            .execute(params![id, provider_name(provider)])?;
    }
    let mode = mode_name(mode);
    let mut insert = tx.prepare_cached(ENQUEUE)?;
    let metadata: Option<i64> = insert
        .query_row(
            params![id, "wikidata", "metadata", mode, None::<i64>],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(dependency) = metadata {
        insert.query_row(
            params![id, provider_name(provider), "cover", mode, dependency],
            |_| Ok(()),
        )?;
    }
    Ok(())
}

/// Persist user requests before waking the scheduler. Each short chunk is independently resumable.
pub fn enqueue_games(
    lib: &Library,
    ids: &[i64],
    mode: Mode,
    provider: Provider,
) -> Result<(), String> {
    let pool = lib.pool()?;
    runtime()?
        .block_on(async {
            for chunk in ids.chunks(64) {
                if lib.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(sqlx::Error::Protocol("Library is shutting down".into()));
                }
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
                for &id in chunk {
                    if mode == Mode::Full {
                        sqlx::query(UPGRADE)
                            .bind(id)
                            .bind(provider_name(provider))
                            .execute(&mut *tx)
                            .await?;
                    }
                    let mode = mode_name(mode);
                    let metadata = sqlx::query_file_scalar!(
                        "src/library/jobs/enqueue.sql",
                        id,
                        "wikidata",
                        "metadata",
                        mode,
                        None::<i64>
                    )
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let Some(dependency) = metadata {
                        let provider = provider_name(provider);
                        sqlx::query_file_scalar!(
                            "src/library/jobs/enqueue.sql",
                            id,
                            provider,
                            "cover",
                            mode,
                            dependency
                        )
                        .fetch_one(&mut *tx)
                        .await?;
                    }
                }
                tx.commit().await?;
            }
            Ok::<_, sqlx::Error>(())
        })
        .map_err(|e| e.to_string())
}

/// Cancel queued or leased work for the selected games. An obsolete lease cannot publish afterwards.
pub fn cancel_games(lib: &Library, ids: &[i64]) -> Result<(), String> {
    let pool = lib.pool()?;
    runtime()?.block_on(async {
        for chunk in ids.chunks(256) {
            let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("UPDATE scrape_jobs SET status='cancelled',owner=NULL,lease_until=NULL,updated_at=unixepoch() WHERE status IN ('pending','running','retry_wait') AND game_id IN (");
            let mut items=query.separated(",");
            for id in chunk { items.push_bind(id); }
            query.push(")");
            query.build().execute(pool).await?;
        }
        Ok::<_,sqlx::Error>(())
    }).map_err(|e| e.to_string())
}

/// Discover work on a background thread. Disk, rather than the wake-up signal, is authoritative.
pub fn due_games(lib: &Library) -> Result<Vec<(i64, Mode)>, String> {
    let pool = lib.pool()?;
    let rows = runtime()?
        .block_on(sqlx::query_file!("src/library/jobs/due.sql").fetch_all(pool))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.game_id,
                if row.full == 1 {
                    Mode::Full
                } else {
                    Mode::Incremental
                },
            )
        })
        .collect())
}

/// A newer selection replaces a previous source for the same stage in the retry list.
pub fn failed_games(lib: &Library) -> Result<Vec<i64>, String> {
    let pool = lib.pool()?;
    let query =
        sqlx::query_scalar("SELECT DISTINCT game_id FROM latest_scrape_failures ORDER BY game_id");
    runtime()?
        .block_on(query.fetch_all(pool))
        .map_err(|e| e.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Metadata,
    Cover,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Cover => "cover",
        }
    }
}

#[derive(Debug)]
pub struct CoverClaim {
    pub(super) id: i64,
    pub(super) game: i64,
    pub(super) owner: String,
    pub(super) mode: Mode,
    pub(super) provider: Provider,
    pub(super) stage: Stage,
    pub(super) attempt: i64,
    pub(super) limit: i64,
}

impl CoverClaim {
    /// Commit in the same transaction as the new asset's reference. A lost lease rolls it all back.
    pub fn finish_cover(&self, db: &Connection) -> Result<(), String> {
        if self.finish(db, "ready").map_err(|e| e.to_string())? {
            Ok(())
        } else {
            Err("Cover job lease expired before publication".into())
        }
    }

    pub(super) fn finish(&self, db: &Connection, outcome: &str) -> rusqlite::Result<bool> {
        db.execute(
            "UPDATE scrape_jobs SET status='succeeded',outcome=?1,updated_at=unixepoch(),
            lease_until=NULL,last_error_code=NULL,last_error_message=NULL WHERE id=?2 AND owner=?3
            AND attempt_count=?4 AND status='running' AND lease_until>unixepoch()",
            params![outcome, self.id, self.owner, self.attempt],
        )
        .map(|n| n == 1)
    }

    pub(super) fn owns(&self, db: &Connection) -> rusqlite::Result<bool> {
        db.prepare_cached("SELECT 1 FROM scrape_jobs WHERE id=?1 AND owner=?2 AND attempt_count=?3 AND status='running' AND lease_until>unixepoch()")?
            .exists(params![self.id,self.owner,self.attempt])
    }
}

pub(super) fn prepare_targets(
    db: &mut Connection,
    ids: &[i64],
    mode: Mode,
    provider: Provider,
    resume: bool,
) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE target_games(game_id INTEGER PRIMARY KEY);
        CREATE TEMP TABLE target_jobs(id INTEGER PRIMARY KEY);",
    )?;
    recover(db)?;
    // Bounded transactions keep imports, manual edits and session checkpoints responsive during large refreshes.
    for chunk in ids.chunks(64) {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for &id in chunk {
            tx.prepare_cached("INSERT OR IGNORE INTO target_games VALUES(?)")?
                .execute([id])?;
            if !resume {
                enqueue(&tx, id, provider, mode)?;
            }
        }
        tx.commit()?;
    }
    db.execute(
        "INSERT INTO target_jobs SELECT j.id FROM scrape_jobs j JOIN target_games t USING(game_id)
        WHERE j.status IN ('pending','retry_wait','running')",
        [],
    )?;
    Ok(())
}

pub(super) fn claim(
    db: &mut Connection,
    owner: &str,
    stage: Stage,
    count: usize,
) -> rusqlite::Result<Vec<CoverClaim>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = tx
        .prepare_cached(include_str!("jobs/claim.sql"))?
        .query_map(
            params![owner, LEASE_SECONDS, stage.name(), count as i64],
            |r| {
                let p: String = r.get(3)?;
                let provider = match p.as_str() {
                    "wikidata" | "libretro" => Provider::Libretro,
                    "screenscraper" => Provider::ScreenScraper,
                    "igdb" => Provider::Igdb,
                    "custom" => Provider::Custom,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(CoverClaim {
                    id: r.get(0)?,
                    game: r.get(1)?,
                    mode: if r.get::<_, String>(2)? == "full" {
                        Mode::Full
                    } else {
                        Mode::Incremental
                    },
                    provider,
                    stage,
                    owner: owner.into(),
                    attempt: r.get(4)?,
                    limit: r.get(5)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tx.commit()?;
    Ok(out)
}

/// Expiration is sparse: the partial index reads only running rows whose lease elapsed.
pub(super) fn recover(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE scrape_jobs INDEXED BY scrape_leases SET
        status=CASE WHEN attempt_count>=max_attempts THEN 'failed' ELSE 'pending' END,
        last_error_code='lease_expired',last_error_message='Worker lease expired',
        next_attempt_at=0,lease_until=NULL,owner=NULL,updated_at=unixepoch()
        WHERE status='running' AND lease_until<=unixepoch()",
        [],
    )?;
    Ok(())
}

pub(super) fn renew(db: &Connection, owner: &str) -> rusqlite::Result<()> {
    recover(db)?;
    db.execute("UPDATE scrape_jobs SET lease_until=unixepoch()+? WHERE owner=? AND status='running' AND lease_until>unixepoch()", params![LEASE_SECONDS, owner])?;
    Ok(())
}

pub(super) fn release_owner(db: &Connection, owner: &str) -> rusqlite::Result<()> {
    db.execute("UPDATE scrape_jobs SET status='pending', next_attempt_at=0, attempt_count=MAX(0,attempt_count-1),
        owner=NULL,lease_until=NULL,updated_at=unixepoch() WHERE status='running' AND owner=?",[owner])?;
    Ok(())
}

pub(super) fn cancel(db: &Connection, owner: &str) -> rusqlite::Result<()> {
    db.execute("UPDATE scrape_jobs SET status='cancelled',owner=NULL,lease_until=NULL,updated_at=unixepoch()
       WHERE id IN target_jobs AND (status IN ('pending','retry_wait') OR status='running' AND owner=?)",[owner])?;
    Ok(())
}

pub(super) struct Failure {
    pub code: &'static str,
    pub message: &'static str,
    pub retry: bool,
    pub deadline: Option<i64>,
}

/// Serial-based sources can proceed on a manual refresh even when the release
/// provider cannot choose an edition or asks us to retry later.
pub(super) fn release_serial_covers(db: &Connection, metadata_job: i64) -> rusqlite::Result<()> {
    db.prepare_cached(
        "UPDATE scrape_jobs SET dependency=NULL, updated_at=unixepoch()
         WHERE dependency=?1 AND mode='full' AND status IN ('pending','retry_wait')
           AND provider IN ('libretro','screenscraper','custom')
           AND EXISTS(SELECT 1 FROM game_metadata m WHERE m.game_id=scrape_jobs.game_id
                      AND COALESCE(m.release_serial,m.disc_serial,'')<>'')",
    )?
    .execute([metadata_job])?;
    Ok(())
}

impl Failure {
    pub fn cover(error: &str) -> Self {
        let lower = error.to_ascii_lowercase();
        let (code, message, retry) =
            if lower.contains("429") || lower.contains("rate limit") || error == "retry" {
                ("limited", "Provider rate limit", true)
            } else if lower.contains("http 404") || lower.contains("http 410") {
                ("not_found", "Provider has no matching cover", false)
            } else if lower.contains("http 401")
                || lower.contains("http 403")
                || lower.contains("credentials")
                || lower.contains("requires eidoryn_")
            {
                (
                    "credentials",
                    "Provider credentials are missing or rejected",
                    false,
                )
            } else if error == "failed"
                || [
                    "timeout",
                    "timed out",
                    "network",
                    "connect",
                    "request failed",
                    "sending request",
                    "dns",
                    "http 5",
                    "failed to download",
                    "database is locked",
                    "recent cover",
                ]
                .iter()
                .any(|s| lower.contains(s))
            {
                (
                    "temporary",
                    "Provider or storage temporarily unavailable",
                    true,
                )
            } else {
                (
                    "invalid",
                    "Provider configuration, media or request is invalid",
                    false,
                )
            };
        Self {
            code,
            message,
            retry,
            deadline: None,
        }
    }
}

pub(super) fn fail(
    tx: &Transaction<'_>,
    claim: &CoverClaim,
    failure: &Failure,
) -> rusqlite::Result<()> {
    let now = super::http_retry::seconds(SystemTime::now());
    let retry = failure.retry && claim.attempt < claim.limit;
    // Random jitter avoids synchronising clients after a provider outage.
    let base = 5_i64 << (claim.attempt - 1).clamp(0, 8);
    let salt = uuid::Uuid::new_v4().as_u128() as u64;
    let jitter = (salt % (base as u64 + 1)) as i64;
    let mut due = (now + base + jitter).max(failure.deadline.unwrap_or(0));
    if failure.code == "limited" && claim.stage == Stage::Cover {
        let host = match claim.provider {
            Provider::Igdb => Some("api.igdb.com"),
            Provider::ScreenScraper => Some("api.screenscraper.fr"),
            Provider::Libretro => Some("raw.githubusercontent.com"),
            Provider::Custom => None,
        };
        let provider_deadline: Option<i64> = if let Some(host) = host {
            tx.query_row(
                "SELECT max(retry_after) FROM cover_rate_limits WHERE host=?",
                [host],
                |r| r.get(0),
            )?
        } else {
            // A custom provider records a sanitized origin/key in its per-game attempt.
            // Unrelated hosts must not delay this game's retry.
            tx.query_row(
                "SELECT max(h.retry_after) FROM cover_attempts a JOIN cover_rate_limits h
                ON instr(a.url,'https://'||h.host||'/')=1 OR instr(a.url,'https://'||h.host||':')=1
                WHERE a.game_id=? AND a.state='retry'",
                [claim.game],
                |r| r.get(0),
            )?
        };
        due = due.max(provider_deadline.unwrap_or(0));
    }
    let changed = tx.execute(
        "UPDATE scrape_jobs SET status=?1,last_error_code=?2,last_error_message=?3,
         next_attempt_at=?4,owner=NULL,lease_until=NULL,updated_at=unixepoch()
         WHERE id=?5 AND owner=?6 AND attempt_count=?7 AND status='running' AND lease_until>unixepoch()",
        params![
            if retry { "retry_wait" } else { "failed" },
            failure.code,
            failure.message,
            due,
            claim.id,
            claim.owner,
            claim.attempt
        ],
    )?;
    if changed == 1 && retry && claim.stage == Stage::Metadata && claim.mode == Mode::Full {
        release_serial_covers(tx, claim.id)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GameState {
    Pending,
    RetryWaiting,
    Cancelled,
    Skipped,
    Done,
}

pub(super) struct State {
    pub outcome: GameState,
    pub error: Option<String>,
}
pub(super) fn state(db: &Connection, id: i64) -> rusqlite::Result<State> {
    let mut read = db.prepare_cached(
        "SELECT count(*)=0 OR sum(CASE WHEN outcome IN ('skip_cover','skipped','not_found') THEN 0 ELSE 1 END)=0,
            max(CASE WHEN status IN ('failed','retry_wait') THEN
                printf('#%d %s/%s (%s, %d/%d): %s',id,provider,job_type,last_error_code,attempt_count,max_attempts,last_error_message) END),
            count(*) FILTER(WHERE status IN ('pending','running'))>0,
            count(*) FILTER(WHERE status='retry_wait')>0,
            count(*) FILTER(WHERE status='cancelled')>0
        FROM scrape_jobs AS j WHERE j.game_id=? AND EXISTS(SELECT 1 FROM target_jobs t WHERE t.id=j.id)"
    )?;
    read.query_row([id], |row| {
        let outcome = if row.get(3)? {
            GameState::RetryWaiting
        } else if row.get(2)? {
            GameState::Pending
        } else if row.get(4)? {
            GameState::Cancelled
        } else if row.get(0)? {
            GameState::Skipped
        } else {
            GameState::Done
        };
        Ok(State {
            outcome,
            error: row.get(1)?,
        })
    })
}

/// Persisted scrape-stage state shown in the library task log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    RetryWait,
    Failed,
    Cancelled,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct JobRecord {
    pub title: String,
    pub provider: String,
    pub cover: bool,
    pub status: JobStatus,
    pub skipped: bool,
    pub updated_at: i64,
}

/// Newest queued stages for games still in the library, including finished jobs.
pub fn recent_jobs(lib: &Library) -> Result<Vec<JobRecord>, String> {
    let pool = lib.pool()?;
    let query = sqlx::query_as(
        "SELECT COALESCE(m.user_title,m.auto_title,g.fallback_title) AS title,
                j.provider,j.job_type='cover' AS cover,j.status,
                COALESCE(j.outcome='skipped',0) AS skipped,j.updated_at
         FROM scrape_jobs j JOIN games g ON g.id=j.game_id
         LEFT JOIN game_metadata m ON m.game_id=g.id
         WHERE NOT g.hidden ORDER BY j.id DESC LIMIT 20",
    );
    runtime()?
        .block_on(query.fetch_all(pool))
        .map_err(|e| e.to_string())
}

/// Most recent failed stages, with bounded, credential-free diagnostics.
pub fn recent_failures(lib: &Library) -> Result<Vec<(i64, String)>, String> {
    let pool = lib.pool()?;
    let query = sqlx::query_as(
        "SELECT game_id,
          printf('#%d %s/%s (%s, %d/%d): %s',id,provider,job_type,last_error_code,
             attempt_count,max_attempts,last_error_message)
        FROM latest_scrape_failures ORDER BY updated_at DESC,id DESC LIMIT 8",
    );
    runtime()?
        .block_on(query.fetch_all(pool))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

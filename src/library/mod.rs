//! Desktop library index. Disc locations and statistics have independent identities
//! so renaming a file or correcting a release never changes a save-state key.
pub mod covers;
mod http_retry;
mod jobs;
pub mod metadata;
mod ordering;
mod scan;
pub mod scrape;
mod store;
mod transport;

pub use ordering::{GameMove, Placement};
use rusqlite::{Connection, TransactionBehavior};
pub use scan::{ScanPhase, ScanProgress, ScanReport, ScanRoot, TrackChoice};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};

type IndexResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct Library {
    path: PathBuf,
    pool: Arc<OnceLock<Result<sqlx::SqlitePool, String>>>,
    worker_connections: Option<Arc<Mutex<Vec<Connection>>>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    import_provider: Arc<Mutex<Option<covers::Provider>>>,
    additions: Option<(
        mpsc::SyncSender<i64>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: i64,
    pub platform: String,
    pub title: String,
    /// Unranked games follow the saved order, by increasing game ID.
    pub manual_position: Option<i64>,
    pub serial: Option<String>,
    pub region: Option<String>,
    pub locations: Vec<Location>,
    pub seconds: u64,
    pub last_played: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Location {
    pub id: i64,
    pub disc_id: i64,
    pub disc_number: i64,
    pub serial: Option<String>,
    pub path: PathBuf,
    pub available: bool,
    pub fast_boot: bool,
}

// PAL discs also exist outside Europe. Keep the release market for display
// while allowing European countries to match a disc identified as Europe.
fn european_market(market: &str) -> bool {
    matches!(
        market,
        "Europe"
            | "Austria"
            | "Belgium"
            | "Denmark"
            | "Finland"
            | "France"
            | "Germany"
            | "Greece"
            | "Ireland"
            | "Italy"
            | "Netherlands"
            | "Norway"
            | "Poland"
            | "Portugal"
            | "Russia"
            | "Scandinavia"
            | "Spain"
            | "Sweden"
            | "UK"
    )
}

const SCHEMA_VERSION: u32 = 7;
const SCHEMA: &str = include_str!("schema.sql");

impl Library {
    pub fn open(path: &Path) -> Result<Self, String> {
        let library = Self::deferred(path);
        library.pool()?;
        Ok(library)
    }

    /// Create a handle without any disk access. Its first database operation initializes the
    /// schema on that caller's thread. All clones wait for the same result.
    pub fn deferred(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            pool: Arc::default(),
            worker_connections: None,
            shutdown: Arc::default(),
            import_provider: Arc::default(),
            additions: None,
        }
    }

    /// Initialize a deferred library on a worker and report any startup error.
    pub fn wait_until_ready(&self) -> Result<(), String> {
        self.pool().map(|_| ())
    }

    fn pool(&self) -> Result<&sqlx::SqlitePool, String> {
        self.pool
            .get_or_init(|| {
                self.initialize()
                    .map_err(|e| format!("{}: {e}", self.path.display()))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn initialize(&self) -> IndexResult<sqlx::SqlitePool> {
        std::fs::create_dir_all(self.base_dir())?;
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&self.path)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = {
            let _enter = runtime()?.enter();
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(4)
                .connect_lazy_with(options)
        };
        self.migrate(&pool)?;
        Ok(pool)
    }

    /// Configure automatic jobs for subsequent imports, including on existing clones.
    pub fn configure_import_scraping(&mut self, provider: Option<covers::Provider>) {
        *self
            .import_provider
            .lock()
            .expect("import preferences lock poisoned") = provider;
    }

    /// Stop accepting work and return this instance's unfinished leases to the queue.
    pub fn request_scrape_shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn for_batch(&self) -> Self {
        let mut batch = self.clone();
        batch.worker_connections = Some(Arc::default());
        batch
    }

    /// Scoped provider workers reuse their SQLite statement/schema caches until this batch exits.
    fn worker_connection(&self) -> Result<WorkerConnection<'_>, String> {
        let idle = self.worker_connections.as_deref();
        let cached =
            idle.and_then(|pool| pool.lock().expect("worker connection pool poisoned").pop());
        let connection = match cached {
            Some(db) => db,
            None => self.connect()?,
        };
        Ok(WorkerConnection {
            connection: Some(connection),
            idle,
        })
    }

    fn queue_new_game(
        &self,
        tx: &rusqlite::Transaction<'_>,
        home: i64,
        game: i64,
    ) -> rusqlite::Result<()> {
        if home == game
            && !self.shutdown.load(std::sync::atomic::Ordering::Acquire)
            && let Some(provider) = *self
                .import_provider
                .lock()
                .expect("import preferences lock poisoned")
        {
            jobs::enqueue(tx, game, provider, scrape::Mode::Incremental)?;
        }
        Ok(())
    }

    fn notify_addition(&self, home: i64, game: i64) {
        // A verified copy gets a home ID for its own sessions, but shares the existing game card.
        if game == home
            && let Some((additions, lagged)) = &self.additions
            && matches!(
                additions.try_send(game),
                Err(std::sync::mpsc::TrySendError::Full(_))
            )
        {
            lagged.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Receive newly indexed game IDs after commit, including imports made by subsequent clones.
    pub fn watch_additions(&mut self) -> mpsc::Receiver<i64> {
        let (sender, receiver) = mpsc::sync_channel(256);
        self.additions = Some((sender, Arc::default()));
        receiver
    }

    /// A slow subscriber reloads its snapshot if the bounded addition stream overflowed.
    pub fn take_additions_overflow(&self) -> bool {
        self.additions
            .as_ref()
            .is_some_and(|(_, lagged)| lagged.swap(false, std::sync::atomic::Ordering::AcqRel))
    }

    pub fn default_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        #[cfg(target_os = "macos")]
        let root =
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"));
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")));
        root.map(|p| p.join("eidoryn/library.sqlite3"))
    }
    pub fn base_dir(&self) -> &Path {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }
    pub fn connect(&self) -> Result<Connection, String> {
        self.connection().map_err(|e| e.to_string())
    }
    fn connection(&self) -> IndexResult<Connection> {
        self.pool()?;
        Ok(self.raw_connection()?)
    }

    fn raw_connection(&self) -> rusqlite::Result<Connection> {
        let db = Connection::open(&self.path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.pragma_update(None, "foreign_keys", true)?;
        Ok(db)
    }
    fn sidecar_path(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }

    fn backup_before_upgrade(&self, db: &Connection, from: u32) -> IndexResult<()> {
        let destination = self.sidecar_path(&format!(".v{from}.bak"));
        let snapshot = tempfile::NamedTempFile::new_in(self.base_dir())?;
        db.execute(
            "VACUUM INTO CAST(? AS TEXT)",
            [snapshot.path().as_os_str().as_encoded_bytes()],
        )?;
        snapshot.as_file().sync_all()?;
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_file() => {
                // A rollback or an unrelated file may already occupy the conventional path.
                let archive = self.sidecar_path(&format!(".v{from}.{}.bak", uuid::Uuid::new_v4()));
                let mut previous = tempfile::NamedTempFile::new_in(self.base_dir())?;
                std::io::copy(
                    &mut std::fs::File::open(&destination)?,
                    previous.as_file_mut(),
                )?;
                previous.as_file().sync_all()?;
                previous.persist_noclobber(archive)?;
            }
            Ok(_) => {
                return Err(format!(
                    "Backup path is not a regular file: {}",
                    destination.display()
                )
                .into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        snapshot.persist(&destination)?;
        #[cfg(unix)]
        std::fs::File::open(self.base_dir())?.sync_all()?;
        Ok(())
    }
    fn migrate(&self, pool: &sqlx::SqlitePool) -> IndexResult<()> {
        let mut db = self.raw_connection()?;
        let from: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if from == SCHEMA_VERSION {
            db.pragma_update(None, "journal_mode", "WAL")?;
            return Ok(());
        }
        if from > SCHEMA_VERSION {
            return Err(format!("Unsupported library schema version {from}").into());
        }
        // The stable sidecar serializes the snapshot and both migration transactions.
        // Leaving the file in place keeps all processes locking the same inode.
        let migration_lock = std::fs::File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.sidecar_path(".migration.lock"))?;
        migration_lock.lock()?;
        let from: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if from == SCHEMA_VERSION {
            return Ok(());
        }
        if (1..SCHEMA_VERSION).contains(&from) {
            self.backup_before_upgrade(&db, from)?;
        }
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = tx.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?;
        if version > SCHEMA_VERSION {
            return Err(format!("Unsupported library schema version {version}").into());
        }
        if version == 0 {
            tx.execute_batch(SCHEMA)?;
            tx.execute_batch(metadata::SCHEMA)?;
        }
        if version < 2 {
            tx.execute_batch(include_str!("metadata/online.sql"))?;
            tx.execute_batch(include_str!("covers/libretro.sql"))?;
        }
        if version < 3 {
            metadata::migrate_local_cache(&tx)?;
        }
        if version < 4 {
            metadata::migrate_provenance(&tx)?;
            covers::migrate_private_sources(&tx)?;
            tx.pragma_update(None, "user_version", 4)?;
        }
        tx.commit()?;
        drop(db);
        runtime()?.block_on(async {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
            let current: i64 = sqlx::query_scalar("PRAGMA user_version")
                .fetch_one(&mut *tx)
                .await?;
            if !(4..=6).contains(&current) {
                return Err(format!("Unsupported library schema version {current}").into());
            }
            if current == 4 {
                sqlx::raw_sql(include_str!("jobs/schema.sql"))
                    .execute(&mut *tx)
                    .await?;
                sqlx::raw_sql(include_str!("optimization.sql"))
                    .execute(&mut *tx)
                    .await?;
            }
            if current <= 5 {
                sqlx::query("ALTER TABLE games ADD COLUMN manual_position INTEGER")
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::raw_sql(
                "ALTER TABLE game_metadata ADD COLUMN cache_generation INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version=7;",
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(())
        })
    }
}

pub(super) struct WorkerConnection<'pool> {
    connection: Option<Connection>,
    idle: Option<&'pool Mutex<Vec<Connection>>>,
}
impl std::ops::Deref for WorkerConnection<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("live connection")
    }
}
impl Drop for WorkerConnection<'_> {
    fn drop(&mut self) {
        if let (Some(pool), Some(db)) = (self.idle, self.connection.take()) {
            pool.lock()
                .expect("worker connection pool poisoned")
                .push(db);
        }
    }
}

/// One I/O runtime is shared by all library instances and provider clients.
pub(super) fn runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("library-io")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[cfg(unix)]
fn path_key(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_owned()
}
#[cfg(unix)]
fn key_path(key: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(key).into()
}
#[cfg(windows)]
fn path_key(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}
#[cfg(windows)]
fn key_path(key: Vec<u8>) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<_> = key
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect();
    std::ffi::OsString::from_wide(&wide).into()
}

#[cfg(test)]
mod index_tests;

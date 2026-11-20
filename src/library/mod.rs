//! Desktop library index. Disc locations and statistics have independent identities
//! so renaming a file or correcting a release never changes a save-state key.
pub mod covers;
mod http_retry;
pub mod metadata;
mod scan;
pub mod scrape;
mod store;

use rusqlite::{Connection, TransactionBehavior};
pub use scan::{ScanProgress, ScanReport, ScanRoot, TrackChoice};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

type IndexResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct Library {
    path: PathBuf,
    additions: Option<mpsc::Sender<i64>>,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: i64,
    pub title: String,
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

const SCHEMA: &str = include_str!("schema.sql");

impl Library {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let library = Self {
            path: path.to_owned(),
            additions: None,
        };
        library
            .migrate()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(library)
    }
    /// Receive newly indexed game IDs after commit, including imports made by subsequent clones.
    pub fn watch_additions(&mut self) -> mpsc::Receiver<i64> {
        let (sender, receiver) = mpsc::channel();
        self.additions = Some(sender);
        receiver
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
        self.path.parent().unwrap_or(Path::new("."))
    }
    pub fn connect(&self) -> Result<Connection, String> {
        self.connection().map_err(|e| e.to_string())
    }
    fn connection(&self) -> rusqlite::Result<Connection> {
        let db = Connection::open(&self.path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.pragma_update(None, "foreign_keys", true)?;
        Ok(db)
    }
    fn migrate(&self) -> IndexResult<()> {
        let mut db = self.connection()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = tx.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?;
        if version > 2 {
            return Err(format!("Unsupported library schema version {version}").into());
        }
        if version == 0 {
            tx.execute_batch(SCHEMA)?;
            tx.execute_batch(metadata::SCHEMA)?;
        }
        if version < 2 {
            tx.execute_batch(include_str!("metadata/online.sql"))?;
            tx.execute_batch(include_str!("covers/libretro.sql"))?;
            tx.pragma_update(None, "user_version", 2)?;
        }
        tx.commit()?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        Ok(())
    }
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

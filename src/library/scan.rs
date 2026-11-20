use super::{
    IndexResult, Library, key_path, path_key,
    store::{Change, Mode},
};
use eidoryn_ps1::disc::DiscImage;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// The operation currently running on the scanner worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanPhase {
    #[default]
    Discovering,
    Indexing,
    CheckingAvailability,
}

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub phase: ScanPhase,
    pub total: usize,
    pub processed: usize,
    pub current_file: Option<PathBuf>,
    pub added: usize,
    pub existing: usize,
    pub pending: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub choices: Vec<TrackChoice>,
}
pub type ScanProgress = ScanReport;
#[derive(Clone, Debug)]
pub struct TrackChoice {
    pub track: PathBuf,
    pub cues: Vec<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct ScanRoot {
    pub path: PathBuf,
    pub recursive: bool,
    pub available: bool,
}
fn ext(path: &Path, kind: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(kind))
}

fn scan_path(path: &Path) -> std::io::Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let absolute = std::path::absolute(path)?;
    // An unavailable image or directory must still match its stored path,
    // including when the caller reached it through a symlink or a junction.
    for parent in absolute.ancestors().skip(1) {
        if let Ok(canonical) = parent.canonicalize()
            && let Ok(suffix) = absolute.strip_prefix(parent)
        {
            return Ok(canonical.join(suffix));
        }
    }
    #[cfg(windows)]
    {
        // Even the drive or network share is offline. Keep the verbatim path
        // form returned by Windows when the same volume was available.
        use std::path::{Component, Prefix};
        let mut parts = absolute.components();
        if let Some(Component::Prefix(prefix)) = parts.next() {
            let root = match prefix.kind() {
                Prefix::Disk(drive) => PathBuf::from(format!(r"\\?\{}:\", char::from(drive))),
                Prefix::UNC(server, share) => Path::new(r"\\?\UNC").join(server).join(share),
                _ => return Ok(absolute),
            };
            if parts.next() == Some(Component::RootDir) {
                return Ok(root.join(parts.as_path()));
            }
        }
    }
    Ok(absolute)
}

fn cue_references(cue: &Path, track: &Path) -> bool {
    DiscImage::cue_dependencies(cue).is_ok_and(|parts| {
        parts
            .iter()
            .any(|p| p.canonicalize().is_ok_and(|p| p == track))
    })
}
fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Relaxed)
}
impl ScanReport {
    fn failure(&mut self, path: &Path, error: impl std::fmt::Display) {
        self.failed += 1;
        self.errors.push(format!("{}: {error}", path.display()));
    }
}
struct Scan<'a, F> {
    library: &'a Library,
    db: Connection,
    cancel: &'a AtomicBool,
    progress: F,
    report: ScanReport,
}
impl<F: FnMut(&ScanProgress)> Scan<'_, F> {
    fn notify(&mut self) {
        (self.progress)(&self.report);
    }
    fn files(&mut self, files: &BTreeSet<PathBuf>, mode: Mode) -> IndexResult<()> {
        self.report.phase = ScanPhase::Indexing;
        let mut tracks = HashSet::new();
        for path in files
            .iter()
            .filter(|p| ext(p, "cue"))
            .chain(files.iter().filter(|p| !ext(p, "cue")))
        {
            if cancelled(self.cancel) {
                break;
            }
            self.report.current_file = Some(path.clone());
            self.report.processed += 1;
            if tracks.contains(path)
                || mode == Mode::Directory
                    && ext(path, "bin")
                    && self
                        .db
                        .prepare_cached("SELECT 1 FROM cue_tracks WHERE path=?")?
                        .exists([path_key(path)])?
            {
                self.notify();
                continue;
            }
            if ext(path, "bin") {
                let linked = Library::known_cues(&self.db, path)?;
                if mode == Mode::Directory && !linked.is_empty() {
                    self.notify();
                    continue;
                }
                if mode == Mode::Explicit {
                    let cues = Library::find_cues(path, linked);
                    if !cues.is_empty() {
                        self.report.pending += 1;
                        self.report.choices.push(TrackChoice {
                            track: path.clone(),
                            cues,
                        });
                        self.notify();
                        continue;
                    }
                }
            }
            match self.library.index(&mut self.db, path, mode, None) {
                Ok(result) => {
                    match result.change {
                        Change::Added => self.report.added += 1,
                        Change::Existing => self.report.existing += 1,
                        Change::Ignored => {}
                    }
                    self.report.pending += usize::from(result.pending);
                    for dep in result.dependencies {
                        let absolute = dep.canonicalize().unwrap_or(dep);
                        if mode == Mode::Directory {
                            self.db
                                .prepare_cached("INSERT OR IGNORE INTO cue_tracks VALUES(?)")?
                                .execute([path_key(&absolute)])?;
                        } else {
                            tracks.insert(absolute);
                        }
                    }
                }
                Err(error) => {
                    self.db.execute(
                        "UPDATE locations SET available=0 WHERE current AND path=?",
                        [path_key(path)],
                    )?;
                    self.report.failure(path, error);
                }
            }
            self.notify();
        }
        Ok(())
    }
    /// Index complete images from this directory before walking any child. A BIN can be
    /// named by a CUE in a different directory, so defer only those ambiguous paths.
    fn scan_folder(
        &mut self,
        folder: &Path,
        recursive: bool,
        folders: &mut Vec<PathBuf>,
        inaccessible: &mut Vec<PathBuf>,
    ) -> IndexResult<()> {
        self.report.phase = ScanPhase::Discovering;
        self.report.current_file = Some(folder.to_owned());
        self.notify();
        if cancelled(self.cancel) {
            return Ok(());
        }
        let entries = match fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) => {
                self.report.failure(folder, error);
                inaccessible.push(folder.to_owned());
                self.notify();
                return Ok(());
            }
        };
        let mut ready = BTreeSet::new();
        for entry in entries {
            if cancelled(self.cancel) {
                return Ok(());
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.report.failure(folder, error);
                    self.notify();
                    continue;
                }
            };
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    self.report.current_file = Some(path.clone());
                    self.report.failure(&path, error);
                    self.notify();
                    continue;
                }
            };
            if kind.is_dir() && recursive {
                folders.push(path);
            } else if DiscImage::supports_path(&path)
                && (kind.is_file() || kind.is_symlink() && path.is_file())
            {
                let path = path.canonicalize().unwrap_or(path);
                let deferred = ext(&path, "bin");
                let added = self
                    .db
                    .prepare_cached("INSERT OR IGNORE INTO discovered VALUES(?,?)")?
                    .execute(params![path_key(&path), deferred])?;
                if added != 0 {
                    self.report.total += 1;
                    if self.report.total.is_multiple_of(64) {
                        self.report.current_file = Some(folder.to_owned());
                        self.notify();
                    }
                    if !deferred {
                        ready.insert(path);
                    }
                }
            }
        }
        self.files(&ready, Mode::Directory)
    }

    fn finish_bins(&mut self) -> IndexResult<()> {
        let mut after = Vec::new();
        while !cancelled(self.cancel) {
            let page = self
                .db
                .prepare_cached(
                    "SELECT path FROM discovered WHERE deferred=1 AND path>? ORDER BY path LIMIT 128",
                )?
                .query_map([&after], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let Some(last) = page.last() else { break };
            after.clone_from(last);
            self.files(&page.into_iter().map(key_path).collect(), Mode::Directory)?;
        }
        Ok(())
    }

    fn reconcile(
        &mut self,
        root: &Path,
        recursive: bool,
        inaccessible: &[PathBuf],
    ) -> IndexResult<()> {
        if cancelled(self.cancel) {
            return Ok(());
        }
        self.report.phase = ScanPhase::CheckingAvailability;
        self.report.current_file = Some(root.to_owned());
        self.notify();
        self.stage_unseen(root, recursive)?;
        if cancelled(self.cancel) {
            return Ok(());
        }
        // Reserve only undiscovered locations. An importer that sees one of these paths
        // clears its token even if its last_seen_at timestamp is in the same second.
        let token = uuid::Uuid::new_v4().to_string();
        let reserved = self.db.execute(
            "UPDATE locations SET availability_check=?1 FROM unseen u
             WHERE locations.id=u.id AND locations.path=u.path AND current AND available",
            [&token],
        )?;
        if reserved == 0 {
            return self.finish_reconcile(root, inaccessible, &token);
        }
        let result = self
            .check_unseen()
            .and_then(|()| self.finish_reconcile(root, inaccessible, &token));
        let cleanup = self.db.execute(
            "UPDATE locations SET availability_check=NULL
             WHERE id IN (SELECT id FROM unseen) AND availability_check=?",
            [&token],
        );
        result?;
        cleanup?;
        Ok(())
    }

    fn stage_unseen(&self, root: &Path, recursive: bool) -> IndexResult<()> {
        self.db.execute_batch(
            "CREATE TEMP TABLE unseen(id INTEGER PRIMARY KEY, path BLOB NOT NULL,
                 missing INTEGER NOT NULL DEFAULT 0)",
        )?;
        let mut after = 0;
        let mut select = self.db.prepare_cached(
            "SELECT id,path FROM locations l WHERE current AND available AND id>?
             AND NOT EXISTS(SELECT 1 FROM discovered d WHERE d.path=l.path)
             ORDER BY id LIMIT 128",
        )?;
        let mut insert = self
            .db
            .prepare_cached("INSERT INTO unseen(id,path) VALUES(?,?)")?;
        while !cancelled(self.cancel) {
            let rows = select
                .query_map([after], |r| Ok((r.get::<_, i64>(0)?, key_path(r.get(1)?))))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let Some((last, _)) = rows.last() else { break };
            after = *last;
            for (id, path) in rows {
                let covered = if recursive {
                    path.starts_with(root)
                } else {
                    path.parent() == Some(root)
                };
                if covered {
                    insert.execute(params![id, path_key(&path)])?;
                }
            }
        }
        Ok(())
    }

    fn check_unseen(&mut self) -> IndexResult<()> {
        let mut after = 0;
        while !cancelled(self.cancel) {
            let page = self
                .db
                .prepare_cached("SELECT id,path FROM unseen WHERE id>? ORDER BY id LIMIT 128")?
                .query_map([after], |r| Ok((r.get::<_, i64>(0)?, key_path(r.get(1)?))))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let Some((last, _)) = page.last() else { break };
            after = *last;
            for (id, path) in page {
                if cancelled(self.cancel) {
                    return Ok(());
                }
                if !path.is_file() {
                    self.db
                        .prepare_cached("UPDATE unseen SET missing=1 WHERE id=?")?
                        .execute([id])?;
                }
                self.report.current_file = Some(path);
                self.notify();
            }
        }
        Ok(())
    }

    fn finish_reconcile(
        &mut self,
        root: &Path,
        inaccessible: &[PathBuf],
        token: &str,
    ) -> IndexResult<()> {
        if cancelled(self.cancel) {
            return Ok(());
        }
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE locations SET available=0,availability_check=NULL FROM unseen u
             WHERE u.missing AND locations.id=u.id AND locations.path=u.path
                 AND current AND available AND availability_check=?",
            [token],
        )?;
        tx.execute(
            "UPDATE roots SET available=?,added=?,existing=?,pending=?,failed=? WHERE path=?",
            params![
                !inaccessible.iter().any(|p| p == root),
                self.report.added,
                self.report.existing,
                self.report.pending,
                self.report.failed,
                path_key(root)
            ],
        )?;
        if cancelled(self.cancel) {
            return Ok(());
        }
        tx.commit()?;
        Ok(())
    }
}

impl Library {
    pub fn roots(&self) -> Result<Vec<ScanRoot>, String> {
        (|| -> IndexResult<_> {
            Ok(self
                .connection()?
                .prepare("SELECT path,recursive,available FROM roots ORDER BY path")?
                .query_map([], |r| {
                    Ok(ScanRoot {
                        path: key_path(r.get(0)?),
                        recursive: r.get(1)?,
                        available: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })()
        .map_err(|e| e.to_string())
    }
    pub fn scan_files(
        &self,
        paths: &[PathBuf],
        cancel: &AtomicBool,
        progress: impl FnMut(&ScanProgress),
    ) -> Result<ScanReport, String> {
        let mut scan = Scan {
            library: self,
            db: self.connect()?,
            cancel,
            progress,
            report: ScanReport::default(),
        };
        let files = paths
            .iter()
            .map(|p| scan_path(p).unwrap_or_else(|_| p.clone()))
            .collect::<BTreeSet<_>>();
        scan.report.total = files.len();
        scan.files(&files, Mode::Explicit)
            .map_err(|e| e.to_string())?;
        Ok(scan.report)
    }
    pub fn scan_directory(
        &self,
        path: &Path,
        recursive: bool,
        cancel: &AtomicBool,
        progress: impl FnMut(&ScanProgress),
    ) -> Result<ScanReport, String> {
        self.walk(path, recursive, cancel, progress)
            .map_err(|e| e.to_string())
    }
    fn walk(
        &self,
        path: &Path,
        recursive: bool,
        cancel: &AtomicBool,
        progress: impl FnMut(&ScanProgress),
    ) -> IndexResult<ScanReport> {
        let root = scan_path(path)?;
        let key = path_key(&root);
        let mut scan = Scan {
            library: self,
            db: self.connection()?,
            cancel,
            progress,
            report: ScanReport::default(),
        };
        scan.db.execute(
            "INSERT INTO roots(path, recursive) VALUES(?, ?)
             ON CONFLICT(path) DO UPDATE SET recursive = excluded.recursive",
            params![key, recursive],
        )?;
        scan.db.execute_batch("CREATE TEMP TABLE discovered(path BLOB PRIMARY KEY, deferred INTEGER NOT NULL) WITHOUT ROWID;
            CREATE INDEX deferred_bins ON discovered(path) WHERE deferred=1;
            CREATE TEMP TABLE cue_tracks(path BLOB PRIMARY KEY) WITHOUT ROWID;")?;
        let mut folders = vec![root.clone()];
        let mut inaccessible = Vec::new();
        while let Some(folder) = folders.pop() {
            if cancelled(cancel) {
                return Ok(scan.report);
            }
            scan.scan_folder(&folder, recursive, &mut folders, &mut inaccessible)?;
        }
        scan.finish_bins()?;
        scan.reconcile(&root, recursive, &inaccessible)?;
        Ok(scan.report)
    }
    fn known_cues(db: &Connection, path: &Path) -> rusqlite::Result<Vec<PathBuf>> {
        let mut query = db.prepare_cached(
            "SELECT l.path FROM parts p JOIN locations l ON l.id = p.location_id
             WHERE p.path = ? AND l.path <> p.path AND l.current AND l.available",
        )?;
        let candidates = query
            .query_map([path_key(path)], |row| Ok(key_path(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(candidates.into_iter().filter(|p| p.is_file()).collect())
    }
    fn find_cues(path: &Path, mut candidates: Vec<PathBuf>) -> Vec<PathBuf> {
        // CUE files can name tracks in child directories. Check the track's
        // ancestors, never recurse into unrelated directory trees.
        candidates.retain(|cue| cue_references(cue, path));
        for parent in path.ancestors().skip(1) {
            let Ok(entries) = fs::read_dir(parent) else {
                continue;
            };
            for item in entries.flatten() {
                let cue = item.path();
                if ext(&cue, "cue") && !candidates.contains(&cue) && cue_references(&cue, path) {
                    candidates.push(cue);
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        candidates
    }
    pub fn track_choices(&self, path: &Path) -> Result<Vec<PathBuf>, String> {
        if !ext(path, "bin") {
            return Ok(Vec::new());
        }
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let known = Self::known_cues(&self.connect()?, &path).map_err(|e| e.to_string())?;
        Ok(Self::find_cues(&path, known))
    }
}

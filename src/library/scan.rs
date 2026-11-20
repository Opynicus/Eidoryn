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

#[derive(Clone, Debug, Default)]
pub struct ScanReport {
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
        let mut tracks = HashSet::new();
        for path in files
            .iter()
            .filter(|p| ext(p, "cue"))
            .chain(files.iter().filter(|p| !ext(p, "cue")))
        {
            if cancelled(self.cancel) {
                break;
            }
            if tracks.contains(path) {
                continue;
            }
            if ext(path, "bin") {
                let linked = Library::known_cues(&self.db, path)?;
                if mode == Mode::Directory && !linked.is_empty() {
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
                        tracks.insert(absolute);
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
}

impl Library {
    pub fn roots(&self) -> Result<Vec<ScanRoot>, String> {
        (|| {
            self.connection()?
                .prepare("SELECT path,recursive,available FROM roots ORDER BY path")?
                .query_map([], |r| {
                    Ok(ScanRoot {
                        path: key_path(r.get(0)?),
                        recursive: r.get(1)?,
                        available: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
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
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .collect();
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
        let root = path.canonicalize().or_else(|_| std::path::absolute(path))?;
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
        let mut folders = vec![root.clone()];
        let mut files = BTreeSet::new();
        let mut inaccessible = Vec::new();
        while let Some(folder) = folders.pop() {
            if cancelled(cancel) {
                return Ok(scan.report);
            }
            let iterator = match fs::read_dir(&folder) {
                Ok(it) => it,
                Err(error) => {
                    scan.report.failure(&folder, error);
                    inaccessible.push(folder);
                    scan.notify();
                    continue;
                }
            };
            for entry in iterator {
                if cancelled(cancel) {
                    return Ok(scan.report);
                }
                let entry = match entry {
                    Ok(v) => v,
                    Err(e) => {
                        scan.report.failure(&folder, e);
                        continue;
                    }
                };
                let path = entry.path();
                let kind = match entry.file_type() {
                    Ok(v) => v,
                    Err(e) => {
                        scan.report.failure(&path, e);
                        continue;
                    }
                };
                if kind.is_dir() && recursive {
                    folders.push(path);
                } else if DiscImage::supports_path(&path)
                    && (kind.is_file() || kind.is_symlink() && path.is_file())
                {
                    files.insert(path.canonicalize().unwrap_or(path));
                }
            }
        }
        scan.files(&files, Mode::Directory)?;
        if !cancelled(cancel) {
            let online = !inaccessible.contains(&root);
            // An offline root keeps its rows and histories. Only availability is reset.
            let tx = scan.db.transaction()?;
            let indexed = tx
                .prepare("SELECT id,path FROM locations WHERE current AND available")?
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, key_path(r.get(1)?))))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, path) in indexed {
                if (if recursive {
                    path.starts_with(&root)
                } else {
                    path.parent() == Some(&root)
                }) && (inaccessible.iter().any(|dir| path.starts_with(dir)) || !path.is_file())
                {
                    tx.execute("UPDATE locations SET available=0 WHERE id=?", [id])?;
                }
            }
            tx.execute(
                "UPDATE roots SET available=?,added=?,existing=?,pending=?,failed=? WHERE path=?",
                params![
                    online,
                    scan.report.added,
                    scan.report.existing,
                    scan.report.pending,
                    scan.report.failed,
                    key
                ],
            )?;
            tx.commit()?;
        }
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

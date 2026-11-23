//! Shared filesystem layout for immutable checkpoint history and a replaceable quick slot.
use super::persistence::atomic_write;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MIN_REWIND_SECONDS: u16 = 5;
pub const MAX_REWIND_SECONDS: u16 = 120;
pub const DEFAULT_REWIND_SECONDS: u16 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuickSlot {
    Empty,
    Saved(SystemTime),
    #[default]
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateInfo {
    pub path: PathBuf,
    pub name: String,
    pub modified: SystemTime,
}

pub(crate) struct StateDirectory {
    pub(super) directory: PathBuf,
    pub(super) name: String,
    extension: &'static str,
}

impl StateDirectory {
    pub fn file_prefix(title: &str) -> String {
        let mut name: String = title
            .chars()
            .take(80)
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        while name.len() > 120 {
            name.pop();
        }
        if name.is_empty() { "game".into() } else { name }
    }

    pub fn new(directory: PathBuf, name: String, extension: &'static str) -> Self {
        Self {
            directory,
            name,
            extension,
        }
    }

    pub fn list(&self) -> Result<Vec<SaveStateInfo>, String> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Cannot list save states: {error}")),
        };
        let mut states = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("Cannot list save states: {e}"))?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == self.extension)
                && entry.file_type().map_err(|e| e.to_string())?.is_file()
            {
                let name = path.file_stem().unwrap_or_default().to_string_lossy();
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(|e| e.to_string())?;
                states.push(SaveStateInfo {
                    name: name.into_owned(),
                    path,
                    modified,
                });
            }
        }
        states.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| b.path.cmp(&a.path))
        });
        Ok(states)
    }

    /// Rename a checkpoint inside this game's directory without replacing another file.
    pub fn rename(&self, path: &Path, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        let stem = name
            .split('.')
            .next()
            .unwrap_or(name)
            .trim_end()
            .to_ascii_uppercase();
        if name.is_empty()
            || name.len() > 240
            || name.ends_with('.')
            || name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            })
        {
            return Err("Choose a nonempty snapshot name without path separators or reserved filename characters (up to 240 UTF-8 bytes)".into());
        }
        self.history_metadata(path)
            .map_err(|error| format!("Cannot rename snapshot {}: {error}", path.display()))?;
        let destination = path.with_file_name(format!("{name}.{}", self.extension));
        if destination == path {
            return Ok(destination);
        }
        // Hard-link publication is create-new on both Unix and Windows. It also
        // preserves save time; fs::rename would overwrite an existing target on Unix.
        fs::hard_link(path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("A snapshot named '{name}' already exists")
            } else {
                format!("Cannot rename snapshot: {error}")
            }
        })?;
        if let Err(error) = fs::remove_file(path) {
            let cleanup = fs::remove_file(&destination);
            return Err(if let Err(cleanup) = cleanup {
                format!(
                    "Cannot remove the old snapshot name: {error}; cannot remove duplicate {}: {cleanup}",
                    destination.display()
                )
            } else {
                format!("Cannot rename snapshot: {error}")
            });
        }
        Ok(destination)
    }

    /// Delete one historical checkpoint if it still has the modification time shown to the user.
    pub fn delete(&self, path: &Path, modified: SystemTime) -> Result<(), String> {
        let failure = |error| format!("Cannot delete snapshot {}: {error}", path.display());
        let metadata = self.history_metadata(path).map_err(failure)?;
        if metadata.modified().map_err(failure)? != modified {
            return Err(format!(
                "Snapshot changed since it was selected: {}",
                path.display()
            ));
        }
        fs::remove_file(path).map_err(failure)
    }

    pub(crate) fn history_metadata(&self, path: &Path) -> std::io::Result<fs::Metadata> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Expected a history snapshot in the current game's directory",
            )
        };
        let parent = path.parent().ok_or_else(invalid)?;
        if path.extension().is_none_or(|ext| ext != self.extension)
            || fs::canonicalize(parent)? != fs::canonicalize(&self.directory)?
        {
            return Err(invalid());
        }
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            return Err(invalid());
        }
        Ok(metadata)
    }

    pub fn save(&self, bytes: &[u8]) -> Result<PathBuf, String> {
        fs::create_dir_all(&self.directory)
            .map_err(|e| format!("Cannot create save directory: {e}"))?;
        // create-new publication never overwrites another checkpoint, even if clocks collide.
        for _ in 0..10 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?;
            let path = self.directory.join(format!(
                "{}_{}.{:09}Z.{}",
                self.name,
                timestamp(stamp.as_secs())?,
                stamp.subsec_nanos(),
                self.extension
            ));
            match atomic_write(&path, bytes, true) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("Cannot save state {}: {error}", path.display())),
            }
        }
        Err("Cannot create a unique save state timestamp".into())
    }

    pub(crate) fn quick_slot_modified(&self) -> Result<Option<SystemTime>, String> {
        let path = self.quick_path();
        match fs::metadata(&path) {
            Ok(file) if file.is_file() => {
                file.modified().map(Some).map_err(|error| error.to_string())
            }
            Ok(_) => Err(format!("Quick slot is not a file: {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "Cannot inspect quick slot {}: {error}",
                path.display()
            )),
        }
    }

    pub fn quick_path(&self) -> PathBuf {
        self.directory
            .join("quick")
            .join(format!("slot.{}", self.extension))
    }

    /// Replace only this game's dedicated slot; historical checkpoints are untouched.
    pub fn save_quick(&self, bytes: &[u8]) -> Result<PathBuf, String> {
        let path = self.quick_path();
        fs::create_dir_all(path.parent().expect("quick slot has a directory"))
            .map_err(|e| format!("Cannot create quick-save directory: {e}"))?;
        atomic_write(&path, bytes, false)
            .map_err(|e| format!("Cannot quick-save state {}: {e}", path.display()))?;
        Ok(path)
    }
}

fn timestamp(seconds: u64) -> Result<String, String> {
    let mut days = seconds / 86_400;
    if days > 2_932_896 {
        return Err("Save state date exceeds year 9999".into());
    }
    let mut year = 1970;
    loop {
        let length = if leap_year(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    let months = [
        31,
        if leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for length in months {
        if days < length {
            break;
        }
        days -= length;
        month += 1;
    }
    Ok(format!(
        "{year:04}-{month:02}-{:02}T{:02}-{:02}-{:02}",
        days + 1,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    ))
}

/// Human-readable UTC date and time for a checkpoint, independent of its filename.
pub fn checkpoint_time(modified: SystemTime) -> Option<String> {
    let stamp = timestamp(modified.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let (date, clock) = stamp.split_once('T')?;
    Some(format!("{date} {} UTC", clock.replace('-', ":")))
}

fn leap_year(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_sort_in_calendar_order() {
        assert_eq!(timestamp(0).unwrap(), "1970-01-01T00-00-00");
        assert_eq!(
            checkpoint_time(UNIX_EPOCH).as_deref(),
            Some("1970-01-01 00:00:00 UTC")
        );
        assert_eq!(timestamp(951_782_400).unwrap(), "2000-02-29T00-00-00");
        assert_eq!(timestamp(1_794_700_800).unwrap(), "2026-11-15T00-00-00");
    }
}

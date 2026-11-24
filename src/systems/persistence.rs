//! Atomic replacement or create-new publication of emulator files.
use std::{
    ffi::OsString,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().expect("validated file filename"));
    name.push(suffix);
    path.with_file_name(name)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8], create: bool) -> std::io::Result<()> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = sibling(path, &format!(".{}.{sequence}.tmp", std::process::id()));
    let mut file = File::create_new(&temp_path)?;
    let temp = Temporary(temp_path);
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if create {
        // Publishing a new file must never replace a file that appeared meanwhile.
        fs::hard_link(&temp.0, path)?;
    } else {
        fs::rename(&temp.0, path)?;
    }
    #[cfg(unix)]
    File::open(path.parent().expect("validated file parent"))?.sync_all()?;
    Ok(())
}

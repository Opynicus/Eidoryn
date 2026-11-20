//! An advisory lock identifies live owners across emulator windows and processes.
use crate::library::Library;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::PathBuf,
};
use uuid::Uuid;

pub(super) struct SessionLock {
    file: Option<File>,
    path: PathBuf,
}

impl SessionLock {
    pub(super) fn acquire(lib: &Library, id: Uuid) -> Result<Self, TryLockError> {
        let dir = lib.base_dir().join("session-locks");
        fs::create_dir_all(&dir).map_err(TryLockError::Error)?;
        let path = dir.join(id.to_string());
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(TryLockError::Error)?;
        file.try_lock()?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn close_interrupted(lib: &Library) -> Result<(), String> {
    let db = lib.connect()?;
    let active = db
        .prepare(
            "SELECT session_id FROM sessions WHERE started_at IS NOT NULL AND ended_at IS NULL",
        )
        .and_then(|mut query| {
            query
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?;
    for key in active {
        let id = Uuid::parse_str(&key).map_err(|e| e.to_string())?;
        let _owner = match SessionLock::acquire(lib, id) {
            Ok(owner) => owner,
            Err(TryLockError::WouldBlock) => continue,
            Err(e) => return Err(e.to_string()),
        };
        db.execute(
            "UPDATE sessions SET ended_at=last_played,exit_reason='interrupted'
                    WHERE session_id=? AND ended_at IS NULL",
            [&key],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

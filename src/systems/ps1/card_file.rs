//! Host-side card persistence. Serial devices never perform filesystem I/O.
use crate::systems::persistence::{atomic_write, sibling};
use crate::systems::ps1::memory_card::{CARD_SIZE, Cards, MemoryCard};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct CardStatus {
    pub path: Option<PathBuf>,
    pub pending: bool,
    pub reads: u64,
    pub writes: u64,
}

struct CardFile {
    path: PathBuf,
    saved_revision: Option<u64>,
    // Lock a stable sibling inode: the card inode is replaced by atomic saves.
    _lock: File,
}

pub struct CardStore {
    slots: [Option<CardFile>; 2],
    last_flush: Instant,
}

impl Default for CardStore {
    fn default() -> Self {
        Self {
            slots: [None, None],
            last_flush: Instant::now(),
        }
    }
}

impl CardStore {
    pub fn open(paths: [Option<PathBuf>; 2]) -> Result<(Self, Cards), String> {
        let mut store = Self::default();
        let mut cards = [None, None];
        for (slot, path) in paths.into_iter().enumerate() {
            if let Some(path) = path {
                let (file, card) = CardFile::open(&path)?;
                store.slots[slot] = Some(file);
                cards[slot] = Some(card);
            }
        }
        Ok((store, cards))
    }

    pub fn status(&self, cards: &Cards) -> [Option<CardStatus>; 2] {
        std::array::from_fn(|slot| {
            cards[slot].as_ref().map(|card| CardStatus {
                path: self.slots[slot].as_ref().map(|file| file.path.clone()),
                pending: self.slots[slot]
                    .as_ref()
                    .is_some_and(|file| file.saved_revision != Some(card.revision())),
                reads: card.reads,
                writes: card.writes,
            })
        })
    }

    /// Batches many 128-byte guest writes without waiting until application exit.
    pub fn flush_if_due(&mut self, cards: &Cards) -> Result<(), String> {
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush(cards)?;
        }
        Ok(())
    }

    pub fn flush(&mut self, cards: &Cards) -> Result<(), String> {
        for (file, card) in self.slots.iter_mut().zip(cards) {
            if let (Some(file), Some(card)) = (file, card)
                && file.saved_revision != Some(card.revision())
            {
                atomic_write(&file.path, card.bytes(), false)
                    .map_err(|error| format!("Cannot save memory card {}: {error}. Data is still in memory; retry Flush saves.", file.path.display()))?;
                file.saved_revision = Some(card.revision());
            }
        }
        self.last_flush = Instant::now();
        Ok(())
    }

    /// A restored timeline may have different bytes with the same revision count.
    pub(super) fn invalidate(&mut self) {
        for file in self.slots.iter_mut().flatten() {
            file.saved_revision = None;
        }
    }
}

impl CardFile {
    fn open(requested: &Path) -> Result<(Self, MemoryCard), String> {
        let open = || -> Result<(Self, MemoryCard), String> {
            let absolute = std::path::absolute(requested).map_err(|e| e.to_string())?;
            let parent = absolute
                .parent()
                .ok_or("Memory card path needs a parent directory")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let name = absolute
                .file_name()
                .ok_or("Memory card path needs a filename")?;
            let path = match fs::canonicalize(&absolute) {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::canonicalize(parent)
                        .map_err(|e| e.to_string())?
                        .join(name)
                }
                Err(error) => return Err(error.to_string()),
            };
            if path.exists() && !fs::metadata(&path).map_err(|e| e.to_string())?.is_file() {
                return Err("Select a regular raw 128 KiB memory card file".into());
            }
            let lock_path = sibling(&path, ".lock");
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .map_err(|e| e.to_string())?;
            lock.try_lock()
                .map_err(|e| format!("Memory card is already in use or cannot be locked: {e}"))?;
            let card = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => {
                    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                        return Err("Select a regular memory card file".into());
                    }
                    let mut bytes = Vec::new();
                    file.take(CARD_SIZE as u64 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    MemoryCard::from_bytes(bytes)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let card = MemoryCard::formatted();
                    atomic_write(&path, card.bytes(), true).map_err(|e| e.to_string())?;
                    card
                }
                Err(error) => return Err(error.to_string()),
            };
            Ok((
                Self {
                    path,
                    saved_revision: Some(card.revision()),
                    _lock: lock,
                },
                card,
            ))
        };
        open().map_err(|error| format!("Memory card {}: {error}", requested.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_card_with_same_revision_is_persisted() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-card-rewind-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("card.mcd");
        let (mut store, mut cards) = CardStore::open([Some(path.clone()), None]).unwrap();
        let mut bytes = cards[0].as_ref().unwrap().bytes().to_vec();
        bytes[1024] ^= 0xff;
        cards[0] = Some(MemoryCard::from_bytes(bytes.clone()).unwrap());
        store.invalidate();
        assert!(store.status(&cards)[0].as_ref().unwrap().pending);
        store.flush(&cards).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert!(!store.status(&cards)[0].as_ref().unwrap().pending);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

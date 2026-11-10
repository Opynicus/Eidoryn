//! Host-side card persistence. Serial devices never perform filesystem I/O.
use crate::memory_card::{CARD_SIZE, Cards, MemoryCard};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
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
    saved_revision: u64,
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
                    .is_some_and(|file| file.saved_revision != card.revision()),
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
                && file.saved_revision != card.revision()
            {
                atomic_write(&file.path, card.bytes(), false)
                    .map_err(|error| format!("Cannot save memory card {}: {error}. Data is still in memory; retry Flush saves.", file.path.display()))?;
                file.saved_revision = card.revision();
            }
        }
        self.last_flush = Instant::now();
        Ok(())
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
                    saved_revision: card.revision(),
                    _lock: lock,
                },
                card,
            ))
        };
        open().map_err(|error| format!("Memory card {}: {error}", requested.display()))
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(path.file_name().expect("validated card filename"));
    name.push(suffix);
    path.with_file_name(name)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn atomic_write(path: &Path, bytes: &[u8], create: bool) -> std::io::Result<()> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = sibling(path, &format!(".{}.{sequence}.tmp", std::process::id()));
    let mut file = File::create_new(&temp_path)?;
    let temp = Temporary(temp_path);
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if create {
        // Publishing a new card must never replace a file that appeared meanwhile.
        fs::hard_link(&temp.0, path)?;
    } else {
        fs::rename(&temp.0, path)?;
    }
    #[cfg(unix)]
    File::open(path.parent().expect("validated card parent"))?.sync_all()?;
    Ok(())
}

//! Desktop host for a GBA cartridge. All machine execution and file I/O run off the UI thread.
use crate::library::{Library, playtime::Playtime};
use eidoryn_gba::{BIOS_LEN, Machine};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests;

pub enum Command {
    Load {
        path: PathBuf,
        title: String,
        bios: PathBuf,
    },
    Buttons(u16),
    Toggle,
    Pause,
    Reset,
    PrepareClose(mpsc::Sender<Result<(), String>>),
    ClearStatistics(i64, mpsc::Sender<Result<(), String>>),
}

/// Reads a GBA firmware image for a selection or a cartridge launch.
pub fn read_bios(path: &Path) -> Result<Vec<u8>, String> {
    let describe = |e: std::io::Error| format!("{}: {e}", path.display());
    let metadata = fs::metadata(path).map_err(describe)?;
    if !metadata.is_file() {
        return Err(format!(
            "Select a regular GBA BIOS file: {}",
            path.display()
        ));
    }
    if metadata.len() != BIOS_LEN as u64 {
        return Err(format!(
            "GBA BIOS must be exactly 16 KiB ({BIOS_LEN} bytes): {} is {} bytes",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::with_capacity(BIOS_LEN);
    File::open(path)
        .map_err(describe)?
        .take(BIOS_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(describe)?;
    eidoryn_gba::validate_bios(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes)
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub path: Option<PathBuf>,
    pub title: String,
    pub running: bool,
    pub rgb: Arc<[u8]>,
    pub media_revision: u64,
    pub error: Option<String>,
}

pub struct Runner {
    tx: Option<mpsc::Sender<Command>>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Runner {
    pub fn start(store: Option<Library>, saves: PathBuf) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let output = Arc::clone(&latest);
        let tracker = store
            .map(Playtime::open)
            .transpose()
            .map_err(|e| e.to_string())?;
        let worker = thread::Builder::new()
            .name("eidoryn-gba".into())
            .spawn(move || Worker::new(saves, tracker).run(&rx, &output))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(tx),
            latest,
            worker: Some(worker),
        })
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or("GBA worker has stopped")?
            .send(command)
            .map_err(|e| e.to_string())
    }

    pub fn take_snapshot(&self) -> Option<Snapshot> {
        self.latest.lock().ok()?.take()
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Cartridge {
    machine: Machine,
    save: PathBuf,
    lock: Arc<File>,
    pending_save: bool,
}

impl Cartridge {
    fn load(
        path: &Path,
        bios: Vec<u8>,
        saves: &Path,
        current: Option<&Self>,
    ) -> Result<(Self, String), String> {
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gba"))
        {
            return Err(format!("Expected a .gba cartridge: {}", path.display()));
        }
        let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.is_file() || metadata.len() > eidoryn_gba::MAX_ROM_LEN as u64 {
            return Err(format!("Invalid cartridge size: {}", path.display()));
        }
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(eidoryn_gba::MAX_ROM_LEN as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let identity = format!("{:x}", Sha256::digest(&bytes));
        let mut machine = Machine::with_bios(bytes, bios).map_err(|e| e.to_string())?;
        fs::create_dir_all(saves).map_err(|e| format!("{}: {e}", saves.display()))?;
        let save = saves.join(format!("{identity}.sav"));
        let lock = if let Some(loaded) = current.filter(|c| c.save == save) {
            Arc::clone(&loaded.lock)
        } else {
            let lock = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(save.with_extension("lock"))
                .map_err(|e| e.to_string())?;
            lock.try_lock()
                .map_err(|e| format!("{}: {e}", save.display()))?;
            Arc::new(lock)
        };
        match fs::metadata(&save) {
            Ok(meta) if !meta.is_file() => {
                return Err(format!("Expected a regular save file: {}", save.display()));
            }
            Ok(_) => {
                let file = File::open(&save).map_err(|e| format!("{}: {e}", save.display()))?;
                let mut bytes = Vec::new();
                file.take(machine.sram().len() as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                machine
                    .restore_sram(&bytes)
                    .map_err(|e| format!("{}: {e}", save.display()))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", save.display())),
        }
        Ok((
            Self {
                machine,
                save,
                lock,
                pending_save: false,
            },
            identity,
        ))
    }

    fn flush(&mut self) -> Result<(), String> {
        self.pending_save |= self.machine.take_sram_dirty();
        if self.pending_save {
            let write = || -> std::io::Result<()> {
                let mut temp =
                    tempfile::NamedTempFile::new_in(self.save.parent().expect("save directory"))?;
                temp.write_all(self.machine.sram())?;
                temp.as_file().sync_all()?;
                temp.persist(&self.save)?;
                Ok(())
            };
            write().map_err(|e| format!("{}: {e}", self.save.display()))?;
            self.pending_save = false;
        }
        Ok(())
    }
}

struct Worker {
    saves: PathBuf,
    tracker: Option<Playtime>,
    cartridge: Option<Cartridge>,
    snapshot: Snapshot,
    last_save: Instant,
}

impl Worker {
    fn new(saves: PathBuf, tracker: Option<Playtime>) -> Self {
        Self {
            saves,
            tracker,
            cartridge: None,
            snapshot: Snapshot::default(),
            last_save: Instant::now(),
        }
    }

    fn flush(&mut self) -> Result<(), String> {
        if let Some(cart) = &mut self.cartridge {
            cart.flush()?;
        }
        self.last_save = Instant::now();
        Ok(())
    }

    fn pause(&mut self) {
        self.snapshot.running = false;
        if let Some(cart) = &mut self.cartridge {
            cart.machine.set_buttons(0);
        }
        if let Some(clock) = &mut self.tracker {
            clock.sample(false);
        }
    }

    fn apply(&mut self, command: Command) -> Result<bool, String> {
        match command {
            Command::Load { path, title, bios } => {
                let bios = match read_bios(&bios) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.snapshot.error = Some(error);
                        return Ok(true);
                    }
                };
                self.pause();
                self.flush()?;
                let (cart, identity) =
                    Cartridge::load(&path, bios, &self.saves, self.cartridge.as_ref())?;
                if let Some(clock) = &mut self.tracker {
                    clock.select(path.clone(), identity, false);
                }
                self.cartridge = Some(cart);
                self.snapshot = Snapshot {
                    path: Some(path),
                    title,
                    running: true,
                    media_revision: self.snapshot.media_revision.wrapping_add(1),
                    ..Snapshot::default()
                };
            }
            Command::Buttons(keys) => {
                if self.snapshot.running
                    && let Some(cart) = &mut self.cartridge
                {
                    cart.machine.set_buttons(keys);
                }
                return Ok(false);
            }
            Command::Toggle if !self.snapshot.running => {
                self.snapshot.running = self.cartridge.is_some();
            }
            Command::Pause | Command::Toggle => {
                self.pause();
                self.flush()?;
            }
            Command::Reset => {
                self.flush()?;
                if let Some(cart) = &mut self.cartridge {
                    cart.machine.reset();
                    self.snapshot.rgb = Arc::from(cart.machine.pixels());
                    self.snapshot.running = true;
                }
            }
            Command::PrepareClose(done) => {
                self.pause();
                let _ = done.send(self.flush());
            }
            Command::ClearStatistics(game, done) => {
                if let Some(clock) = &mut self.tracker {
                    clock.clear(game, done);
                } else {
                    let _ = done.send(Err("Game library is unavailable".into()));
                }
                return Ok(false);
            }
        }
        self.snapshot.error = None;
        Ok(true)
    }

    fn frame(&mut self) -> Result<(), String> {
        let Some(cart) = &mut self.cartridge else {
            return Ok(());
        };
        cart.machine.run_frame().map_err(|e| e.to_string())?;
        if cart.machine.pixels() != self.snapshot.rgb.as_ref() {
            self.snapshot.rgb = Arc::from(cart.machine.pixels());
        }
        if let Some(clock) = &mut self.tracker {
            clock.sample(true);
        }
        if self.last_save.elapsed() >= Duration::from_secs(1) {
            self.flush()?;
        }
        Ok(())
    }

    fn run(mut self, rx: &mpsc::Receiver<Command>, output: &Mutex<Option<Snapshot>>) {
        let field = Duration::from_secs_f64(1.0 / eidoryn_gba::FRAME_RATE);
        let mut next = Instant::now();
        loop {
            let result = if self.snapshot.running {
                // A zero timeout still checks queued input and shutdown when rendering is late.
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(command) => self.apply(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let result = self.frame().map(|()| true);
                        next = (next + field).max(Instant::now());
                        result
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                let Ok(command) = rx.recv() else {
                    break;
                };
                next = Instant::now();
                self.apply(command)
            };
            match result {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    self.pause();
                    self.snapshot.error = Some(error);
                }
            }
            let mut snapshot = self.snapshot.clone();
            if snapshot.error.is_none() {
                snapshot.error = self.tracker.as_ref().and_then(Playtime::error);
            }
            if let Ok(mut slot) = output.lock() {
                *slot = Some(snapshot);
            }
        }
        self.pause();
        if let Err(error) = self.flush() {
            eprintln!("Eidoryn: {error}");
        }
    }
}

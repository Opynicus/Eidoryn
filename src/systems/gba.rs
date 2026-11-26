//! Desktop host for a GBA cartridge. All machine execution and file I/O run off the UI thread.
use super::checkpoints::{DEFAULT_REWIND_SECONDS, QuickSlot, SaveStateInfo, StateDirectory};
use crate::library::{Library, playtime::Playtime};
use eidoryn_gba::Machine;
use eidoryn_host::audio::AudioBuffer;
use std::fmt::Write as _;
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

mod battery;
mod checkpoints;
mod rewind;
#[cfg(test)]
mod tests;

pub enum Command {
    Load { path: PathBuf, title: String },
    Buttons(u16),
    SetHardware { enabled: bool, request: u64 },
    Toggle,
    Pause,
    SetDebug(bool),
    SaveState,
    QuickSave,
    QuickSaveIfUnchanged { revision: u64, slot: QuickSlot },
    QuickLoad,
    LoadState { path: PathBuf },
    RenameState { path: PathBuf, name: String },
    DeleteState { path: PathBuf, modified: SystemTime },
    RefreshSaveStates,
    SetRewindEnabled(bool),
    SetRewindSeconds(u16),
    SetRewinding(bool),
    SyncBattery,
    ImportBattery { path: PathBuf, media_revision: u64 },
    ExportBattery { path: PathBuf, media_revision: u64 },
    Step,
    Slice,
    Reset,
    PrepareClose(mpsc::Sender<Result<(), String>>),
    ClearStatistics(i64, mpsc::Sender<Result<(), String>>),
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub path: Option<PathBuf>,
    pub title: String,
    pub running: bool,
    pub save_states: Arc<[SaveStateInfo]>,
    pub quick_slot: QuickSlot,
    pub quick_slot_revision: u64,
    pub renamed_state: Option<(PathBuf, PathBuf)>,
    pub rewind_enabled: bool,
    pub rewind_max_seconds: u16,
    pub rewinding: bool,
    pub rewind_states: usize,
    pub rewind_seconds: f64,
    pub battery_path: Option<PathBuf>,
    pub success_message: Option<String>,
    pub rgb: Arc<[u8]>,
    pub video_fps: f64,
    pub graphics_device: Option<String>,
    pub graphics_error: Option<String>,
    pub graphics_request: u64,
    pub media_revision: u64,
    pub error: Option<String>,
    pub debug: Option<Arc<eidoryn_gba::debug::DebugSnapshot>>,
}

pub struct Runner {
    tx: Option<mpsc::Sender<Command>>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Runner {
    pub fn start(store: Option<Library>, data_dir: PathBuf) -> Result<Self, String> {
        Self::start_with_audio(store, data_dir, None)
    }

    /// Start a cartridge worker with an optional PCM queue at the core's sample rate.
    pub fn start_with_audio(
        store: Option<Library>,
        data_dir: PathBuf,
        audio: Option<AudioBuffer>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let output = Arc::clone(&latest);
        let tracker = store
            .map(Playtime::open)
            .transpose()
            .map_err(|e| e.to_string())?;
        let worker = thread::Builder::new()
            .name("eidoryn-gba".into())
            .spawn(move || Worker::new(&data_dir, tracker, audio).run(&rx, &output))
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
    saved_rtc: Option<[u8; eidoryn_gba::RTC_BATTERY_LEN]>,
}

impl Cartridge {
    fn load(path: &Path, saves: &Path, current: Option<&Self>) -> Result<(Self, String), String> {
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
        let mut machine = Machine::from_rom(bytes).map_err(|e| e.to_string())?;
        let mut identity = String::with_capacity(64);
        for byte in machine.rom_digest() {
            write!(identity, "{byte:02x}").expect("String formatting is infallible");
        }
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
        let saved_rtc = battery::load_rtc(&mut machine, &save)?;
        battery::load_existing(&mut machine, &save)?;
        Ok((
            Self {
                machine,
                save,
                lock,
                pending_save: false,
                saved_rtc,
            },
            identity,
        ))
    }
}

struct FrameRate {
    start: Instant,
    frames: u32,
}

impl FrameRate {
    fn new(start: Instant) -> Self {
        Self { start, frames: 0 }
    }

    fn completed_frame(&mut self, now: Instant) -> Option<f64> {
        self.frames += 1;
        let elapsed = now.duration_since(self.start);
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        let fps = f64::from(self.frames) / elapsed.as_secs_f64();
        *self = Self::new(now);
        Some(fps)
    }
}

struct Worker {
    audio: Option<AudioBuffer>,
    saves: PathBuf,
    states: Option<StateDirectory>,
    rewind: rewind::Rewind,
    held: u16,
    tracker: Option<Playtime>,
    cartridge: Option<Cartridge>,
    hardware: bool,
    snapshot: Snapshot,
    last_save: Instant,
    frame_rate: Option<FrameRate>,
    capture_debug: bool,
}

impl Worker {
    fn new(data_dir: &Path, tracker: Option<Playtime>, audio: Option<AudioBuffer>) -> Self {
        Self {
            audio,
            saves: data_dir.join("saves/gba"),
            states: None,
            rewind: rewind::Rewind::default(),
            held: 0,
            tracker,
            cartridge: None,
            hardware: false,
            snapshot: Snapshot {
                rewind_max_seconds: DEFAULT_REWIND_SECONDS,
                ..Snapshot::default()
            },
            last_save: Instant::now(),
            frame_rate: None,
            capture_debug: false,
        }
    }

    fn flush(&mut self) -> Result<(), String> {
        if let Some(cart) = &mut self.cartridge {
            cart.flush()?;
        }
        self.last_save = Instant::now();
        Ok(())
    }

    fn reset_frame_rate(&mut self) {
        self.frame_rate = None;
        self.snapshot.video_fps = 0.0;
    }

    fn pause(&mut self) {
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        self.end_rewind();
        self.held = 0;
        self.reset_frame_rate();
        self.snapshot.running = false;
        if let Some(cart) = &mut self.cartridge {
            cart.machine.set_buttons(0);
        }
        if let Some(clock) = &mut self.tracker {
            clock.sample(false);
        }
    }

    fn configure_graphics(&mut self) {
        self.reset_frame_rate();
        if let Some(cart) = &mut self.cartridge {
            self.snapshot.graphics_error =
                cart.machine.set_hardware_acceleration(self.hardware).err();
            self.snapshot.graphics_device = cart.machine.graphics_device().map(str::to_owned);
        }
    }

    fn load_cartridge(&mut self, path: PathBuf, title: String) -> Result<(), String> {
        self.pause();
        self.flush()?;
        let (cart, identity) = Cartridge::load(&path, &self.saves, self.cartridge.as_ref())?;
        self.states = Some(StateDirectory::new(
            self.saves.join("states").join(&identity),
            StateDirectory::file_prefix(&title),
            "gbastate",
        ));
        if let Some(clock) = &mut self.tracker {
            clock.select(path.clone(), identity, false);
        }
        let battery_path = Some(cart.save.clone());
        self.cartridge = Some(cart);
        self.snapshot = Snapshot {
            path: Some(path),
            title,
            running: true,
            battery_path,
            rewind_enabled: self.snapshot.rewind_enabled,
            rewind_max_seconds: self.snapshot.rewind_max_seconds,
            quick_slot_revision: self.snapshot.quick_slot_revision.wrapping_add(1),
            media_revision: self.snapshot.media_revision.wrapping_add(1),
            graphics_request: self.snapshot.graphics_request,
            ..Snapshot::default()
        };
        self.configure_graphics();
        self.reset_rewind()?;
        Ok(())
    }

    fn set_hardware(&mut self, enabled: bool, request: u64) {
        let previous = std::mem::replace(&mut self.hardware, enabled);
        self.configure_graphics();
        if self.snapshot.graphics_error.is_some() {
            self.hardware = previous;
        }
        self.snapshot.graphics_request = request;
    }

    fn step_debugger(&mut self, frame: bool) -> Result<(), String> {
        self.pause();
        if let Some(cart) = &mut self.cartridge {
            if frame {
                cart.machine.run_frame()
            } else {
                cart.machine.step()
            }
            .map_err(|error| error.to_string())?;
            drop(cart.machine.take_audio_samples());
            self.snapshot.rgb = Arc::from(cart.machine.pixels());
            if frame {
                self.completed_rewind_frame()?;
            }
        }
        self.flush()
    }

    fn reset_machine(&mut self) -> Result<(), String> {
        self.pause();
        self.flush()?;
        if let Some(cart) = &mut self.cartridge {
            cart.machine.reset();
            self.snapshot.rgb = Arc::from(cart.machine.pixels());
            self.snapshot.running = !self.capture_debug;
            self.reset_rewind()?;
        }
        Ok(())
    }

    fn apply(&mut self, command: Command) -> Result<bool, String> {
        if !matches!(
            command,
            Command::SetRewinding(_)
                | Command::Buttons(_)
                | Command::SetDebug(_)
                | Command::RefreshSaveStates
                | Command::ClearStatistics(..)
        ) {
            self.end_rewind();
        }
        if !matches!(
            command,
            Command::Buttons(_) | Command::SetDebug(_) | Command::RefreshSaveStates
        ) {
            self.snapshot.success_message = None;
        }
        let operation = match command {
            Command::Load { path, title } => {
                self.load_cartridge(path, title)?;
                self.refresh_states()
            }
            Command::SetHardware { enabled, request } => {
                self.set_hardware(enabled, request);
                return Ok(true);
            }
            Command::Buttons(keys) => {
                if self.snapshot.running || self.snapshot.rewinding {
                    self.held = keys;
                }
                if self.snapshot.running
                    && let Some(cart) = &mut self.cartridge
                {
                    cart.machine.set_buttons(keys);
                }
                return Ok(false);
            }
            Command::Toggle if !self.snapshot.running => {
                self.snapshot.running = self.cartridge.is_some();
                if let Some(cart) = &mut self.cartridge {
                    cart.machine.set_buttons(self.held);
                }
                Ok(())
            }
            Command::Pause | Command::Toggle => {
                self.pause();
                self.flush()?;
                Ok(())
            }
            Command::SetDebug(enabled) => {
                self.capture_debug = enabled;
                return Ok(true);
            }
            Command::Step | Command::Slice => self.step_debugger(matches!(command, Command::Slice)),
            Command::Reset => self.reset_machine(),
            Command::PrepareClose(done) => {
                self.pause();
                let _ = done.send(self.flush());
                Ok(())
            }
            Command::ClearStatistics(game, done) => {
                if let Some(clock) = &mut self.tracker {
                    clock.clear(game, done);
                } else {
                    let _ = done.send(Err("Game library is unavailable".into()));
                }
                return Ok(false);
            }
            Command::SetRewindEnabled(enabled) => self.set_rewind_enabled(enabled),
            Command::SetRewindSeconds(seconds) => self.set_rewind_seconds(seconds),
            Command::SetRewinding(true) => self.begin_rewind(),
            Command::SetRewinding(false) => {
                self.end_rewind();
                Ok(())
            }
            Command::SaveState => self.save_state(false),
            Command::QuickSave => self.save_state(true),
            Command::QuickSaveIfUnchanged { revision, slot } => {
                self.quick_save_if_unchanged(revision, slot)
            }
            Command::QuickLoad => self.quick_load(),
            Command::LoadState { path } => self.load_state(&path),
            Command::RenameState { path, name } => self.rename_state(path, &name),
            Command::DeleteState { path, modified } => self.delete_state(&path, modified),
            Command::RefreshSaveStates if self.states.is_none() => Ok(()),
            Command::RefreshSaveStates => self.refresh_states(),
            Command::SyncBattery => self.sync_battery(),
            Command::ImportBattery { media_revision, .. }
            | Command::ExportBattery { media_revision, .. }
                if self.snapshot.media_revision != media_revision =>
            {
                Err("The cartridge changed while selecting a battery save".into())
            }
            Command::ImportBattery { path, .. } => self.import_battery(&path),
            Command::ExportBattery { path, .. } => self.export_battery(&path),
        };
        self.snapshot.error = operation.err();
        if self.snapshot.error.is_some() {
            self.snapshot.success_message = None;
        }
        Ok(true)
    }

    fn frame(&mut self) -> Result<(), String> {
        let Some(cart) = &mut self.cartridge else {
            return Ok(());
        };
        let meter = self
            .frame_rate
            .get_or_insert_with(|| FrameRate::new(Instant::now()));
        cart.machine.run_frame().map_err(|e| e.to_string())?;
        let samples = cart.machine.take_audio_samples();
        if let Some(audio) = &self.audio {
            audio.set_playing(true);
            audio.push(&samples);
        }
        // Even identical framebuffers count. Debug stepping and worker notifications do not.
        if let Some(fps) = meter.completed_frame(Instant::now()) {
            self.snapshot.video_fps = fps;
        }
        if cart.machine.pixels() != self.snapshot.rgb.as_ref() {
            self.snapshot.rgb = Arc::from(cart.machine.pixels());
        }
        if let Some(clock) = &mut self.tracker {
            clock.sample(true);
        }
        self.completed_rewind_frame()?;
        if self.last_save.elapsed() >= Duration::from_secs(1) {
            self.flush()?;
        }
        Ok(())
    }

    fn run(mut self, rx: &mpsc::Receiver<Command>, output: &Mutex<Option<Snapshot>>) {
        let field = Duration::from_secs_f64(1.0 / eidoryn_gba::FRAME_RATE);
        let mut next = Instant::now();
        loop {
            let result = if self.snapshot.rewinding {
                let wait = self.rewind.wait_duration();
                match rx.recv_timeout(wait) {
                    Ok(command) => self.apply(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => self.rewind_one(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else if self.snapshot.running {
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
            if let Some(error) = self
                .cartridge
                .as_mut()
                .and_then(|cart| cart.machine.take_graphics_error())
            {
                self.snapshot.graphics_error = Some(error);
                self.snapshot.graphics_device = None;
            }
            let mut snapshot = self.snapshot.clone();
            if self.capture_debug
                && let Some(cart) = &self.cartridge
            {
                snapshot.debug = Some(Arc::new(cart.machine.debug_snapshot()));
            }
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

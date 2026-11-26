//! Nintendo DS desktop worker. Emulation and file I/O stay off the UI thread.
use super::{
    checkpoints::{QuickSlot, StateDirectory},
    persistence::atomic_write,
};
use crate::library::{Library, playtime::Playtime};
use eidoryn_host::audio::AudioBuffer;
use eidoryn_nds::Machine;
use std::{
    fmt::Write as _,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

/// Commands for the active Nintendo DS cartridge.
pub enum Command {
    Load { path: PathBuf, title: String },
    Buttons(u16),
    Touch(Option<(u8, u8)>),
    Toggle,
    Pause,
    Reset,
    Step,
    Frame,
    QuickLoad,
    QuickSave,
    QuickSaveIfUnchanged { revision: u64, slot: QuickSlot },
    PrepareClose(mpsc::Sender<Result<(), String>>),
    ClearStatistics(i64, mpsc::Sender<Result<(), String>>),
}

/// The latest completed frame uses RGB24 and contains both physical LCDs, top first.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub path: Option<PathBuf>,
    pub title: String,
    pub rgb: Arc<[u8]>,
    pub running: bool,
    pub arm9_pc: u32,
    pub arm7_pc: u32,
    pub video_fps: f64,
    pub error: Option<String>,
    pub media_revision: u64,
    pub quick_slot_revision: u64,
    pub quick_slot: QuickSlot,
}

pub struct Runner {
    input: Option<mpsc::Sender<Command>>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Runner {
    pub fn start(library: Option<Library>, data_dir: PathBuf) -> Result<Self, String> {
        Self::start_with_audio(library, data_dir, None)
    }

    /// Start a cartridge worker with an optional PCM queue at the core's sample rate.
    pub fn start_with_audio(
        library: Option<Library>,
        data_dir: PathBuf,
        audio: Option<AudioBuffer>,
    ) -> Result<Self, String> {
        let clock = library
            .map(Playtime::open)
            .transpose()
            .map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let output = Arc::clone(&latest);
        let worker = thread::Builder::new()
            .name("eidoryn-nds".into())
            .spawn(move || Worker::new(&data_dir, clock, audio).run(&rx, &output))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            input: Some(tx),
            latest,
            worker: Some(worker),
        })
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("Nintendo DS worker stopped")?
            .send(command)
            .map_err(|e| e.to_string())
    }
    pub fn take_snapshot(&self) -> Option<Snapshot> {
        self.latest.lock().ok()?.take()
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        drop(self.input.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_file(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    read_bounded(path, max).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_bounded(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > max as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Unexpected file size or type",
        ));
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(max as u64 + 1).read_to_end(&mut data)?;
    if data.len() > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File exceeds size limit",
        ));
    }
    Ok(data)
}

struct Cartridge {
    machine: Machine,
    states: StateDirectory,
    battery: PathBuf,
    saved_revision: u64,
}

impl Cartridge {
    fn load_battery(&mut self) -> Result<(), String> {
        match read_bounded(&self.battery, eidoryn_nds::MAX_BACKUP_BYTES) {
            Ok(bytes) => self
                .machine
                .load_backup(&bytes)
                .map_err(|e| format!("{}: {e}", self.battery.display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", self.battery.display())),
        }
        self.saved_revision = self.machine.backup_revision();
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let revision = self.machine.backup_revision();
        if revision != self.saved_revision
            && let Some(bytes) = self.machine.backup()
        {
            let write = || -> io::Result<()> {
                fs::create_dir_all(
                    self.battery
                        .parent()
                        .expect("managed battery file has a parent"),
                )?;
                atomic_write(&self.battery, bytes, false)
            };
            write().map_err(|e| format!("{}: {e}", self.battery.display()))?;
        }
        self.saved_revision = revision;
        Ok(())
    }
}

struct Worker {
    audio: Option<AudioBuffer>,
    saves: PathBuf,
    cart: Option<Cartridge>,
    clock: Option<Playtime>,
    state: Snapshot,
    keys: u16,
    touch: Option<(u8, u8)>,
    rate_since: Instant,
    frames: u32,
}

impl Worker {
    fn new(data_dir: &Path, clock: Option<Playtime>, audio: Option<AudioBuffer>) -> Self {
        Self {
            audio,
            saves: data_dir.join("saves/nds"),
            cart: None,
            clock,
            state: Snapshot::default(),
            keys: 0,
            touch: None,
            rate_since: Instant::now(),
            frames: 0,
        }
    }
    fn pause(&mut self) {
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        self.keys = 0;
        self.touch = None;
        if let Some(cart) = &mut self.cart {
            cart.machine.set_buttons(0);
            cart.machine.set_touch(None);
        }
        if let Some(clock) = &mut self.clock {
            clock.sample(false);
        }
        self.state.running = false;
        self.reset_rate();
    }
    fn reset_rate(&mut self) {
        self.rate_since = Instant::now();
        self.frames = 0;
        self.state.video_fps = 0.0;
    }

    fn load(&mut self, path: PathBuf, title: String) -> Result<(), String> {
        self.pause();
        self.flush()?;
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("nds"))
        {
            return Err(format!("Expected an .nds cartridge: {}", path.display()));
        }
        let machine = Machine::from_rom(read_file(&path, eidoryn_nds::MAX_ROM_LEN)?)
            .map_err(|e| e.to_string())?;
        let mut identity = String::with_capacity(64);
        for byte in machine.rom_digest() {
            write!(identity, "{byte:02x}").expect("formatting into a String");
        }
        let states = StateDirectory::new(
            self.saves.join("states").join(&identity),
            StateDirectory::file_prefix(&title),
            "ndsstate",
        );
        let slot = states
            .quick_slot_modified()?
            .map_or(QuickSlot::Empty, QuickSlot::Saved);
        let mut cart = Cartridge {
            machine,
            states,
            battery: self.saves.join(format!("{identity}.sav")),
            saved_revision: 0,
        };
        cart.load_battery()?;
        if let Some(clock) = &mut self.clock {
            clock.select(path.clone(), identity, false);
        }
        self.state = Snapshot {
            path: Some(path),
            title,
            running: true,
            media_revision: self.state.media_revision.wrapping_add(1),
            quick_slot_revision: self.state.quick_slot_revision.wrapping_add(1),
            quick_slot: slot,
            ..Snapshot::default()
        };
        self.cart = Some(cart);
        Ok(())
    }

    fn cart(&mut self) -> Result<&mut Cartridge, String> {
        self.cart
            .as_mut()
            .ok_or_else(|| "Load a Nintendo DS cartridge first".into())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.cart.as_mut().map_or(Ok(()), Cartridge::flush)
    }

    fn checkpoint(&mut self, expected: Option<(u64, QuickSlot)>) -> Result<(), String> {
        let revision = self.state.quick_slot_revision;
        let cart = self.cart()?;
        let slot = cart
            .states
            .quick_slot_modified()?
            .map_or(QuickSlot::Empty, QuickSlot::Saved);
        if expected.is_some_and(|old| old != (revision, slot)) {
            self.state.quick_slot = slot;
            return Err("Quick slot changed. Check the current slot and try again.".into());
        }
        let data = cart.machine.save_state().map_err(|e| e.to_string())?;
        cart.states.save_quick(&data)?;
        self.state.quick_slot = cart
            .states
            .quick_slot_modified()?
            .map_or(QuickSlot::Empty, QuickSlot::Saved);
        self.state.quick_slot_revision = revision.wrapping_add(1);
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        let (keys, touch) = (self.keys, self.touch);
        let cart = self.cart()?;
        let data = read_file(&cart.states.quick_path(), eidoryn_nds::MAX_STATE_BYTES)?;
        cart.machine.load_state(&data).map_err(|e| e.to_string())?;
        cart.machine.set_buttons(keys);
        cart.machine.set_touch(touch);
        if let Some(audio) = &self.audio {
            audio.clear();
        }
        self.reset_rate();
        self.flush()
    }
    fn apply(&mut self, command: Command) -> Result<bool, String> {
        match command {
            Command::Load { path, title } => self.load(path, title)?,
            Command::Pause | Command::Toggle if self.state.running => {
                self.pause();
                self.flush()?;
            }
            Command::Pause => self.flush()?,
            Command::Toggle => {
                self.state.running = self.cart.is_some();
                self.reset_rate();
            }
            Command::Buttons(keys) => {
                if self.state.running {
                    self.keys = keys;
                    self.cart()?.machine.set_buttons(keys);
                }
                return Ok(false);
            }
            Command::Touch(touch) => {
                if self.state.running {
                    self.touch = touch;
                    self.cart()?.machine.set_touch(touch);
                }
                return Ok(false);
            }
            Command::Step | Command::Frame => {
                self.pause();
                let machine = &mut self.cart()?.machine;
                if matches!(command, Command::Step) {
                    machine.step()
                } else {
                    machine.run_frame()
                }
                .map_err(|e| e.to_string())?;
                drop(machine.take_audio_samples());
                self.flush()?;
            }
            Command::Reset => {
                self.pause();
                self.flush()?;
                self.cart()?.machine.reset();
                self.state.running = true;
            }
            Command::QuickSave => self.checkpoint(None)?,
            Command::QuickSaveIfUnchanged { revision, slot } => {
                self.checkpoint(Some((revision, slot)))?;
            }
            Command::QuickLoad => self.restore()?,
            Command::PrepareClose(done) => {
                self.pause();
                let _ = done.send(self.flush());
            }
            Command::ClearStatistics(game, reply) => {
                if let Some(clock) = &mut self.clock {
                    clock.clear(game, reply);
                } else {
                    let _ = reply.send(Err("Game library unavailable".into()));
                }
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn frame(&mut self) -> Result<(), String> {
        let machine = &mut self.cart()?.machine;
        machine.run_frame().map_err(|e| e.to_string())?;
        let samples = machine.take_audio_samples();
        if let Some(audio) = &self.audio {
            audio.set_playing(true);
            audio.push(&samples);
        }
        if let Some(clock) = &mut self.clock {
            clock.sample(true);
        }
        self.frames += 1;
        let elapsed = self.rate_since.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.flush()?;
            self.state.video_fps = f64::from(self.frames) / elapsed.as_secs_f64();
            self.frames = 0;
            self.rate_since = Instant::now();
        }
        Ok(())
    }
    fn publish(&mut self, latest: &Mutex<Option<Snapshot>>) {
        if let Some(cart) = &self.cart {
            let pixels = cart.machine.pixels();
            if pixels != self.state.rgb.as_ref() {
                self.state.rgb = Arc::from(pixels);
            }
            (self.state.arm9_pc, self.state.arm7_pc) = cart.machine.pcs();
        }
        let mut state = self.state.clone();
        if state.error.is_none() {
            state.error = self.clock.as_ref().and_then(Playtime::error);
        }
        *latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
    }
    fn run(mut self, rx: &mpsc::Receiver<Command>, latest: &Mutex<Option<Snapshot>>) {
        let interval = Duration::from_secs_f64(1.0 / eidoryn_nds::FRAME_RATE);
        let mut next = Instant::now();
        loop {
            let cmd = if self.state.running {
                match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(cmd) => Some(cmd),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                let Ok(cmd) = rx.recv() else {
                    break;
                };
                next = Instant::now();
                Some(cmd)
            };
            let mut publish = false;
            if let Some(cmd) = cmd {
                match self.apply(cmd) {
                    Ok(changed) => {
                        publish = changed;
                        if changed {
                            self.state.error = None;
                        }
                    }
                    Err(e) => {
                        self.state.error = Some(e);
                        publish = true;
                    }
                }
            }
            // Service the frame deadline even during sustained key and stylus motion.
            if self.state.running && Instant::now() >= next {
                if let Err(error) = self.frame() {
                    self.pause();
                    self.state.error = Some(error);
                }
                next = (next + interval).max(Instant::now());
                publish = true;
            }
            if publish {
                self.publish(latest);
            }
        }
        self.pause();
        if let Err(error) = self.flush() {
            eprintln!("Eidoryn: {error}");
        }
    }
}

#[cfg(test)]
mod tests;

//! One emulation thread owns the machine; clients send commands and consume snapshots.
mod pacing;
mod snapshot;
mod worker;

use crate::systems::ps1::card_file::CardStore;
use crate::systems::ps1::machine::Machine;
use crate::systems::ps1::state_file::StateStore;
use eidoryn_host::audio::AudioBuffer;
pub use snapshot::{ExecutionState, QuickSlot, SelectedMedia, Snapshot, SnapshotMode};
// Preserve the existing loader entry points while keeping file I/O outside the worker.
pub use crate::systems::ps1::media::{load_disc, load_path};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::SystemTime,
};

type SharedSnapshot = Arc<Mutex<Option<Snapshot>>>;

impl QuickSlot {
    fn inspect(store: &StateStore, message: &mut Option<String>) -> Self {
        match store.quick_slot_modified() {
            Ok(Some(modified)) => Self::Saved(modified),
            Ok(None) => Self::Empty,
            Err(error) => {
                message.get_or_insert(error);
                Self::Unavailable
            }
        }
    }
}

pub const MIN_REWIND_SECONDS: u16 = 5;
pub const MAX_REWIND_SECONDS: u16 = 120;
pub const DEFAULT_REWIND_SECONDS: u16 = 30;

pub enum Command {
    SetSnapshotMode(SnapshotMode),
    SetButtons(u16),
    SetTurbo(super::input::TurboConfig),
    SetRenderer(crate::systems::ps1::gpu::RendererMode),
    /// Change hardware backend and its internal raster resolution in one request.
    SetRendererWithScale {
        mode: crate::systems::ps1::gpu::RendererMode,
        scale: crate::systems::ps1::gpu::RenderScale,
    },
    /// Atomically change the backend, internal resolution and presentation color depth.
    SetRendererConfig {
        mode: super::gpu::RendererMode,
        scale: super::gpu::RenderScale,
        color_depth: super::gpu::ColorDepth,
    },
    /// Select the hardware filter for subsequently produced presentation frames.
    SetAntiAliasing(super::gpu::AntiAliasing),
    SetCpuMode(crate::systems::ps1::cpu_engine::CpuMode),
    SetRewindEnabled(bool),
    SetRewindSeconds(u16),
    SetRewinding(bool),
    Toggle,
    Step,
    Slice,
    Reset,
    Demo,
    FlushCards,
    SaveState,
    QuickSave,
    /// Save only to the session and slot shown when the user made the decision.
    QuickSaveIfUnchanged {
        revision: u64,
        slot: QuickSlot,
    },
    QuickLoad,
    LoadState {
        path: PathBuf,
    },
    RenameState {
        path: PathBuf,
        name: String,
    },
    DeleteState {
        path: PathBuf,
        modified: SystemTime,
    },
    RefreshSaveStates,
    Load {
        path: PathBuf,
        bios: bool,
    },
    LoadDisc {
        path: PathBuf,
        bios: Option<PathBuf>,
        fast_boot: bool,
    },
    /// Reopen a history entry and identify it in the successful media receipt.
    ReopenDisc {
        path: PathBuf,
        bios: Option<PathBuf>,
        fast_boot: bool,
    },
    RequestClose,
    Shutdown,
}

pub struct Runner {
    sender: mpsc::Sender<Command>,
    latest: SharedSnapshot,
    thread: Option<thread::JoinHandle<()>>,
}

impl Runner {
    pub fn new(machine: Machine, auto_run: bool) -> std::io::Result<Self> {
        Self::with_card_store(machine, auto_run, CardStore::default())
    }

    pub fn with_card_store(
        machine: Machine,
        auto_run: bool,
        store: CardStore,
    ) -> std::io::Result<Self> {
        Self::with_outputs(machine, auto_run, store, None)
    }

    pub fn with_outputs(
        machine: Machine,
        auto_run: bool,
        store: CardStore,
        audio: Option<AudioBuffer>,
    ) -> std::io::Result<Self> {
        Self::with_snapshot_mode(machine, auto_run, store, audio, SnapshotMode::Debug)
    }

    /// Select initial snapshot detail without constructing unused disassembly.
    /// Existing constructors retain full snapshots for diagnostic clients.
    pub fn with_snapshot_mode(
        machine: Machine,
        auto_run: bool,
        store: CardStore,
        audio: Option<AudioBuffer>,
        mode: SnapshotMode,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let mut unlatched_preview = None;
        let mut snapshot = Snapshot::capture_with_preview(
            &machine,
            auto_run,
            None,
            0.0,
            mode,
            &mut unlatched_preview,
        );
        snapshot.cards = store.status(&machine.bus.sio.cards);
        let states = StateStore::new(Path::new("saves"), &machine);
        snapshot.quick_slot = QuickSlot::inspect(&states, &mut snapshot.message);
        match states.list() {
            Ok(states) => snapshot.save_states = states,
            Err(error) => {
                snapshot.message.get_or_insert(error);
            }
        }
        let latest = Arc::new(Mutex::new(Some(snapshot)));
        let output = Arc::clone(&latest);
        let thread = thread::Builder::new()
            .name("eidoryn-emulation".into())
            .spawn(move || {
                worker::run(
                    machine,
                    auto_run,
                    &receiver,
                    &output,
                    store,
                    audio,
                    (mode, unlatched_preview),
                );
            })?;
        Ok(Self {
            sender,
            latest,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.sender
            .send(command)
            .map_err(|_| "Emulator worker stopped".into())
    }

    pub fn take_snapshot(&self) -> Option<Snapshot> {
        self.latest.lock().ok()?.take()
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

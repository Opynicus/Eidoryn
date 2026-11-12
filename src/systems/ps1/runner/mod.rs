//! One emulation thread owns the machine; clients send commands and consume snapshots.
mod snapshot;
mod worker;

use crate::systems::ps1::card_file::CardStore;
use crate::systems::ps1::machine::Machine;
use eidoryn_host::audio::AudioBuffer;
pub use snapshot::{ExecutionState, Snapshot, SnapshotMode};
// Preserve the existing loader entry points while keeping file I/O outside the worker.
pub use crate::systems::ps1::media::{load_disc, load_path};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
};

type SharedSnapshot = Arc<Mutex<Option<Snapshot>>>;

pub enum Command {
    SetSnapshotMode(SnapshotMode),
    SetButtons(u16),
    SetTurbo(super::input::TurboConfig),
    SetRenderer(crate::systems::ps1::gpu::RendererMode),
    SetCpuMode(crate::systems::ps1::cpu_engine::CpuMode),
    Toggle,
    Step,
    Slice,
    Reset,
    Demo,
    FlushCards,
    Load {
        path: PathBuf,
        bios: bool,
    },
    LoadDisc {
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
        let mut snapshot = Snapshot::capture_with_mode(&machine, auto_run, None, 0.0, mode);
        snapshot.cards = store.status(&machine.bus.sio.cards);
        let latest = Arc::new(Mutex::new(Some(snapshot)));
        let output = Arc::clone(&latest);
        let thread = thread::Builder::new()
            .name("eidoryn-emulation".into())
            .spawn(move || {
                worker::run(machine, auto_run, &receiver, &output, store, audio, mode);
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

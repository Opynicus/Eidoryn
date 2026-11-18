//! Immutable state published to frontends and diagnostic clients.
use crate::systems::ps1::{
    card_file::CardStatus,
    cpu::{Cpu, disassemble},
    cpu_engine::{CpuMode, CpuStats},
    gpu::RendererMode,
    machine::Machine,
    state_file::SaveStateInfo,
};
use std::{path::PathBuf, sync::Arc, time::SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionState {
    Running,
    Paused,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickSlot {
    Empty,
    Saved(SystemTime),
    Unavailable,
}

/// Display clients do not need formatted CPU instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotMode {
    Display,
    Debug,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedMedia {
    Bios,
    Exe,
    Disc,
    Demo,
}

#[allow(clippy::struct_excessive_bools)] // Independent media, execution, and shutdown flags.
pub struct Snapshot {
    pub save_states: Vec<SaveStateInfo>,
    pub quick_slot: QuickSlot,
    /// Increments after a session replacement or a successful quick save.
    pub quick_slot_revision: u64,
    pub renamed_state: Option<(PathBuf, PathBuf)>,
    pub mode: SnapshotMode,
    pub cards: [Option<CardStatus>; 2],
    pub pad_buttons: u16,
    pub pad_polls: u64,
    /// Last successfully selected BIOS; failed loads do not replace it.
    pub bios_path: Option<PathBuf>,
    pub disc_path: Option<PathBuf>,
    pub disc_fast_boot: bool,
    pub media_selection: Option<SelectedMedia>,
    pub media_revision: u64,
    pub cdrom_reads: u64,
    pub disc_mounted: bool,
    pub cpu: Cpu,
    pub cpu_mode: CpuMode,
    pub cpu_stats: CpuStats,
    pub title: String,
    pub is_demo: bool,
    pub running: bool,
    pub rewind_enabled: bool,
    pub rewind_max_seconds: u16,
    pub rewinding: bool,
    pub rewind_states: usize,
    pub rewind_seconds: f64,
    /// A machine/renderer fault, independent of recoverable notifications.
    pub halted: bool,
    /// Recoverable errors and machine faults; successful actions have their own channel.
    pub message: Option<String>,
    pub success_message: Option<String>,
    pub close_ready: bool,
    pub gpu_status: u32,
    pub renderer: RendererMode,
    /// Completed renderer-selection requests, including rejected requests.
    pub renderer_revision: u64,
    pub renderer_device: String,
    pub gpu_commands: u64,
    pub slices: u64,
    pub rgb: Arc<[u8]>,
    pub dimensions: (usize, usize),
    pub disassembly: Vec<(u32, Option<u32>, String)>,
    pub slices_per_second: f64,
    pub video_frames: u64,
    pub video_fps: f64,
    pub target_fps: f64,
}

impl Snapshot {
    pub fn execution_state(&self) -> ExecutionState {
        if self.halted || self.close_ready {
            ExecutionState::Stopped
        } else if self.running {
            ExecutionState::Running
        } else {
            ExecutionState::Paused
        }
    }
    /// Equal pixels and dimensions can share the frontend's existing GPU texture.
    pub fn has_same_frame(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions
            && (Arc::ptr_eq(&self.rgb, &other.rgb) || self.rgb == other.rgb)
    }

    pub fn capture(machine: &Machine, running: bool, message: Option<String>, rate: f64) -> Self {
        Self::capture_with_mode(machine, running, message, rate, SnapshotMode::Debug)
    }

    pub fn capture_with_mode(
        machine: &Machine,
        running: bool,
        message: Option<String>,
        rate: f64,
        mode: SnapshotMode,
    ) -> Self {
        let gpu = &machine.bus.gpu;
        let (dimensions, rgb) = gpu.completed_frame().map_or_else(
            || (gpu.dimensions(), gpu.display_rgb().into()),
            |frame| (frame.dimensions, Arc::clone(&frame.rgb)),
        );
        let disassembly = (0..if mode == SnapshotMode::Debug { 9 } else { 0 })
            .map(|n| {
                let pc = machine.cpu.pc.wrapping_add(n * 4);
                let word = machine.bus.peek32(pc);
                (
                    pc,
                    word,
                    word.map_or_else(|| "unmapped".into(), |word| disassemble(pc, word)),
                )
            })
            .collect();
        let renderer_error = gpu.renderer_error();
        let halted = machine.halted.is_some() || renderer_error.is_some();
        Self {
            save_states: Vec::new(),
            quick_slot: QuickSlot::Empty,
            quick_slot_revision: 0,
            renamed_state: None,
            mode,
            cards: std::array::from_fn(|slot| {
                machine.bus.sio.cards[slot].as_ref().map(|card| CardStatus {
                    path: None,
                    pending: false,
                    reads: card.reads,
                    writes: card.writes,
                })
            }),
            pad_buttons: machine.bus.sio.buttons(),
            pad_polls: machine.bus.sio.polls,
            bios_path: None,
            disc_path: None,
            disc_fast_boot: false,
            media_selection: None,
            media_revision: 0,
            cdrom_reads: machine.bus.cdrom.sectors_read,
            disc_mounted: machine.bus.cdrom.mounted(),
            cpu: machine.cpu.clone(),
            cpu_mode: machine.cpu_mode(),
            cpu_stats: machine.cpu_stats(),
            title: machine.title.clone(),
            is_demo: machine.is_demo(),
            running: running && !halted,
            rewind_enabled: false,
            rewind_max_seconds: super::DEFAULT_REWIND_SECONDS,
            rewinding: false,
            rewind_states: 0,
            rewind_seconds: 0.0,
            halted,
            message: renderer_error
                .or(message)
                .or_else(|| machine.halted.as_ref().map(ToString::to_string)),
            success_message: None,
            close_ready: false,
            gpu_status: machine.bus.gpu.status(),
            renderer: machine.bus.gpu.renderer_mode(),
            renderer_revision: 0,
            renderer_device: machine.bus.gpu.renderer_device(),
            gpu_commands: machine.bus.gpu.commands,
            slices: machine.slices,
            rgb,
            dimensions,
            disassembly,
            slices_per_second: rate,
            video_frames: machine.bus.gpu.frames,
            video_fps: 0.0,
            target_fps: machine.bus.gpu.nominal_fps(),
        }
    }
}

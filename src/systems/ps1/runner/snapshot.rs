//! Immutable state published to frontends and diagnostic clients.
use crate::systems::ps1::{
    card_file::CardStatus,
    cpu::{Cpu, disassemble},
    cpu_engine::{CpuMode, CpuStats},
    gpu::RendererMode,
    machine::Machine,
};
use std::{path::PathBuf, sync::Arc};

pub struct Snapshot {
    pub cards: [Option<CardStatus>; 2],
    pub pad_buttons: u16,
    pub pad_polls: u64,
    /// Last successfully selected BIOS; failed loads do not replace it.
    pub bios_path: Option<PathBuf>,
    pub cdrom_reads: u64,
    pub disc_mounted: bool,
    pub cpu: Cpu,
    pub cpu_mode: CpuMode,
    pub cpu_stats: CpuStats,
    pub title: String,
    pub running: bool,
    pub message: Option<String>,
    pub close_ready: bool,
    pub gpu_status: u32,
    pub renderer: RendererMode,
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
    /// Equal pixels and dimensions can share the frontend's existing GPU texture.
    pub fn has_same_frame(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions
            && (Arc::ptr_eq(&self.rgb, &other.rgb) || self.rgb == other.rgb)
    }

    pub fn capture(machine: &Machine, running: bool, message: Option<String>, rate: f64) -> Self {
        let gpu = &machine.bus.gpu;
        let (dimensions, rgb) = gpu.completed_frame().map_or_else(
            || (gpu.dimensions(), gpu.display_rgb().into()),
            |frame| (frame.dimensions, Arc::clone(&frame.rgb)),
        );
        let disassembly = (0..9)
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
        Self {
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
            cdrom_reads: machine.bus.cdrom.sectors_read,
            disc_mounted: machine.bus.cdrom.mounted(),
            cpu: machine.cpu.clone(),
            cpu_mode: machine.cpu_mode(),
            cpu_stats: machine.cpu_stats(),
            title: machine.title.clone(),
            running: running && gpu.renderer_error().is_none(),
            message: gpu
                .renderer_error()
                .or(message)
                .or_else(|| machine.halted.as_ref().map(ToString::to_string)),
            close_ready: false,
            gpu_status: machine.bus.gpu.status(),
            renderer: machine.bus.gpu.renderer_mode(),
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

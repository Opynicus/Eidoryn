//! PlayStation session scheduling and transitions. No frontend dependencies.
mod rewind;

use super::{
    Command, QuickSlot, SelectedMedia, SharedSnapshot, Snapshot, SnapshotMode, pacing::FramePacing,
};
use crate::systems::ps1::{
    card_file::CardStore,
    cpu_engine::CpuMode,
    gpu::{RenderScale, RendererMode},
    input::PadInput,
    machine::{INSTRUCTIONS_PER_SLICE, Machine},
    media::{load_disc, load_path},
    state_file::{SaveStateInfo, StateStore},
};
use eidoryn_host::audio::{AudioBuffer, StereoFrame};
use rewind::RewindBuffer;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

const DISPLAY_INTERVAL: Duration = Duration::from_micros(16_667);
// Check input between bounded chunks, but only wait after completing a video frame.
const HOST_BUDGET: Duration = Duration::from_millis(4);

pub(super) fn run(
    machine: Machine,
    auto_run: bool,
    receiver: &mpsc::Receiver<Command>,
    output: &SharedSnapshot,
    store: CardStore,
    audio: Option<AudioBuffer>,
    mode: SnapshotMode,
) {
    let mut worker = Worker::new(machine, auto_run, store, audio, mode);
    loop {
        let wait = worker.rewind.wait_duration().or_else(|| {
            worker
                .running
                .then(|| worker.pacing.wait_duration(Instant::now()))
        });
        let command = if let Some(wait) = wait {
            match receiver.recv_timeout(wait) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        let view_only = matches!(command, Some(Command::SetSnapshotMode(_)));
        let state_command = matches!(
            command,
            Some(
                Command::SaveState
                    | Command::QuickSave
                    | Command::QuickSaveIfUnchanged { .. }
                    | Command::QuickLoad
                    | Command::LoadState { .. }
                    | Command::RenameState { .. }
                    | Command::DeleteState { .. }
                    | Command::RefreshSaveStates
                    | Command::SetRewinding(true)
            )
        );
        let mut force_publish = command
            .as_ref()
            .is_some_and(|c| !matches!(c, Command::SetButtons(_) | Command::SetTurbo(_)));
        let mut close_ready = match command {
            Some(Command::Shutdown) => break,
            Some(command) => worker.apply(command),
            None => {
                force_publish = if worker.rewind.active.is_some() {
                    worker.rewind_one()
                } else {
                    worker.run_batch()
                };
                false
            }
        };
        if !view_only && !state_command && !worker.flush_cards(!worker.running) {
            close_ready = false;
            force_publish = true;
        }
        worker.publish_audio();
        if (force_publish
            || !worker.running
            || close_ready
            || worker.last_publish.elapsed() >= DISPLAY_INTERVAL)
            && !worker.publish(output, close_ready)
        {
            break;
        }
        if close_ready {
            break;
        }
    }
    worker.end_rewind();
    if let Err(error) = worker.store.flush(&worker.machine.bus.sio.cards) {
        eprintln!("Eidoryn shutdown: {error}");
    }
    if let Some(audio) = &worker.audio {
        audio.set_playing(false);
    }
}

struct Worker {
    snapshot_mode: SnapshotMode,
    machine: Machine,
    input: PadInput,
    initial: Machine,
    renderer: RendererMode,
    render_scale: RenderScale,
    renderer_revision: u64,
    cpu_mode: CpuMode,
    running: bool,
    message: Option<String>,
    success_message: Option<String>,
    bios_path: Option<PathBuf>,
    disc_path: Option<PathBuf>,
    disc_fast_boot: bool,
    media_selection: Option<SelectedMedia>,
    media_revision: u64,
    renamed_state: Option<(PathBuf, PathBuf)>,
    pacing: FramePacing,
    last_publish: Instant,
    epoch: Instant,
    epoch_frames: u64,
    count: u32,
    rate: f64,
    video_fps: f64,
    store: CardStore,
    states: StateStore,
    save_states: Vec<SaveStateInfo>,
    quick_slot: QuickSlot,
    quick_slot_revision: u64,
    audio: Option<AudioBuffer>,
    audio_scratch: Vec<StereoFrame>,
    rewind: RewindBuffer,
}

impl Worker {
    fn new(
        mut machine: Machine,
        running: bool,
        store: CardStore,
        audio: Option<AudioBuffer>,
        snapshot_mode: SnapshotMode,
    ) -> Self {
        machine.bus.spu.drain_audio().for_each(drop);
        let now = Instant::now();
        let frames = machine.bus.gpu.frames;
        let pacing = FramePacing::new(now, frames, machine.bus.gpu.nominal_fps());
        let mut input = PadInput::default();
        input.set_buttons(machine.bus.sio.buttons(), machine.bus.elapsed_cycles());
        let mut worker = Self {
            input,
            snapshot_mode,
            initial: machine.clone(),
            renderer: machine.bus.gpu.renderer_mode(),
            render_scale: machine.bus.gpu.render_scale(),
            renderer_revision: 0,
            cpu_mode: machine.cpu_mode(),
            states: StateStore::new(Path::new("saves"), &machine),
            save_states: Vec::new(),
            quick_slot: QuickSlot::Empty,
            quick_slot_revision: 0,
            machine,
            running,
            message: None,
            success_message: None,
            bios_path: None,
            disc_path: None,
            disc_fast_boot: false,
            media_selection: None,
            media_revision: 0,
            renamed_state: None,
            pacing,
            last_publish: now,
            epoch: now,
            epoch_frames: frames,
            count: 0,
            rate: 0.0,
            video_fps: 0.0,
            store,
            audio,
            audio_scratch: Vec::new(),
            rewind: RewindBuffer::default(),
        };
        worker.refresh_states();
        worker.capture_rewind();
        worker
    }

    fn reset_pacing(&mut self) {
        let now = Instant::now();
        self.pacing = FramePacing::new(
            now,
            self.machine.bus.gpu.frames,
            self.machine.bus.gpu.nominal_fps(),
        );
        self.epoch = now;
        self.epoch_frames = self.machine.bus.gpu.frames;
        self.count = 0;
        self.rate = 0.0;
        self.video_fps = 0.0;
    }

    fn replace_machine(&mut self, mut machine: Machine) -> bool {
        if let Err(error) = machine.set_cpu_mode(self.cpu_mode) {
            self.message = Some(format!("Cannot start CPU engine: {error}"));
            self.running = false;
            return false;
        }
        if let Err(error) = machine
            .bus
            .gpu
            .set_renderer_with_scale(self.renderer, self.render_scale)
        {
            self.message = Some(format!("Cannot start GPU renderer: {error}"));
            self.running = false;
            return false;
        }
        machine.bus.sio.cards = std::mem::take(&mut self.machine.bus.sio.cards);
        machine.bus.sio.deselect_cards();
        self.machine = machine;
        self.release_input();
        self.initial.clone_from(&self.machine);
        self.states = StateStore::new(Path::new("saves"), &self.initial);
        self.quick_slot_revision = self.quick_slot_revision.wrapping_add(1);
        self.save_states.clear();
        self.renamed_state = None;
        self.message = None;
        self.refresh_states();
        self.reset_rewind();
        true
    }

    fn select_cpu(&mut self, mode: CpuMode) {
        match self.machine.set_cpu_mode(mode) {
            Ok(()) => {
                self.cpu_mode = mode;
                self.message = None;
            }
            Err(error) => {
                self.message = Some(format!("Cannot switch CPU engine: {error}"));
                self.running = false;
            }
        }
        self.reset_pacing();
    }

    fn select_renderer(&mut self, mode: RendererMode, scale: RenderScale) {
        match self.machine.bus.gpu.set_renderer_with_scale(mode, scale) {
            Ok(()) => {
                self.renderer = mode;
                self.render_scale = self.machine.bus.gpu.render_scale();
                self.message = None;
            }
            Err(error) => {
                self.message = Some(format!("Cannot switch GPU renderer: {error}"));
                self.running = false;
            }
        }
        self.reset_pacing();
    }

    fn load_image(&mut self, path: PathBuf, bios: bool) {
        self.running = false;
        self.release_input();
        match load_path(&path, bios) {
            Ok(loaded) => {
                if self.replace_machine(loaded) {
                    if bios {
                        self.bios_path = Some(path);
                    }
                    self.disc_path = None;
                    self.disc_fast_boot = false;
                    self.media_selection = Some(if bios {
                        SelectedMedia::Bios
                    } else {
                        SelectedMedia::Exe
                    });
                    self.media_revision = self.media_revision.wrapping_add(1);
                }
                self.reset_pacing();
            }
            // A read/validation failure retains the previous pacing and statistics.
            Err(error) => self.message = Some(error),
        }
    }

    fn load_disc(
        &mut self,
        path: PathBuf,
        bios: Option<PathBuf>,
        fast_boot: bool,
        selection: SelectedMedia,
    ) {
        self.running = false;
        self.release_input();
        match load_disc(&path, bios.as_deref(), fast_boot) {
            Ok(loaded) => {
                if self.replace_machine(loaded) {
                    if bios.is_some() {
                        self.bios_path = bios;
                    }
                    self.running = fast_boot;
                    self.disc_path = Some(path);
                    self.disc_fast_boot = fast_boot;
                    self.media_selection = Some(selection);
                    self.media_revision = self.media_revision.wrapping_add(1);
                }
            }
            Err(error) => self.message = Some(error),
        }
        // Unlike an image read failure, a failed disc load resets timing too.
        self.reset_pacing();
    }

    fn refresh_quick_slot(&mut self) {
        self.quick_slot = QuickSlot::inspect(&self.states, &mut self.message);
        if self.quick_slot == QuickSlot::Unavailable {
            self.success_message = None;
        }
    }

    fn refresh_states(&mut self) {
        self.refresh_quick_slot();
        match self.states.list() {
            Ok(states) => self.save_states = states,
            Err(error) => {
                self.message = Some(error);
                self.success_message = None;
            }
        }
    }

    fn save_state(&mut self, quick: bool) {
        let result = if quick {
            self.states.save_quick(&self.machine, &self.input)
        } else {
            self.states.save(&self.machine, &self.input)
        };
        match result {
            Ok(path) => {
                self.message = None;
                let action = if quick {
                    self.quick_slot_revision = self.quick_slot_revision.wrapping_add(1);
                    "Quick-saved state"
                } else {
                    "Saved state"
                };
                self.success_message = Some(format!("{action}: {}", path.display()));
            }
            Err(error) => self.message = Some(error),
        }
        self.refresh_states();
        self.reset_pacing();
    }

    fn load_state(&mut self, path: &Path) {
        let prepare = || -> Result<_, String> {
            let (mut machine, input) = self.states.load(path)?;
            machine
                .set_cpu_mode(self.cpu_mode)
                .map_err(|e| format!("Cannot start CPU engine: {e}"))?;
            machine
                .bus
                .gpu
                .set_renderer_with_scale(self.renderer, self.render_scale)
                .map_err(|e| format!("Cannot start GPU renderer: {e}"))?;
            Ok((machine, input))
        };
        match prepare() {
            Ok((machine, input)) => {
                let held = self.input.held_buttons();
                self.machine = machine;
                self.input = input;
                if self.running {
                    // Resuming immediately must not keep historical buttons stuck down.
                    self.input
                        .set_buttons(held, self.machine.bus.elapsed_cycles());
                    self.update_input();
                }
                self.store.invalidate();
                if let Some(audio) = &self.audio {
                    audio.clear();
                    audio.set_playing(self.running);
                }
                self.message = None;
                self.success_message = Some(format!("Loaded state: {}", path.display()));
                self.reset_pacing();
                self.reset_rewind();
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn rename_state(&mut self, path: &Path, name: &str) {
        match self.states.rename(path, name) {
            Ok(renamed) => {
                self.message = None;
                self.success_message = Some(format!("Renamed snapshot: {}", renamed.display()));
                self.renamed_state = Some((path.to_path_buf(), renamed));
            }
            Err(error) => self.message = Some(error),
        }
        self.refresh_states();
    }

    /// Returns true when the frontend should close after receiving the final snapshot.
    #[allow(clippy::too_many_lines)] // Keep command ordering and persistence guards together.
    fn apply(&mut self, command: Command) -> bool {
        match command {
            Command::SetRewinding(enabled) => {
                if enabled {
                    self.begin_rewind();
                } else {
                    self.end_rewind();
                }
                return false;
            }
            Command::SetRewindEnabled(enabled) => {
                if self.rewind.enabled != enabled {
                    self.end_rewind();
                    self.rewind.enabled = enabled;
                    self.reset_rewind();
                }
                return false;
            }
            Command::SetRewindSeconds(seconds) => {
                self.set_rewind_seconds(seconds);
                return false;
            }
            Command::SetButtons(buttons) if self.rewind.active.is_some() => {
                self.rewind.active.as_mut().expect("active rewind").held = buttons & !6;
                return false;
            }
            Command::SetSnapshotMode(_) | Command::DeleteState { .. } => {}
            _ => {
                self.end_rewind();
            }
        }
        let view_only = matches!(command, Command::SetSnapshotMode(_));
        let input_only = matches!(command, Command::SetButtons(_) | Command::SetTurbo(_));
        if !view_only && !input_only && !matches!(command, Command::RefreshSaveStates) {
            self.success_message = None;
        }
        // Checkpoints remain available even when a memory-card file cannot be flushed.
        match command {
            Command::SaveState | Command::QuickSave => {
                self.save_state(matches!(command, Command::QuickSave));
                return false;
            }
            Command::QuickSaveIfUnchanged { revision, slot } => {
                self.refresh_quick_slot();
                if self.quick_slot != QuickSlot::Unavailable {
                    if self.quick_slot == slot && self.quick_slot_revision == revision {
                        self.save_state(true);
                    } else {
                        self.message = Some(
                            "Quick slot changed. Check the current slot and try again.".into(),
                        );
                    }
                }
                return false;
            }
            Command::QuickLoad => {
                self.load_state(&self.states.quick_path());
                self.refresh_quick_slot();
                return false;
            }
            Command::LoadState { path } => {
                self.load_state(&path);
                return false;
            }
            Command::RenameState { path, name } => {
                self.rename_state(&path, &name);
                return false;
            }
            Command::DeleteState { path, modified } => {
                match self.states.delete(&path, modified) {
                    Ok(()) => {
                        self.save_states.retain(|state| state.path != path);
                        self.message = None;
                        self.success_message =
                            Some(format!("Deleted snapshot: {}", path.display()));
                    }
                    Err(error) => self.message = Some(error),
                }
                self.refresh_states();
                return false;
            }
            Command::RefreshSaveStates => {
                self.refresh_states();
                return false;
            }
            _ => {}
        }
        if matches!(
            command,
            Command::SetRenderer(_) | Command::SetRendererWithScale { .. }
        ) {
            // Also acknowledge a request rejected by the card-flush guard below.
            self.renderer_revision = self.renderer_revision.wrapping_add(1);
        }
        if !view_only && !input_only && !self.flush_cards(true) {
            return false;
        }
        if !view_only && !input_only && !matches!(command, Command::FlushCards) {
            if let Some(audio) = &self.audio {
                audio.clear();
            }
            self.machine.bus.spu.drain_audio().for_each(drop);
        }
        match command {
            Command::SetSnapshotMode(mode) => self.snapshot_mode = mode,
            Command::SetButtons(buttons) => {
                self.input
                    .set_buttons(buttons, self.machine.bus.elapsed_cycles());
                self.update_input();
            }
            Command::SetTurbo(config) => {
                self.input
                    .configure(config, self.machine.bus.elapsed_cycles());
                self.update_input();
            }
            Command::SetCpuMode(mode) => self.select_cpu(mode),
            Command::SetRenderer(mode) => self.select_renderer(mode, self.render_scale),
            Command::SetRendererWithScale { mode, scale } => self.select_renderer(mode, scale),
            Command::Toggle => {
                self.message = None;
                self.running = !self.running;
                if !self.running {
                    self.release_input();
                }
                self.reset_pacing();
            }
            Command::Step => {
                self.running = false;
                self.update_input();
                self.message = self.machine.step().err().map(|e| e.to_string());
                self.update_input();
                self.machine.bus.gpu.latch_display();
            }
            Command::Slice => {
                self.running = false;
                self.update_input();
                self.message = self.machine.run_slice().err().map(|e| e.to_string());
                self.update_input();
                self.machine.bus.gpu.latch_display();
            }
            Command::Reset => {
                self.running = false;
                self.replace_machine(self.initial.clone());
                self.release_input();
                self.reset_pacing();
            }
            Command::Demo => {
                self.running = self.replace_machine(Machine::demo());
                if self.running {
                    self.disc_path = None;
                    self.disc_fast_boot = false;
                    self.media_selection = Some(SelectedMedia::Demo);
                    self.media_revision = self.media_revision.wrapping_add(1);
                }
                self.reset_pacing();
            }
            Command::FlushCards => self.message = None,
            Command::SaveState
            | Command::SetRewinding(_)
            | Command::SetRewindEnabled(_)
            | Command::SetRewindSeconds(_)
            | Command::QuickSave
            | Command::QuickLoad
            | Command::QuickSaveIfUnchanged { .. }
            | Command::LoadState { .. }
            | Command::RenameState { .. }
            | Command::DeleteState { .. }
            | Command::RefreshSaveStates => {
                unreachable!("handled before card persistence")
            }
            Command::Load { path, bios } => self.load_image(path, bios),
            Command::LoadDisc {
                path,
                bios,
                fast_boot,
            } => self.load_disc(path, bios, fast_boot, SelectedMedia::Disc),
            Command::ReopenDisc {
                path,
                bios,
                fast_boot,
            } => self.load_disc(path, bios, fast_boot, SelectedMedia::ReopenedDisc),
            Command::RequestClose | Command::Shutdown => {
                self.running = false;
                self.release_input();
                return true;
            }
        }
        self.capture_rewind();
        false
    }

    fn flush_cards(&mut self, force: bool) -> bool {
        if self.rewind.active.is_some() {
            return true;
        }
        let result = if force {
            self.store.flush(&self.machine.bus.sio.cards)
        } else {
            self.store.flush_if_due(&self.machine.bus.sio.cards)
        };
        if let Err(error) = result {
            self.message = Some(error);
            self.success_message = None;
            self.running = false;
            return false;
        }
        true
    }

    fn publish_audio(&mut self) {
        if let Some(audio) = &self.audio {
            audio.set_playing(self.running && self.rewind.active.is_none());
            self.audio_scratch
                .extend(self.machine.bus.spu.drain_audio());
            audio.push(&self.audio_scratch);
            self.audio_scratch.clear();
        } else {
            self.machine.bus.spu.drain_audio().for_each(drop);
        }
    }

    fn run_batch(&mut self) -> bool {
        let start = Instant::now();
        let mut frame_finished = false;
        loop {
            self.update_input();
            match self.machine.run_slice() {
                Ok(n) => {
                    self.count += 1;
                    // The built-in demo explicitly yields at a drawn frame.
                    if n < INSTRUCTIONS_PER_SLICE
                        || self.pacing.frame_reached(self.machine.bus.gpu.frames)
                    {
                        frame_finished = true;
                        break;
                    }
                }
                Err(error) => {
                    self.message = Some(error.to_string());
                    self.success_message = None;
                    self.running = false;
                    self.release_input();
                    break;
                }
            }
            if start.elapsed() >= HOST_BUDGET {
                break;
            }
        }
        self.update_input();
        let now = Instant::now();
        self.pacing.finish_batch(
            now,
            self.machine.bus.gpu.frames,
            self.machine.bus.gpu.nominal_fps(),
            frame_finished,
        );
        self.capture_rewind();
        frame_finished || !self.running
    }

    fn update_input(&mut self) {
        self.machine
            .bus
            .sio
            .set_buttons(self.input.buttons(self.machine.bus.elapsed_cycles()));
    }

    fn release_input(&mut self) {
        self.input.set_buttons(0, self.machine.bus.elapsed_cycles());
        self.update_input();
    }

    fn publish(&mut self, output: &SharedSnapshot, close_ready: bool) -> bool {
        if self.epoch.elapsed() >= Duration::from_secs(1) {
            let elapsed = self.epoch.elapsed().as_secs_f64();
            self.rate = f64::from(self.count) / elapsed;
            self.video_fps = self
                .machine
                .bus
                .gpu
                .frames
                .saturating_sub(self.epoch_frames) as f64
                / elapsed;
            self.count = 0;
            self.epoch_frames = self.machine.bus.gpu.frames;
            self.epoch = Instant::now();
        }
        let mut snapshot = Snapshot::capture_with_mode(
            &self.machine,
            self.running,
            self.message.clone(),
            if self.running { self.rate } else { 0.0 },
            self.snapshot_mode,
        );
        snapshot.bios_path.clone_from(&self.bios_path);
        snapshot.disc_path.clone_from(&self.disc_path);
        snapshot.disc_fast_boot = self.disc_fast_boot;
        snapshot.media_selection = self.media_selection;
        snapshot.media_revision = self.media_revision;
        snapshot.renamed_state.clone_from(&self.renamed_state);
        snapshot.rewind_enabled = self.rewind.enabled;
        snapshot.rewind_max_seconds = self.rewind.max_seconds;
        snapshot.rewinding = self.rewind.active.is_some();
        (snapshot.rewind_states, snapshot.rewind_seconds) =
            self.rewind.available(self.machine.elapsed_cycles());
        if snapshot.rewinding {
            snapshot.renderer = self.renderer;
            snapshot.render_scale = self.render_scale;
            snapshot.cpu_mode = self.cpu_mode;
        }
        if snapshot.message.is_none() {
            snapshot.success_message.clone_from(&self.success_message);
        }
        snapshot.save_states.clone_from(&self.save_states);
        snapshot.quick_slot = self.quick_slot;
        snapshot.quick_slot_revision = self.quick_slot_revision;
        snapshot.renderer_revision = self.renderer_revision;
        snapshot.cards = self.store.status(&self.machine.bus.sio.cards);
        snapshot.video_fps = if self.running { self.video_fps } else { 0.0 };
        snapshot.close_ready = close_ready;
        let previous = {
            let Ok(mut slot) = output.lock() else {
                return false;
            };
            slot.replace(snapshot)
        };
        // Releasing strings/pixels can reach the allocator. Keep that work
        // outside the mailbox lock shared with the UI thread.
        drop(previous);
        self.last_publish = Instant::now();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::ps1::{input::TurboConfig, timing::CPU_CLOCK_HZ};

    #[test]
    fn audio_publication_drains_spu_and_reuses_staging_capacity() {
        let audio = AudioBuffer::default();
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            Some(audio.clone()),
            SnapshotMode::Display,
        );
        worker
            .machine
            .bus
            .spu
            .tick(crate::spu::SAMPLE_CLOCKS * 2048)
            .unwrap();
        worker.publish_audio();
        assert_eq!(audio.stats().queued_frames, 2048);
        let allocation = worker.audio_scratch.as_ptr();
        let capacity = worker.audio_scratch.capacity();
        for frames in [32, 256, 2048] {
            audio.clear();
            worker
                .machine
                .bus
                .spu
                .tick(crate::spu::SAMPLE_CLOCKS * frames)
                .unwrap();
            worker.publish_audio();
            assert_eq!(audio.stats().queued_frames, frames as usize);
            assert_eq!(worker.machine.bus.spu.drain_audio().count(), 0);
            assert_eq!(worker.audio_scratch.len(), 0);
            assert_eq!(worker.audio_scratch.capacity(), capacity);
            assert_eq!(worker.audio_scratch.as_ptr(), allocation);
        }
    }

    #[test]
    fn loads_preserve_execution_mode_and_resume_with_live_buttons() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-load-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let audio = AudioBuffer::default();
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            Some(audio.clone()),
            SnapshotMode::Display,
        );
        worker.states = StateStore::new(&root, &worker.initial);
        worker.machine.cpu.registers[5] = 33;
        worker.apply(Command::SetButtons(0x8000));
        worker.apply(Command::SaveState);
        let path = worker.save_states[0].path.clone();
        worker.apply(Command::QuickSave);
        for quick in [false, true] {
            for (running, held) in [(false, 0), (true, 0), (true, 0x4000)] {
                worker.running = running;
                worker.apply(Command::SetButtons(held));
                worker.machine.cpu.registers[5] = 99;
                audio.set_playing(true);
                audio.push(&[[123, -123]; 64]);
                worker.apply(if quick {
                    Command::QuickLoad
                } else {
                    Command::LoadState { path: path.clone() }
                });
                assert!(worker.message.is_none());
                assert_eq!(worker.running, running);
                assert_eq!(worker.machine.cpu.registers[5], 33);
                let buttons = if running { held } else { 0x8000 };
                assert_eq!(worker.input.held_buttons(), buttons);
                assert_eq!(worker.machine.bus.sio.buttons(), buttons);
                assert_eq!(audio.stats().queued_frames, 0);
                audio.push(&[[123, -123]; 4]);
                assert_eq!(audio.stats().queued_frames, if running { 4 } else { 0 });
                if running {
                    worker.run_batch();
                    assert!(worker.count > 0);
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quick_load_rewinds_the_slot_and_failed_loads_preserve_execution() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-worker-quick-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        worker.states = StateStore::new(&root, &worker.initial);
        let initial = worker.machine.save_state().unwrap();
        worker.apply(Command::QuickLoad);
        assert!(worker.message.is_some());
        assert!(worker.success_message.is_none());
        assert!(worker.running);
        assert_eq!(worker.machine.save_state().unwrap(), initial);

        worker.apply(Command::QuickSave);
        assert!(worker.message.is_none());
        assert!(worker.success_message.is_some());
        assert!(worker.running);
        worker.machine.run_slice().unwrap();
        worker.apply(Command::SetButtons(0x8000));
        let saved = worker.machine.save_state().unwrap();
        let input = worker.input.save_state();
        let old_revision = worker.quick_slot_revision;
        worker.apply(Command::QuickSave);
        assert!(worker.save_states.is_empty());
        worker.machine.run_slice().unwrap();
        // An earlier confirmation must fail even on a filesystem with coarse timestamps.
        worker.apply(Command::QuickSaveIfUnchanged {
            revision: old_revision,
            slot: worker.quick_slot,
        });
        assert!(worker.message.as_ref().unwrap().contains("slot changed"));
        worker.apply(Command::QuickLoad);
        assert!(worker.running);
        assert!(worker.message.is_none());
        assert!(worker.success_message.is_some());
        assert_eq!(worker.machine.save_state().unwrap(), saved);
        assert_eq!(worker.input.save_state(), input);

        std::fs::write(worker.states.quick_path(), b"damaged").unwrap();
        worker.running = true;
        let pacing = worker.pacing;
        worker.apply(Command::QuickLoad);
        assert!(worker.running);
        assert_eq!(worker.pacing, pacing);
        assert!(worker.message.is_some());
        assert!(worker.success_message.is_none());
        assert_eq!(worker.machine.save_state().unwrap(), saved);
        assert_eq!(worker.input.save_state(), input);
        std::fs::remove_file(worker.states.quick_path()).unwrap();
        worker.apply(Command::QuickLoad);
        assert_eq!(worker.quick_slot, QuickSlot::Empty);
        std::fs::create_dir(worker.states.quick_path()).unwrap();
        worker.apply(Command::RefreshSaveStates);
        assert_eq!(worker.quick_slot, QuickSlot::Unavailable);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hardware_validation_failure_preserves_running_and_paused_sessions() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-invalid-hardware-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for running in [false, true] {
            let audio = AudioBuffer::default();
            let mut worker = Worker::new(
                Machine::demo(),
                running,
                CardStore::default(),
                Some(audio.clone()),
                SnapshotMode::Display,
            );
            worker.states = StateStore::new(&root, &worker.initial);
            worker.apply(Command::SetButtons(0x8000));
            worker.machine.run_slice().unwrap();
            audio.set_playing(running);
            // Use the normal encoders so both checksums pass and loading reaches
            // Machine's hardware validation before the runner rejects the state.
            let mut invalid = Machine::demo();
            invalid.bus.ram.clear();
            let path = worker.states.save(&invalid, &PadInput::default()).unwrap();
            worker
                .states
                .save_quick(&invalid, &PadInput::default())
                .unwrap();
            for quick in [false, true] {
                audio.push(&[[123, -123]; 64]);
                let queued = audio.stats().queued_frames;
                let before = worker.machine.save_state().unwrap();
                let input = worker.input.save_state();
                let pacing = worker.pacing;
                let mut expected = worker.machine.clone();
                worker.apply(if quick {
                    Command::QuickLoad
                } else {
                    Command::LoadState { path: path.clone() }
                });
                assert!(
                    worker
                        .message
                        .as_deref()
                        .unwrap()
                        .contains("Invalid saved RAM or BIOS size"),
                    "running={running}, quick={quick}"
                );
                assert!(worker.success_message.is_none());
                assert_eq!(worker.running, running);
                assert_eq!(worker.machine.save_state().unwrap(), before);
                assert_eq!(worker.input.save_state(), input);
                assert_eq!(worker.pacing, pacing);
                assert_eq!(audio.stats().queued_frames, queued);
                audio.push(&[[123, -123]; 4]);
                assert_eq!(
                    audio.stats().queued_frames,
                    queued + if running { 4 } else { 0 }
                );
                worker.machine.run_slice().unwrap();
                expected.run_slice().unwrap();
                assert_eq!(
                    worker.machine.save_state().unwrap(),
                    expected.save_state().unwrap()
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_reports_new_path_without_changing_execution() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-worker-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        worker.states = StateStore::new(&root, &worker.initial);
        worker.apply(Command::SaveState);
        let original = worker.save_states[0].path.clone();
        let machine = worker.machine.save_state().unwrap();
        let pacing = worker.pacing;
        worker.apply(Command::RenameState {
            path: original.clone(),
            name: "关卡二".into(),
        });
        let renamed = worker.save_states[0].path.clone();
        assert_eq!(worker.save_states[0].name, "关卡二");
        assert_eq!(worker.renamed_state, Some((original, renamed.clone())));
        assert!(worker.success_message.is_some());
        assert!(worker.message.is_none());
        assert!(worker.running);
        assert_eq!(worker.pacing, pacing);
        assert_eq!(worker.machine.save_state().unwrap(), machine);
        worker.apply(Command::SaveState);
        let newer = worker.save_states[0].path.clone();
        assert_ne!(newer, renamed);
        worker.apply(Command::RenameState {
            path: newer.clone(),
            name: "关卡二".into(),
        });
        assert!(
            worker
                .message
                .as_deref()
                .unwrap()
                .contains("already exists")
        );
        assert!(worker.success_message.is_none());
        assert!(newer.exists() && renamed.exists());
        let output = std::sync::Arc::new(std::sync::Mutex::new(None));
        worker.publish(&output, false);
        assert_eq!(
            output.lock().unwrap().as_ref().unwrap().renamed_state,
            worker.renamed_state
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_history_refreshes_the_list_and_preserves_playback() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-worker-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        worker.states = StateStore::new(&root, &worker.initial);
        worker.apply(Command::QuickSave);
        worker.apply(Command::SaveState);
        worker.apply(Command::SaveState);
        let saved = worker.save_states.clone();
        let machine = worker.machine.save_state().unwrap();
        let slot = worker.quick_slot;
        let pacing = worker.pacing;
        for (index, state) in saved.iter().enumerate() {
            worker.running = index == 0;
            let command = || Command::DeleteState {
                path: state.path.clone(),
                modified: state.modified,
            };
            worker.apply(command());
            assert_eq!(worker.save_states.len(), saved.len() - index - 1);
            assert!(!state.path.exists());
            assert!(worker.message.is_none());
            assert!(
                worker
                    .success_message
                    .as_ref()
                    .unwrap()
                    .contains(&state.name)
            );
            worker.apply(command());
            assert!(worker.message.is_some());
            assert!(worker.success_message.is_none());
            assert_eq!(worker.running, index == 0);
            assert_eq!(worker.machine.save_state().unwrap(), machine);
            assert_eq!(worker.quick_slot, slot);
            assert_eq!(worker.pacing, pacing);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn media_receipts_only_advance_after_successful_selection() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-media-receipts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let disc = root.join("game.iso");
        let bios = root.join("bios.bin");
        let exe = root.join("game.exe");
        std::fs::write(&disc, [0; 2048]).unwrap();
        std::fs::write(&bios, vec![0; crate::systems::ps1::bus::BIOS_SIZE]).unwrap();
        std::fs::write(&exe, crate::systems::ps1::demo::executable()).unwrap();
        let mut worker = Worker::new(
            Machine::demo(),
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        for revision in [1, 2] {
            worker.apply(Command::LoadDisc {
                path: disc.clone(),
                bios: Some(bios.clone()),
                fast_boot: false,
            });
            assert_eq!(worker.media_revision, revision);
            assert_eq!(worker.media_selection, Some(SelectedMedia::Disc));
            assert_eq!(worker.disc_path, Some(disc.clone()));
            assert!(!worker.disc_fast_boot);
        }
        worker.apply(Command::Reset);
        assert_eq!(worker.media_revision, 2);
        assert_eq!(worker.disc_path, Some(disc.clone()));
        worker.apply(Command::LoadDisc {
            path: root.join("missing.iso"),
            bios: Some(bios.clone()),
            fast_boot: false,
        });
        assert_eq!(worker.media_revision, 2);
        assert_eq!(worker.disc_path, Some(disc));
        for (path, is_bios, selection, revision) in [
            (bios.clone(), true, SelectedMedia::Bios, 3),
            (bios.clone(), true, SelectedMedia::Bios, 4),
            (exe, false, SelectedMedia::Exe, 5),
        ] {
            worker.apply(Command::Load {
                path,
                bios: is_bios,
            });
            assert_eq!(worker.media_revision, revision);
            assert_eq!(worker.media_selection, Some(selection));
            assert!(worker.disc_path.is_none());
        }
        worker.apply(Command::Demo);
        assert_eq!(worker.media_revision, 6);
        assert_eq!(worker.media_selection, Some(SelectedMedia::Demo));
        assert_eq!(worker.bios_path, Some(bios));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_state_rewinds_without_cards_and_failed_restore_preserves_session() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-rewind-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let audio = AudioBuffer::default();
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            Some(audio.clone()),
            SnapshotMode::Debug,
        );
        worker.states = StateStore::new(&root, &worker.initial);
        worker.apply(Command::SetTurbo(TurboConfig::new(0x8000, 10).unwrap()));
        worker.apply(Command::SetButtons(0x8000));
        worker.machine.run_slice().unwrap();
        let saved = worker.machine.save_state().unwrap();
        let input = worker.input.save_state();
        worker.apply(Command::SaveState);
        assert!(worker.running);
        assert!(worker.message.is_none());
        assert!(
            worker
                .success_message
                .as_deref()
                .unwrap()
                .starts_with("Saved state:")
        );
        let success = worker.success_message.clone();
        worker.apply(Command::RefreshSaveStates);
        worker.apply(Command::SetSnapshotMode(SnapshotMode::Display));
        assert_eq!(worker.success_message, success);
        assert_eq!(worker.machine.save_state().unwrap(), saved);
        let path = worker.save_states[0].path.clone();
        worker.machine.run_slice().unwrap();
        audio.push(&[[123, -123]; 64]);
        worker.apply(Command::LoadState { path: path.clone() });
        assert!(worker.running);
        assert!(worker.message.is_none());
        assert!(
            worker
                .success_message
                .as_deref()
                .unwrap()
                .starts_with("Loaded state:")
        );
        let published = std::sync::Arc::new(std::sync::Mutex::new(None));
        assert!(worker.publish(&published, false));
        let snapshot = published.lock().unwrap().take().unwrap();
        assert!(snapshot.message.is_none());
        assert_eq!(snapshot.success_message, worker.success_message);
        assert_eq!(audio.stats().queued_frames, 0);
        assert_eq!(worker.machine.save_state().unwrap(), saved);
        assert_eq!(worker.input.save_state(), input);
        assert!(worker.machine.bus.sio.cards.iter().all(Option::is_none));
        let mut expected = Machine::load_state(&saved).unwrap();
        expected.run_slice().unwrap();
        worker.machine.run_slice().unwrap();
        assert_eq!(
            worker.machine.save_state().unwrap(),
            expected.save_state().unwrap()
        );
        worker.running = true;
        audio.set_playing(true);
        let before = worker.machine.save_state().unwrap();
        let pacing = worker.pacing;
        audio.push(&[[123, -123]; 64]);
        std::fs::write(&path, b"broken").unwrap();
        worker.apply(Command::LoadState { path });
        assert!(worker.running);
        assert_eq!(worker.pacing, pacing);
        assert_eq!(worker.machine.save_state().unwrap(), before);
        assert_eq!(audio.stats().queued_frames, 64);
        assert!(worker.message.as_deref().unwrap().contains("Invalid"));
        assert!(worker.success_message.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn game_changes_and_paused_startup_load_their_own_history() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("eidoryn-history-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let game_a = root.join(format!("history-a-{stamp}.exe"));
        let game_b = root.join(format!("history-b-{stamp}.exe"));
        let bytes = crate::systems::ps1::demo::executable();
        std::fs::write(&game_a, &bytes).unwrap();
        std::fs::write(&game_b, &bytes).unwrap();
        let mut worker = Worker::new(
            load_path(&game_a, false).unwrap(),
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        assert!(worker.save_states.is_empty());
        worker.apply(Command::SaveState);
        let saved_a = worker.save_states[0].path.clone();
        worker.apply(Command::QuickSave);
        let slot_a = worker.quick_slot;
        let revision_a = worker.quick_slot_revision;
        assert!(matches!(slot_a, QuickSlot::Saved(_)));
        worker.apply(Command::Load {
            path: game_b.clone(),
            bios: false,
        });
        assert!(worker.save_states.is_empty());
        assert!(worker.success_message.is_none());
        worker.apply(Command::SaveState);
        let saved_b = worker.save_states[0].path.clone();
        worker.apply(Command::QuickSaveIfUnchanged {
            revision: revision_a,
            slot: slot_a,
        });
        assert!(!worker.states.quick_path().exists());
        assert_eq!(worker.quick_slot, QuickSlot::Empty);
        assert_ne!(saved_a.parent(), saved_b.parent());
        for (game, saved) in [
            (&game_a, &saved_a),
            (&game_b, &saved_b),
            (&game_a, &saved_a),
        ] {
            worker.apply(Command::Load {
                path: game.clone(),
                bios: false,
            });
            assert_eq!(worker.save_states.len(), 1);
            assert_eq!(&worker.save_states[0].path, saved);
            assert_eq!(
                worker.quick_slot,
                if game == &game_a {
                    slot_a
                } else {
                    QuickSlot::Empty
                }
            );
        }
        worker.apply(Command::Load {
            path: root.join("missing.exe"),
            bios: false,
        });
        assert_eq!(worker.save_states.len(), 1);
        assert_eq!(worker.save_states[0].path, saved_a);
        worker.apply(Command::Reset);
        assert_eq!(worker.save_states.len(), 1);
        assert_eq!(worker.save_states[0].path, saved_a);

        let restarted =
            super::super::Runner::new(load_path(&game_a, false).unwrap(), false).unwrap();
        let snapshot = restarted.take_snapshot().unwrap();
        assert!(!snapshot.running);
        assert_eq!(snapshot.save_states.len(), 1);
        assert_eq!(snapshot.save_states[0].path, saved_a);
        assert_eq!(snapshot.quick_slot, slot_a);
        drop(restarted);
        for path in [saved_a, saved_b] {
            std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_loads_preserve_their_distinct_timing_and_clear_audio() {
        for (command, resets) in [
            (
                Command::Load {
                    path: PathBuf::new(),
                    bios: false,
                },
                false,
            ),
            (
                Command::LoadDisc {
                    path: PathBuf::new(),
                    bios: None,
                    fast_boot: true,
                },
                true,
            ),
        ] {
            let audio = AudioBuffer::default();
            let mut worker = Worker::new(
                Machine::demo(),
                true,
                CardStore::default(),
                Some(audio.clone()),
                SnapshotMode::Display,
            );
            let previous = Instant::now() + Duration::from_secs(60);
            worker.pacing = FramePacing::new(previous, 5, 50.0);
            worker.epoch = previous;
            worker.epoch_frames = 5;
            worker.machine.bus.gpu.frames = 7;
            worker.count = 12;
            worker.rate = 30.0;
            worker.video_fps = 50.0;
            worker.bios_path = Some("previous.bin".into());
            worker.apply(Command::SetButtons(0x8000));
            let pacing = worker.pacing;
            let pc = worker.machine.cpu.pc;
            audio.set_playing(true);
            audio.push(&[[123, -123]; 128]);
            worker
                .machine
                .bus
                .spu
                .tick(crate::systems::ps1::spu::SAMPLE_CLOCKS * 16)
                .unwrap();

            assert!(!worker.apply(command));
            assert!(!worker.running);
            assert!(worker.message.is_some());
            assert_eq!(worker.machine.cpu.pc, pc);
            assert_eq!(worker.machine.bus.sio.buttons(), 0);
            assert_eq!(worker.bios_path, Some("previous.bin".into()));
            assert_eq!(audio.stats().queued_frames, 0);
            assert_eq!(worker.machine.bus.spu.drain_audio().count(), 0);
            if resets {
                assert_ne!(worker.epoch, previous);
                assert_eq!(worker.pacing.wait_duration(worker.epoch), Duration::ZERO);
                assert!(!worker.pacing.frame_reached(7));
                assert!(worker.pacing.frame_reached(8));
                assert_eq!(
                    (
                        worker.epoch_frames,
                        worker.count,
                        worker.rate,
                        worker.video_fps
                    ),
                    (7, 0, 0.0, 0.0)
                );
            } else {
                assert_eq!(worker.pacing, pacing);
                assert_eq!(worker.epoch, previous);
                assert_eq!(
                    (
                        worker.epoch_frames,
                        worker.count,
                        worker.rate,
                        worker.video_fps
                    ),
                    (5, 12, 30.0, 50.0)
                );
            }
        }
    }

    #[test]
    fn turbo_reaches_sio_without_clearing_audio_and_survives_session_reset() {
        let audio = AudioBuffer::default();
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            Some(audio.clone()),
            SnapshotMode::Display,
        );
        audio.push(&[[123, -123]; 128]);
        let queued = audio.stats().queued_frames;
        let pacing = worker.pacing;
        worker.apply(Command::SetTurbo(TurboConfig::new(0x8000, 10).unwrap()));
        worker.apply(Command::SetButtons(0x8020));
        assert_eq!(audio.stats().queued_frames, queued);
        assert_eq!(worker.pacing, pacing);
        assert_eq!(worker.machine.bus.sio.buttons(), 0x8020);
        worker.machine.bus.tick(CPU_CLOCK_HZ / 20).unwrap();
        worker.apply(Command::SetButtons(0x8020));
        assert_eq!(worker.machine.bus.sio.buttons(), 0x0020);
        worker.machine.bus.tick(CPU_CLOCK_HZ / 20).unwrap();
        worker.apply(Command::Slice);
        assert_eq!(worker.machine.bus.sio.buttons(), 0x8020);
        worker.apply(Command::Reset);
        assert_eq!(worker.machine.bus.sio.buttons(), 0);
        worker.apply(Command::SetButtons(0x8000));
        assert_eq!(worker.machine.bus.sio.buttons(), 0x8000);
        worker.machine.bus.tick(CPU_CLOCK_HZ / 20).unwrap();
        worker.apply(Command::Step);
        assert_eq!(worker.machine.bus.sio.buttons(), 0);
    }

    #[test]
    fn run_batches_pulse_and_pause_or_media_changes_release_input() {
        let mut machine =
            Machine::from_exe(&crate::systems::ps1::demo::executable(), "Turbo test").unwrap();
        // A tiny CPU loop advances time without the demo's expensive redraws.
        machine.bus.write32(0x8001_0000, 0x0800_4000).unwrap();
        machine.bus.write32(0x8001_0004, 0).unwrap();
        let mut worker = Worker::new(
            machine,
            true,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        worker.apply(Command::SetTurbo(TurboConfig::new(0x8000, 20).unwrap()));
        worker.apply(Command::SetButtons(0x8000));
        let mut previous = worker.machine.bus.sio.buttons();
        let mut edges = 0;
        for _ in 0..500 {
            worker.run_batch();
            assert!(worker.running);
            let buttons = worker.machine.bus.sio.buttons();
            edges += usize::from(previous != buttons);
            previous = buttons;
            if edges >= 6 {
                break;
            }
        }
        assert!(
            edges >= 6,
            "Expected repeat edges during execution, got {edges}"
        );
        worker.apply(Command::Toggle);
        assert!(!worker.running);
        worker.machine.bus.tick(CPU_CLOCK_HZ / 10).unwrap();
        worker.update_input();
        assert_eq!(worker.machine.bus.sio.buttons(), 0);
        for command in [
            Command::Demo,
            Command::Reset,
            Command::Load {
                path: PathBuf::from("missing-turbo-test.exe"),
                bios: false,
            },
            Command::RequestClose,
        ] {
            worker.apply(Command::SetButtons(0x8000));
            worker.apply(command);
            worker.machine.bus.tick(CPU_CLOCK_HZ / 10).unwrap();
            worker.update_input();
            assert_eq!(worker.machine.bus.sio.buttons(), 0);
        }
    }

    #[test]
    fn snapshot_mode_does_not_reset_execution_audio_or_notifications() {
        let audio = AudioBuffer::default();
        audio.set_playing(true);
        audio.push(&[[123, -123]; 2048]);
        let mut worker = Worker::new(
            Machine::demo(),
            true,
            CardStore::default(),
            Some(audio.clone()),
            SnapshotMode::Display,
        );
        worker.message = Some("Previous notification".into());
        let pacing = worker.pacing;
        let pc = worker.machine.cpu.pc;
        for mode in [SnapshotMode::Debug, SnapshotMode::Display] {
            let queued = audio.stats().queued_frames;
            let generated = worker.machine.bus.spu.generated_samples;
            worker
                .machine
                .bus
                .spu
                .tick(crate::systems::ps1::spu::SAMPLE_CLOCKS * 16)
                .unwrap();
            let new_samples = worker.machine.bus.spu.generated_samples - generated;
            assert!(new_samples > 0);
            assert!(!worker.apply(Command::SetSnapshotMode(mode)));
            worker.publish_audio();
            assert_eq!(worker.snapshot_mode, mode);
            assert!(worker.running);
            assert_eq!(worker.message.as_deref(), Some("Previous notification"));
            assert_eq!(worker.machine.cpu.pc, pc);
            assert_eq!(worker.pacing, pacing);
            assert_eq!(audio.stats().queued_frames, queued + new_samples as usize);
        }
    }
}

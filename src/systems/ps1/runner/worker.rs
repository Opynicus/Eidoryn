//! PlayStation session scheduling and transitions. No frontend dependencies.
use super::{Command, SharedSnapshot, Snapshot, SnapshotMode, pacing::FramePacing};
use crate::systems::ps1::{
    card_file::CardStore,
    cpu_engine::CpuMode,
    gpu::RendererMode,
    input::PadInput,
    machine::{INSTRUCTIONS_PER_SLICE, Machine},
    media::{load_disc, load_path},
};
use eidoryn_host::audio::AudioBuffer;
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
        let command = if worker.running {
            match receiver.recv_timeout(worker.pacing.wait_duration(Instant::now())) {
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
        let mut force_publish = command
            .as_ref()
            .is_some_and(|c| !matches!(c, Command::SetButtons(_) | Command::SetTurbo(_)));
        let mut close_ready = match command {
            Some(Command::Shutdown) => break,
            Some(command) => worker.apply(command),
            None => {
                force_publish = worker.run_batch();
                false
            }
        };
        if !view_only && !worker.flush_cards(!worker.running) {
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
    renderer_revision: u64,
    cpu_mode: CpuMode,
    running: bool,
    message: Option<String>,
    bios_path: Option<PathBuf>,
    pacing: FramePacing,
    last_publish: Instant,
    epoch: Instant,
    epoch_frames: u64,
    count: u32,
    rate: f64,
    video_fps: f64,
    store: CardStore,
    audio: Option<AudioBuffer>,
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
        Self {
            input,
            snapshot_mode,
            initial: machine.clone(),
            renderer: machine.bus.gpu.renderer_mode(),
            renderer_revision: 0,
            cpu_mode: machine.cpu_mode(),
            machine,
            running,
            message: None,
            bios_path: None,
            pacing,
            last_publish: now,
            epoch: now,
            epoch_frames: frames,
            count: 0,
            rate: 0.0,
            video_fps: 0.0,
            store,
            audio,
        }
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
        if let Err(error) = machine.bus.gpu.set_renderer(self.renderer) {
            self.message = Some(format!("Cannot start GPU renderer: {error}"));
            self.running = false;
            return false;
        }
        machine.bus.sio.cards = std::mem::take(&mut self.machine.bus.sio.cards);
        machine.bus.sio.deselect_cards();
        self.machine = machine;
        self.release_input();
        self.initial.clone_from(&self.machine);
        self.message = None;
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

    fn select_renderer(&mut self, mode: RendererMode) {
        match self.machine.bus.gpu.set_renderer(mode) {
            Ok(()) => {
                self.renderer = mode;
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
                if self.replace_machine(loaded) && bios {
                    self.bios_path = Some(path);
                }
                self.reset_pacing();
            }
            // A read/validation failure retains the previous pacing and statistics.
            Err(error) => self.message = Some(error),
        }
    }

    fn load_disc(&mut self, path: &Path, bios: Option<PathBuf>, fast_boot: bool) {
        self.running = false;
        self.release_input();
        match load_disc(path, bios.as_deref(), fast_boot) {
            Ok(loaded) => {
                if self.replace_machine(loaded) {
                    if bios.is_some() {
                        self.bios_path = bios;
                    }
                    self.running = fast_boot;
                }
            }
            Err(error) => self.message = Some(error),
        }
        // Unlike an image read failure, a failed disc load resets timing too.
        self.reset_pacing();
    }

    /// Returns true when the frontend should close after receiving the final snapshot.
    fn apply(&mut self, command: Command) -> bool {
        if matches!(command, Command::SetRenderer(_)) {
            // Also acknowledge a request rejected by the card-flush guard below.
            self.renderer_revision = self.renderer_revision.wrapping_add(1);
        }
        let view_only = matches!(command, Command::SetSnapshotMode(_));
        let input_only = matches!(command, Command::SetButtons(_) | Command::SetTurbo(_));
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
            Command::SetRenderer(mode) => self.select_renderer(mode),
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
                self.reset_pacing();
            }
            Command::FlushCards => self.message = None,
            Command::Load { path, bios } => self.load_image(path, bios),
            Command::LoadDisc {
                path,
                bios,
                fast_boot,
            } => self.load_disc(&path, bios, fast_boot),
            Command::RequestClose | Command::Shutdown => {
                self.running = false;
                self.release_input();
                return true;
            }
        }
        false
    }

    fn flush_cards(&mut self, force: bool) -> bool {
        let result = if force {
            self.store.flush(&self.machine.bus.sio.cards)
        } else {
            self.store.flush_if_due(&self.machine.bus.sio.cards)
        };
        if let Err(error) = result {
            self.message = Some(error);
            self.running = false;
            return false;
        }
        true
    }

    fn publish_audio(&mut self) {
        if let Some(audio) = &self.audio {
            audio.set_playing(self.running);
            audio.push(self.machine.bus.spu.drain_audio());
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
            audio.push([[123, -123]; 128]);
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
        audio.push([[123, -123]; 128]);
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
        audio.push([[123, -123]; 2048]);
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

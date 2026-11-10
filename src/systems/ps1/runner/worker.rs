//! PlayStation session scheduling and transitions. No frontend dependencies.
use super::{Command, SharedSnapshot, Snapshot};
use crate::systems::ps1::{
    card_file::CardStore,
    cpu_engine::CpuMode,
    gpu::RendererMode,
    machine::{INSTRUCTIONS_PER_SLICE, Machine},
    media::{load_disc, load_path},
};
use eidoryn_host::audio::AudioBuffer;
use std::{
    path::PathBuf,
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
) {
    let mut worker = Worker::new(machine, auto_run, store, audio);
    loop {
        let command = if worker.running {
            match receiver.recv_timeout(worker.deadline.saturating_duration_since(Instant::now())) {
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
        let mut force_publish = command
            .as_ref()
            .is_some_and(|c| !matches!(c, Command::SetButtons(_)));
        let mut close_ready = match command {
            Some(Command::Shutdown) => break,
            Some(command) => worker.apply(command),
            None => {
                force_publish = worker.run_batch();
                false
            }
        };
        if !worker.flush_cards(!worker.running) {
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
    machine: Machine,
    initial: Machine,
    renderer: RendererMode,
    cpu_mode: CpuMode,
    running: bool,
    message: Option<String>,
    bios_path: Option<PathBuf>,
    deadline: Instant,
    frame_deadline: Instant,
    next_frame: u64,
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
    ) -> Self {
        machine.bus.spu.drain_audio().for_each(drop);
        let now = Instant::now();
        let frames = machine.bus.gpu.frames;
        let frame_interval = Duration::from_secs_f64(1.0 / machine.bus.gpu.nominal_fps());
        Self {
            initial: machine.clone(),
            renderer: machine.bus.gpu.renderer_mode(),
            cpu_mode: machine.cpu_mode(),
            machine,
            running,
            message: None,
            bios_path: None,
            deadline: now,
            frame_deadline: now + frame_interval,
            next_frame: frames + 1,
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
        self.deadline = now;
        self.frame_deadline =
            now + Duration::from_secs_f64(1.0 / self.machine.bus.gpu.nominal_fps());
        self.next_frame = self.machine.bus.gpu.frames + 1;
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

    /// Returns true when the frontend should close after receiving the final snapshot.
    fn apply(&mut self, command: Command) -> bool {
        if !matches!(command, Command::SetButtons(_)) && !self.flush_cards(true) {
            return false;
        }
        if !matches!(command, Command::SetButtons(_) | Command::FlushCards) {
            if let Some(audio) = &self.audio {
                audio.clear();
            }
            self.machine.bus.spu.drain_audio().for_each(drop);
        }
        match command {
            Command::SetButtons(buttons) => self.machine.bus.sio.set_buttons(buttons),
            Command::SetCpuMode(mode) => self.select_cpu(mode),
            Command::SetRenderer(mode) => {
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
            Command::Toggle => {
                self.message = None;
                self.running = !self.running;
                if !self.running {
                    self.machine.bus.sio.set_buttons(0);
                }
                self.reset_pacing();
            }
            Command::Step => {
                self.running = false;
                self.message = self.machine.step().err().map(|e| e.to_string());
                self.machine.bus.gpu.latch_display();
            }
            Command::Slice => {
                self.running = false;
                self.message = self.machine.run_slice().err().map(|e| e.to_string());
                self.machine.bus.gpu.latch_display();
            }
            Command::Reset => {
                self.running = false;
                self.replace_machine(self.initial.clone());
                self.machine.bus.sio.set_buttons(0);
                self.reset_pacing();
            }
            Command::Demo => {
                self.running = self.replace_machine(Machine::demo());
                self.reset_pacing();
            }
            Command::FlushCards => self.message = None,
            Command::Load { path, bios } => {
                self.running = false;
                match load_path(&path, bios) {
                    Ok(loaded) => {
                        if self.replace_machine(loaded) && bios {
                            self.bios_path = Some(path);
                        }
                        self.reset_pacing();
                    }
                    Err(error) => self.message = Some(error),
                }
            }
            Command::LoadDisc {
                path,
                bios,
                fast_boot,
            } => {
                self.running = false;
                match load_disc(&path, bios.as_deref(), fast_boot) {
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
                self.reset_pacing();
            }
            Command::RequestClose | Command::Shutdown => {
                self.running = false;
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
            match self.machine.run_slice() {
                Ok(n) => {
                    self.count += 1;
                    // The built-in demo explicitly yields at a drawn frame.
                    if n < INSTRUCTIONS_PER_SLICE || self.machine.bus.gpu.frames >= self.next_frame
                    {
                        frame_finished = true;
                        break;
                    }
                }
                Err(error) => {
                    self.message = Some(error.to_string());
                    self.running = false;
                    break;
                }
            }
            if start.elapsed() >= HOST_BUDGET {
                break;
            }
        }
        let now = Instant::now();
        self.deadline = now;
        if frame_finished {
            // Preserve small scheduling jitter; discard a backlog longer than one frame.
            self.deadline = self.frame_deadline.max(now);
            let interval = Duration::from_secs_f64(1.0 / self.machine.bus.gpu.nominal_fps());
            self.frame_deadline = if now.saturating_duration_since(self.frame_deadline) > interval {
                now + interval
            } else {
                self.frame_deadline + interval
            };
            self.next_frame = self.machine.bus.gpu.frames + 1;
        }
        frame_finished || !self.running
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
        let mut snapshot = Snapshot::capture(
            &self.machine,
            self.running,
            self.message.clone(),
            if self.running { self.rate } else { 0.0 },
        );
        snapshot.bios_path.clone_from(&self.bios_path);
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

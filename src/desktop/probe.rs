//! Opt-in diagnostics for the `play_probe` example; normal windows do not record or inject input.
use eidoryn_psx::systems::ps1::{
    cpu_engine::CpuMode,
    runner::{Command, ExecutionState, Snapshot},
};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct ProbeOptions {
    pub seconds: u64,
    pub buttons: u16,
    pub path: PathBuf,
    /// Timed worker commands for memory probes; empty during input probes.
    pub commands: Vec<(Duration, Command)>,
}

pub(super) struct Probe {
    start: Instant,
    duration: Duration,
    buttons: u16,
    held: u16,
    closing: bool,
    rows: Vec<(u128, u64, &'static str)>,
    output: File,
    commands: VecDeque<(Duration, Command)>,
    report_status: bool,
    state: Option<ProbeState>,
    next_status: Duration,
}

#[derive(PartialEq, Eq)]
struct ProbeState {
    cpu: CpuMode,
    renderer_revision: u64,
    execution: ExecutionState,
    bios: bool,
    disc: bool,
    error: bool,
}

impl Probe {
    pub(super) fn new(options: ProbeOptions) -> Result<Self, String> {
        if !(1..=30).contains(&options.seconds) {
            return Err("Window probe requires 1..30 seconds".into());
        }
        let duration = Duration::from_secs(options.seconds);
        if options.commands.iter().any(|(at, _)| *at >= duration)
            || options
                .commands
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err("Probe commands must be ordered and precede its end".into());
        }
        Ok(Self {
            start: Instant::now(),
            duration,
            buttons: options.buttons,
            held: 0,
            closing: false,
            rows: Vec::with_capacity(16_384),
            output: File::create_new(options.path).map_err(|e| e.to_string())?,
            report_status: !options.commands.is_empty(),
            commands: options.commands.into(),
            state: None,
            next_status: Duration::ZERO,
        })
    }

    pub(super) fn input(&mut self) -> Option<Command> {
        let elapsed = self.start.elapsed();
        if elapsed >= self.duration {
            if !self.closing {
                self.closing = true;
                return Some(Command::RequestClose);
            }
            return None;
        }
        if self.commands.front().is_some_and(|(at, _)| elapsed >= *at) {
            let (_, command) = self.commands.pop_front().expect("checked front");
            return Some(command);
        }
        let held = if elapsed.as_millis() % 500 < 34 {
            self.buttons
        } else {
            0
        };
        if held == self.held {
            None
        } else {
            self.held = held;
            Some(Command::SetButtons(held))
        }
    }

    pub(super) fn receive(&mut self, snapshot: &Snapshot) {
        self.record(snapshot.video_frames, "receive");
        if !self.report_status {
            return;
        }
        let state = ProbeState {
            cpu: snapshot.cpu_mode,
            renderer_revision: snapshot.renderer_revision,
            execution: snapshot.execution_state(),
            bios: snapshot.bios_path.is_some(),
            disc: snapshot.disc_mounted,
            error: snapshot.message.is_some(),
        };
        let renderer_changed = self
            .state
            .as_ref()
            .is_none_or(|old| old.renderer_revision != state.renderer_revision);
        let elapsed = self.start.elapsed();
        if self.state.as_ref() != Some(&state) || elapsed >= self.next_status {
            self.next_status = elapsed + Duration::from_secs(1);
            self.state = Some(state);
            println!(
                "backend-probe seconds={:.3} cpu={:?} renderer={} revision={} bios={} disc={} running={} halted={} instructions={} frames={} blocks={} native={} error={:?}",
                elapsed.as_secs_f64(),
                snapshot.cpu_mode,
                snapshot.renderer.label(),
                snapshot.renderer_revision,
                snapshot.bios_path.is_some(),
                snapshot.disc_mounted,
                snapshot.running,
                snapshot.halted,
                snapshot.cpu.instructions,
                snapshot.video_frames,
                snapshot.cpu_stats.compiled_blocks,
                snapshot.cpu_stats.native_instructions,
                snapshot.message,
            );
        }
        if renderer_changed {
            println!(
                "renderer-probe revision={} renderer={} running={} instructions={} frames={} error={:?}",
                snapshot.renderer_revision,
                snapshot.renderer.label(),
                snapshot.running,
                snapshot.cpu.instructions,
                snapshot.video_frames,
                snapshot.message
            );
        }
    }

    pub(super) fn record(&mut self, frame: u64, kind: &'static str) {
        if self.rows.len() < self.rows.capacity() {
            self.rows
                .push((self.start.elapsed().as_micros(), frame, kind));
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let mut output = BufWriter::new(&self.output);
        let result = (|| {
            writeln!(output, "host_us,video_frame,kind")?;
            for (time, frame, kind) in &self.rows {
                writeln!(output, "{time},{frame},{kind}")?;
            }
            output.flush()
        })();
        if let Err(error) = result {
            eprintln!("Window probe trace: {error}");
        }
    }
}

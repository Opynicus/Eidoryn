//! Opt-in diagnostics for the `play_probe` example; normal windows do not record or inject input.
use eidoryn_psx::systems::ps1::{
    gpu::RendererMode,
    runner::{Command, Snapshot},
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
    /// Timed renderer requests for the memory probe; empty during input probes.
    pub renderer_changes: Vec<(Duration, RendererMode)>,
}

pub(super) struct Probe {
    start: Instant,
    duration: Duration,
    buttons: u16,
    held: u16,
    closing: bool,
    rows: Vec<(u128, u64, &'static str)>,
    output: File,
    renderer_changes: VecDeque<(Duration, RendererMode)>,
    report_renderers: bool,
    renderer_revision: Option<u64>,
}

impl Probe {
    pub(super) fn new(options: ProbeOptions) -> Result<Self, String> {
        if !(1..=30).contains(&options.seconds) {
            return Err("Window probe requires 1..30 seconds".into());
        }
        let duration = Duration::from_secs(options.seconds);
        if options
            .renderer_changes
            .iter()
            .any(|(at, _)| *at >= duration)
            || options
                .renderer_changes
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err("Renderer probe requests must be ordered and precede its end".into());
        }
        Ok(Self {
            start: Instant::now(),
            duration,
            buttons: options.buttons,
            held: 0,
            closing: false,
            rows: Vec::with_capacity(16_384),
            output: File::create_new(options.path).map_err(|e| e.to_string())?,
            report_renderers: !options.renderer_changes.is_empty(),
            renderer_changes: options.renderer_changes.into(),
            renderer_revision: None,
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
        if self
            .renderer_changes
            .front()
            .is_some_and(|(at, _)| elapsed >= *at)
        {
            let (_, mode) = self.renderer_changes.pop_front().expect("checked front");
            return Some(Command::SetRenderer(mode));
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
        if self.report_renderers && self.renderer_revision != Some(snapshot.renderer_revision) {
            self.renderer_revision = Some(snapshot.renderer_revision);
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

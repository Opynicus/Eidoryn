//! Interactive diagnostic session: guest input only, no BIOS/game patches.
//! Commands: `run STEPS HEX_MASK [FRAME.ppm]`, `until STEP HEX_MASK [FRAME.ppm]`,
//! `script PATH`, `frame PATH`, `record PATH.wav`, `stop-record`, `bench STEPS`,
//! `video SLICES PREFIX` (live/latched frame trace), `pace SECONDS` (host audio with the desktop feature), `flush`, `quit`.
//! `window-probe SECONDS HEX_MASK CSV` measures desktop delivery during repeated input.
//! `window` opens a desktop clone with in-memory cards; it leaves this session unchanged.
//! `renderer serial|threaded|vulkan`, `render-bench STEPS [MODE]`, `render-compare SLICES [MODE]`.
//! `cpu interpreter|jit`, `cpu-bench STEPS`, `cpu-compare SLICES`.
//! `frame-bench FRAMES HEX_MASK PREFIX` pulses a button twice per second on three independent clones.
//! `state`, `cd-trace PATH`, `mark`, `restore` inspect controller state and keep a temporary in-memory checkpoint.
#[path = "support/cpu.rs"]
mod cpu;
#[cfg(feature = "desktop")]
#[path = "../src/desktop/mod.rs"]
mod desktop;
#[path = "support/frames.rs"]
mod frames;
#[path = "support/renderer.rs"]
mod renderer;
#[path = "support/video.rs"]
mod video;

use eidoryn_psx::{
    audio::{AudioBuffer, wav::WavWriter},
    card_file::CardStore,
    machine::Machine,
    media::load_disc,
    runner::{Command, Runner},
};
use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::Instant,
};

struct Session {
    machine: Machine,
    store: CardStore,
    steps: u64,
    recording: Option<WavWriter>,
    cd_trace: Option<std::io::BufWriter<std::fs::File>>,
    checkpoint: Option<(Machine, u64, eidoryn_psx::gpu::RendererMode)>,
}

impl Session {
    fn clone_machine(&self) -> Result<Machine, String> {
        let mut sample = self.machine.clone();
        sample
            .bus
            .gpu
            .set_renderer(self.machine.bus.gpu.renderer_mode())
            .map_err(|e| e.to_string())?;
        Ok(sample)
    }

    fn run(&mut self, steps: u64, buttons: u16) -> Result<(), String> {
        self.machine.bus.sio.set_buttons(buttons);
        let start = Instant::now();
        let frames = self.machine.bus.gpu.frames;
        let result = (|| {
            let mut remaining = steps;
            while remaining > 0 {
                let chunk = remaining
                    .min(4096 - self.steps % 4096)
                    .min(5_000_000 - self.steps % 5_000_000);
                self.machine
                    .run_steps(chunk as usize)
                    .map_err(|e| e.to_string())?;
                self.steps += chunk;
                remaining -= chunk;
                if let Some(trace) = &mut self.cd_trace {
                    for event in self.machine.bus.cdrom.take_trace() {
                        writeln!(trace, "step<={} {event}", self.steps)
                            .map_err(|e| e.to_string())?;
                    }
                }
                if self.steps.is_multiple_of(4096)
                    && let Some(wav) = &mut self.recording
                {
                    wav.write(self.machine.bus.spu.drain_audio())
                        .map_err(|e| e.to_string())?;
                }
                if self.steps.is_multiple_of(5_000_000) {
                    self.machine.check_renderer().map_err(|e| e.to_string())?;
                    self.store.flush(&self.machine.bus.sio.cards)?;
                }
            }
            Ok(())
        })();
        self.store.flush(&self.machine.bus.sio.cards)?;
        if let Some(trace) = &mut self.cd_trace {
            trace.flush().map_err(|e| e.to_string())?;
        }
        if let Some(wav) = &mut self.recording {
            wav.write(self.machine.bus.spu.drain_audio())
                .map_err(|e| e.to_string())?;
        }
        println!(
            "steps={} PC={:08X} elapsed={:.3}s Msteps/s={:.2} video_FPS={:.2} card R/W={:?}",
            self.steps,
            self.machine.cpu.pc,
            start.elapsed().as_secs_f64(),
            steps as f64 / start.elapsed().as_secs_f64() / 1e6,
            (self.machine.bus.gpu.frames - frames) as f64 / start.elapsed().as_secs_f64(),
            self.machine
                .bus
                .sio
                .cards
                .each_ref()
                .map(|c| c.as_ref().map(|c| (c.reads, c.writes)))
        );
        result
    }

    fn frame(&self, path: &Path) -> Result<(), String> {
        let (w, h) = self.machine.bus.gpu.dimensions();
        let mut file = std::fs::File::create_new(path).map_err(|e| e.to_string())?;
        write!(file, "P6\n{w} {h}\n255\n")
            .and_then(|()| file.write_all(&self.machine.bus.gpu.display_rgb()))
            .map_err(|e| e.to_string())
    }

    fn video(&mut self, slices: u64, prefix: &Path) -> Result<(), String> {
        if self.recording.is_some() {
            return Err("Stop audio recording before tracing video".into());
        }
        video::trace(&mut self.machine, &mut self.steps, slices, prefix)?;
        self.store.flush(&self.machine.bus.sio.cards)
    }

    fn script(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut buttons = self.machine.bus.sio.buttons();
        for line in text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        {
            let mut fields = line.split_whitespace();
            let step: u64 = fields
                .next()
                .ok_or("Missing step")?
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let next = mask(fields.next())?;
            let count = step
                .checked_sub(self.steps)
                .ok_or("Input script is behind current session")?;
            self.run(count, buttons)?;
            buttons = next;
        }
        self.machine.bus.sio.set_buttons(buttons);
        Ok(())
    }

    fn bench(&self, steps: u32) -> Result<(), String> {
        let mut sample = self.machine.clone();
        let start = Instant::now();
        for _ in 0..steps {
            sample.step().map_err(|e| e.to_string())?;
        }
        println!(
            "full step: {:.2} ns/step",
            start.elapsed().as_secs_f64() * 1e9 / f64::from(steps)
        );
        let mut sample = self.machine.clone();
        let start = Instant::now();
        let mut completed = 0;
        for _ in 0..steps {
            if let Err(error) = sample.cpu.step(&mut sample.bus) {
                eprintln!("CPU-only sample stopped without device clocks: {error}");
                break;
            }
            completed += 1;
        }
        if completed != 0 {
            println!(
                "CPU only: {:.2} ns/step ({completed} steps)",
                start.elapsed().as_secs_f64() * 1e9 / f64::from(completed)
            );
        }
        let mut sample = self.machine.clone();
        let start = Instant::now();
        for _ in 0..steps {
            sample.bus.tick(1).map_err(|e| e.to_string())?;
        }
        println!(
            "bus tick: {:.2} ns/step",
            start.elapsed().as_secs_f64() * 1e9 / f64::from(steps)
        );
        let mut sample = self.machine.clone();
        let start = Instant::now();
        for _ in 0..steps {
            std::hint::black_box(sample.bus.gpu.tick(1, &mut sample.bus.timers));
        }
        println!(
            "GPU timing: {:.2} ns/step",
            start.elapsed().as_secs_f64() * 1e9 / f64::from(steps)
        );
        std::hint::black_box(sample);
        Ok(())
    }

    fn pace(&self, seconds: u64) -> Result<(), String> {
        use std::time::Duration;
        if !(1..=30).contains(&seconds) {
            return Err("Use 1..30 seconds".into());
        }
        let mut sample = self.clone_machine()?;
        sample.bus.sio.set_buttons(0);
        let frames = sample.bus.gpu.frames;
        let audio = AudioBuffer::default();
        #[cfg(feature = "desktop")]
        let _output = eidoryn_psx::audio::output::AudioOutput::open(audio.clone())?;
        let runner = Runner::with_outputs(sample, true, CardStore::default(), Some(audio.clone()))
            .map_err(|e| e.to_string())?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(seconds) {
            if let Some(snapshot) = runner.take_snapshot() {
                if let Some(error) = snapshot.message {
                    return Err(error);
                }
                std::hint::black_box(snapshot.rgb);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let pause = Instant::now();
        runner.send(Command::Toggle)?;
        loop {
            if let Some(snapshot) = runner.take_snapshot()
                && !snapshot.running
            {
                println!(
                    "paced video_FPS={:.2} target={:.2} pause_ms={:.2}",
                    (snapshot.video_frames - frames) as f64 / start.elapsed().as_secs_f64(),
                    snapshot.target_fps,
                    pause.elapsed().as_secs_f64() * 1000.0
                );
                let stats = audio.stats();
                println!(
                    "audio queue={} underruns={} dropped={} callback_misses={}",
                    stats.queued_frames,
                    stats.underruns,
                    stats.dropped_frames,
                    stats.callback_misses
                );
                return Ok(());
            }
            if pause.elapsed() > Duration::from_secs(5) {
                return Err("Worker did not pause".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[allow(clippy::too_many_lines)] // Keep diagnostic command dispatch together.
    fn command(&mut self, line: &str) -> Result<bool, String> {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("state") => {
                println!(
                    "steps={} CPU={:08x} R={:08x?}\nCD {}",
                    self.steps,
                    self.machine.cpu.pc,
                    self.machine.cpu.registers,
                    self.machine.bus.cdrom.debug_state()
                );
                for n in 0..12 {
                    let pc = self.machine.cpu.pc.wrapping_add(n * 4);
                    if let Some(word) = self.machine.bus.peek32(pc) {
                        println!("{pc:08x}: {}", eidoryn_psx::cpu::disassemble(pc, word));
                    }
                }
            }
            Some("cd-trace") => {
                let file = std::fs::File::create_new(fields.next().ok_or("Missing trace path")?)
                    .map_err(|e| e.to_string())?;
                self.cd_trace = Some(std::io::BufWriter::new(file));
                self.machine.bus.cdrom.enable_trace(true);
            }
            Some("mark") => {
                self.checkpoint = Some((
                    self.machine.clone(),
                    self.steps,
                    self.machine.bus.gpu.renderer_mode(),
                ));
                println!("Marked {} steps (in memory)", self.steps);
            }
            Some("restore") => {
                let (saved, steps, renderer) =
                    self.checkpoint.as_ref().ok_or("No in-memory mark")?;
                let mut restored = saved.clone();
                restored
                    .bus
                    .gpu
                    .set_renderer(*renderer)
                    .map_err(|e| e.to_string())?;
                restored.bus.cdrom.enable_trace(self.cd_trace.is_some());
                self.machine = restored;
                self.steps = *steps;
                println!("Restored {} steps (test card only)", self.steps);
            }
            Some("record") => {
                if self.recording.is_some() {
                    return Err("Already recording".into());
                }
                self.machine.bus.spu.drain_audio().for_each(drop);
                self.recording = Some(
                    WavWriter::create(Path::new(fields.next().ok_or("WAV path required")?))
                        .map_err(|e| e.to_string())?,
                );
            }
            Some("stop-record") => {
                if let Some(wav) = self.recording.take() {
                    wav.finish().map_err(|e| e.to_string())?;
                }
            }
            Some(command @ ("run" | "until")) => {
                let mut steps: u64 = fields
                    .next()
                    .ok_or("Missing step count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                if command == "until" {
                    steps = steps.checked_sub(self.steps).ok_or("Step already passed")?;
                }
                self.run(steps, mask(fields.next())?)?;
                if let Some(path) = fields.next() {
                    self.frame(Path::new(path))?;
                }
            }
            Some("script") => {
                self.script(Path::new(fields.next().ok_or("Missing input script")?))?;
            }
            Some("frame") => self.frame(Path::new(fields.next().ok_or("Missing frame path")?))?,
            Some("video") => self.video(
                fields
                    .next()
                    .ok_or("Missing slice count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?,
                Path::new(fields.next().ok_or("Missing output prefix")?),
            )?,
            #[cfg(feature = "desktop")]
            Some("window-probe") => {
                let seconds = fields
                    .next()
                    .ok_or("Missing seconds")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                let buttons = mask(fields.next())?;
                let path = fields.next().ok_or("Missing CSV path")?.into();
                desktop::run_with_probe(
                    self.clone_machine()?,
                    desktop::Options {
                        language: None,
                        muted: Some(false),
                        volume: None,
                        renderer: Some(self.machine.bus.gpu.renderer_mode()),
                        clean_fullscreen: None,
                        auto_run: true,
                        bios: None,
                        fullscreen: false,
                        game_view: false,
                        theme: None,
                        theme_plugin: None,
                        crt: Some(eidoryn_psx::video_filter::CrtFilter::Off),
                        graphics_open: false,
                    },
                    CardStore::default(),
                    Some(desktop::ProbeOptions {
                        seconds,
                        buttons,
                        path,
                    }),
                )?;
            }
            #[cfg(feature = "desktop")]
            Some("window") => {
                let crt = fields.next().unwrap_or("off").parse()?;
                let graphics_open = match fields.next() {
                    None => false,
                    Some("graphics") => true,
                    _ => return Err("Expected window [off|soft|classic] [graphics]".into()),
                };
                desktop::run(
                    self.clone_machine()?,
                    desktop::Options {
                        language: None,
                        muted: Some(false),
                        volume: None,
                        renderer: Some(self.machine.bus.gpu.renderer_mode()),
                        clean_fullscreen: None,
                        auto_run: true,
                        bios: None,
                        fullscreen: !graphics_open,
                        game_view: !graphics_open,
                        theme: None,
                        theme_plugin: None,
                        crt: Some(crt),
                        graphics_open,
                    },
                    CardStore::default(),
                )?;
            }
            Some("renderer") => {
                self.machine
                    .bus
                    .gpu
                    .set_renderer(fields.next().ok_or("Missing renderer mode")?.parse()?)
                    .map_err(|e| e.to_string())?;
                println!(
                    "Renderer: {} — {}",
                    self.machine.bus.gpu.renderer_mode().label(),
                    self.machine.bus.gpu.renderer_device()
                );
            }
            Some("cpu") => {
                self.machine
                    .set_cpu_mode(fields.next().ok_or("Missing CPU mode")?.parse()?)
                    .map_err(|e| e.to_string())?;
                println!("CPU: {}", self.machine.cpu_mode().label());
            }
            Some(command @ ("cpu-bench" | "cpu-compare")) => {
                let count = fields
                    .next()
                    .ok_or("Missing count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                if command == "cpu-bench" {
                    cpu::benchmark(&self.machine, count)?;
                } else {
                    cpu::compare(&self.machine, count)?;
                }
            }
            Some(command @ ("render-bench" | "render-compare")) => {
                let count = fields
                    .next()
                    .ok_or("Missing count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                if command == "render-bench" {
                    renderer::benchmark(
                        &self.machine,
                        count,
                        fields.next().unwrap_or("threaded").parse()?,
                    )?;
                } else {
                    renderer::compare(
                        &self.machine,
                        count,
                        fields.next().unwrap_or("threaded").parse()?,
                    )?;
                }
            }
            Some("bench") => self.bench(
                fields
                    .next()
                    .ok_or("Missing count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?,
            )?,
            Some("frame-bench") => frames::benchmark(
                &self.machine,
                fields
                    .next()
                    .ok_or("Missing frame count")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?,
                mask(fields.next())?,
                Path::new(fields.next().ok_or("Missing output prefix")?),
            )?,
            Some("pace") => self.pace(
                fields
                    .next()
                    .ok_or("Missing seconds")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?,
            )?,
            Some("flush") => self.store.flush(&self.machine.bus.sio.cards)?,
            Some("quit") => return Ok(false),
            None => {}
            _ => {
                return Err(
                    "Expected run/until/script/frame/record/stop-record/bench/frame-bench/render-bench/render-compare/renderer/cpu/cpu-bench/cpu-compare/pace/video/window/window-probe (desktop)/state/cd-trace/mark/restore/flush/quit"
                        .into(),
                );
            }
        }
        Ok(true)
    }
}

fn mask(text: Option<&str>) -> Result<u16, String> {
    u16::from_str_radix(
        text.ok_or("Missing hex button mask")?
            .trim_start_matches("0x"),
        16,
    )
    .map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let bios = PathBuf::from(args.next().ok_or("BIOS path required")?);
    let disc = PathBuf::from(args.next().ok_or("Disc path required")?);
    let card = PathBuf::from(args.next().ok_or("Test card path required")?);
    let (store, cards) = CardStore::open([Some(card), None])?;
    let mut machine = load_disc(&disc, Some(&bios), false)?;
    machine.bus.sio.cards = cards;
    let mut session = Session {
        machine,
        store,
        steps: 0,
        recording: None,
        cd_trace: None,
        checkpoint: None,
    };
    println!(
        "Ready. Commands: run/until STEPS HEX_MASK [FRAME], script PATH, frame PATH, record PATH.wav, stop-record, bench STEPS, frame-bench FRAMES HEX_MASK PREFIX, renderer serial|threaded|vulkan, render-bench STEPS [MODE], render-compare SLICES [MODE], cpu interpreter|jit, cpu-bench STEPS, cpu-compare SLICES, pace SECONDS (host audio with desktop), video SLICES PREFIX, window [off|soft|classic] [graphics] (desktop), window-probe SECONDS HEX_MASK CSV (desktop), state, cd-trace PATH, mark, restore, flush, quit"
    );
    for line in std::io::stdin().lock().lines() {
        match session.command(&line.map_err(|e| e.to_string())?) {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) => eprintln!("Diagnostic: {error}"),
        }
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
    session.store.flush(&session.machine.bus.sio.cards)?;
    if let Some(wav) = session.recording.take() {
        wav.finish().map_err(|e| e.to_string())?;
    }
    Ok(())
}

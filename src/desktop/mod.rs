#![allow(clippy::unreadable_literal)] // Material colors use six-digit RGB notation.
mod actions;
mod appearance;
mod audio_controls;
mod components;
mod graphics;
mod inspector;
mod probe;
mod video;
mod view;

use audio_controls::AudioControls;
use eidoryn_psx::{
    app,
    audio::{AudioBuffer, output::AudioOutput},
    card_file::CardStore,
    machine::Machine,
    runner::{Command, Runner, Snapshot},
    theme::ThemeCatalog,
};
use gpui::{
    App, Application, Bounds, FocusHandle, RenderImage, WindowBounds, WindowOptions, prelude::*,
    px, size,
};
pub use probe::ProbeOptions;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use video::{CrtState, frame_image};

#[allow(clippy::struct_excessive_bools)] // Independent startup preferences.
pub struct Options {
    pub muted: bool,
    pub auto_run: bool,
    pub bios: Option<std::path::PathBuf>,
    pub fullscreen: bool,
    pub game_view: bool,
    pub theme: Option<String>,
    pub theme_plugin: Option<std::path::PathBuf>,
    pub crt: eidoryn_psx::video_filter::CrtFilter,
    pub graphics_open: bool,
}

pub fn run(machine: Machine, options: Options, store: CardStore) -> Result<(), String> {
    run_with_probe(machine, options, store, None)
}

pub fn run_with_probe(
    machine: Machine,
    options: Options,
    store: CardStore,
    probe: Option<ProbeOptions>,
) -> Result<(), String> {
    let probe = probe.map(probe::Probe::new).transpose()?;
    let mut themes = ThemeCatalog::default();
    let errors = themes.discover(std::path::Path::new("plugins/themes"));
    if let Some(path) = &options.theme_plugin {
        let id = themes.load_plugin(path)?;
        themes.select(&id)?;
    }
    if let Some(id) = &options.theme {
        themes.select(id)?;
    }
    let mut notice = (!errors.is_empty()).then(|| errors.join("\n"));
    let audio_buffer = AudioBuffer::default();
    audio_buffer.set_volume(if options.muted { 0 } else { 75 });
    let audio_output = match AudioOutput::open(audio_buffer.clone()) {
        Ok(output) => Some(output),
        Err(error) => {
            notice = Some(error);
            None
        }
    };
    let mut snapshot = Snapshot::capture(&machine, options.auto_run, None, 0.0);
    snapshot.cards = store.status(&machine.bus.sio.cards);
    let runner = Runner::with_outputs(machine, options.auto_run, store, Some(audio_buffer.clone()))
        .map_err(|e| e.to_string())?;
    let audio = AudioControls {
        buffer: audio_buffer,
        output: audio_output,
        muted: options.muted,
        volume: 75,
        dragging: false,
    };
    open_window(runner, snapshot, options, themes, notice, audio, probe)
}

fn open_window(
    runner: Runner,
    snapshot: Snapshot,
    options: Options,
    themes: ThemeCatalog,
    notice: Option<String>,
    audio: AudioControls,
    probe: Option<probe::Probe>,
) -> Result<(), String> {
    let startup_error = Rc::new(RefCell::new(None));
    let error_slot = Rc::clone(&startup_error);
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(900.)), cx);
        let result = cx.open_window(
            WindowOptions {
                app_id: Some(app::APPLICATION_ID.into()),
                window_bounds: Some(if options.fullscreen {
                    WindowBounds::Fullscreen(bounds)
                } else {
                    WindowBounds::Windowed(bounds)
                }),
                window_min_size: Some(size(px(1080.), px(820.))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(app::NAME.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title(app::NAME);
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus);
                    let subscriptions = vec![
                        cx.observe_window_activation(window, |view: &mut Desktop, window, cx| {
                            if !window.is_window_active() {
                                view.audio.dragging = false;
                                view.release_buttons(cx);
                            }
                        }),
                        cx.on_blur(&focus, window, |view: &mut Desktop, _, cx| {
                            view.audio.dragging = false;
                            view.release_buttons(cx);
                        }),
                    ];
                    let image = frame_image(&snapshot);
                    let weak = cx.weak_entity();
                    window.on_window_should_close(cx, move |_, cx| {
                        weak.update(cx, |view: &mut Desktop, cx| {
                            view.command(Command::RequestClose, cx);
                        })
                        .is_err()
                    });
                    Desktop::start_snapshot_updates(cx);
                    Desktop {
                        graphics: graphics::GraphicsControls::new(
                            snapshot.renderer,
                            options.graphics_open,
                        ),
                        runner,
                        snapshot,
                        image,
                        icon: Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            include_bytes!("../../assets/eidoryn.png").to_vec(),
                        )),
                        crt: CrtState::new(options.crt),
                        last_redraw: Instant::now(),
                        focus,
                        notice,
                        audio,
                        bios: options.bios,
                        game_view: options.game_view,
                        themes,
                        appearance_open: false,
                        pressed_buttons: 0,
                        probe,
                        _subscriptions: subscriptions,
                    }
                })
            },
        );
        if let Err(error) = result {
            *error_slot.borrow_mut() = Some(format!("Cannot open GPUI window: {error}"));
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
    startup_error.borrow_mut().take().map_or(Ok(()), Err)
}

struct Desktop {
    runner: Runner,
    snapshot: Snapshot,
    image: Arc<RenderImage>,
    icon: Arc<gpui::Image>,
    crt: CrtState,
    last_redraw: Instant,
    focus: FocusHandle,
    notice: Option<String>,
    audio: AudioControls,
    bios: Option<std::path::PathBuf>,
    game_view: bool,
    pressed_buttons: u16,
    probe: Option<probe::Probe>,
    themes: ThemeCatalog,
    appearance_open: bool,
    graphics: graphics::GraphicsControls,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Desktop {
    fn start_snapshot_updates(cx: &mut gpui::Context<Self>) {
        cx.spawn(async |this, cx| {
            // Poll more often than one guest field. A relative 16 ms sleep plus
            // UI work drifts beyond NTSC's 16.72 ms field, so the latest-only
            // mailbox can skip an animation frame. Keep deadlines independent
            // of conversion/render work; unchanged images still do not redraw.
            let interval = Duration::from_millis(8);
            let mut deadline = Instant::now() + interval;
            loop {
                cx.background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now()))
                    .await;
                if this.update(cx, Self::receive_snapshot).is_err() {
                    break;
                }
                deadline += interval;
                let now = Instant::now();
                if deadline < now {
                    deadline = now + interval;
                }
            }
        })
        .detach();
    }

    fn receive_snapshot(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(command) = self.probe.as_mut().and_then(probe::Probe::input) {
            self.command(command, cx);
        }
        if let Some(error) = self.audio.output.as_ref().and_then(AudioOutput::take_error) {
            self.audio.output = None;
            self.audio.buffer.clear();
            self.notice = Some(error);
            cx.notify();
        }
        if let Some(snapshot) = self.runner.take_snapshot() {
            if let Some(probe) = &mut self.probe {
                probe.record(snapshot.video_frames, "receive");
            }
            self.graphics.receive(&snapshot);
            if snapshot.bios_path.is_some() {
                self.bios.clone_from(&snapshot.bios_path);
            }
            let frame_changed = !snapshot.has_same_frame(&self.snapshot);
            let redraw = frame_changed
                || !snapshot.running
                || snapshot.running != self.snapshot.running
                || snapshot.close_ready
                || self.last_redraw.elapsed() >= Duration::from_millis(100);
            if frame_changed {
                let image = frame_image(&snapshot);
                let old = std::mem::replace(&mut self.image, image);
                cx.drop_image(old, None);
            }
            self.snapshot = snapshot;
            // Static video only needs occasional inspector/status updates. Changed
            // video and paused/manual execution remain immediately visible.
            if redraw {
                self.last_redraw = Instant::now();
                cx.notify();
            }
        }
    }
}

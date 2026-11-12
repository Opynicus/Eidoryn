#![allow(clippy::unreadable_literal)] // Material colors use six-digit RGB notation.
mod actions;
mod appearance;
mod audio_controls;
mod components;
mod console;
mod fonts;
mod graphics;
mod icons;
mod probe;
mod ps1_inspector;
mod settings;
mod sidebar;
#[cfg(all(test, feature = "ui-tests"))]
mod tests;
mod video;
mod view;

use audio_controls::AudioControls;
use eidoryn_psx::{
    app,
    audio::{AudioBuffer, output::AudioOutput},
    systems::ps1::{
        card_file::CardStore,
        machine::Machine,
        runner::{Command, Runner, Snapshot, SnapshotMode},
    },
    theme::ThemeCatalog,
};
use gpui::{
    App, Application, Bounds, FocusHandle, RenderImage, WindowBounds, WindowOptions, prelude::*,
    px, size,
};
pub use probe::ProbeOptions;
use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use video::{CrtState, VideoImage};

#[allow(clippy::struct_excessive_bools)] // Independent startup preferences.
#[derive(Default)]
pub struct Options {
    pub debugger: bool,
    pub language: Option<eidoryn_psx::i18n::Language>,
    pub muted: Option<bool>,
    pub volume: Option<u8>,
    pub renderer: Option<eidoryn_psx::systems::ps1::gpu::RendererMode>,
    pub clean_fullscreen: Option<bool>,
    pub auto_run: bool,
    pub bios: Option<std::path::PathBuf>,
    pub fullscreen: bool,
    pub game_view: bool,
    pub theme: Option<String>,
    pub theme_plugin: Option<std::path::PathBuf>,
    pub crt: Option<eidoryn_psx::video_filter::CrtFilter>,
    pub graphics_open: bool,
}

pub fn run(machine: Machine, options: Options, store: CardStore) -> Result<(), String> {
    run_with_probe(machine, options, store, None)
}

pub fn run_with_probe(
    mut machine: Machine,
    options: Options,
    store: CardStore,
    probe: Option<ProbeOptions>,
) -> Result<(), String> {
    let probe = probe.map(probe::Probe::new).transpose()?;
    let mut startup = settings::load(&mut machine, &options)?;
    let audio_buffer = AudioBuffer::default();
    audio_buffer.set_volume(if startup.muted { 0 } else { startup.volume });
    let audio_output = match AudioOutput::open(audio_buffer.clone()) {
        Ok(output) => Some(output),
        Err(error) => {
            startup.notice = Some(error);
            None
        }
    };
    let mode = if options.debugger {
        SnapshotMode::Debug
    } else {
        SnapshotMode::Display
    };
    let mut snapshot = Snapshot::capture_with_mode(&machine, options.auto_run, None, 0.0, mode);
    snapshot.cards = store.status(&machine.bus.sio.cards);
    let runner = Runner::with_snapshot_mode(
        machine,
        options.auto_run,
        store,
        Some(audio_buffer.clone()),
        mode,
    )
    .map_err(|e| e.to_string())?;
    let audio = AudioControls {
        buffer: audio_buffer,
        output: audio_output,
        muted: startup.muted,
        volume: startup.volume,
        dragging: false,
    };
    open_window(runner, snapshot, options, startup, audio, probe)
}

#[allow(clippy::too_many_lines)] // Keep window setup and the Desktop initializer together.
fn open_window(
    runner: Runner,
    snapshot: Snapshot,
    options: Options,
    startup: settings::Startup,
    audio: AudioControls,
    probe: Option<probe::Probe>,
) -> Result<(), String> {
    let startup_error = Rc::new(RefCell::new(None));
    let error_slot = Rc::clone(&startup_error);
    Application::new()
        .with_assets(icons::Icons)
        .run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1280.), px(900.)), cx);
            let result = cx.open_window(
                WindowOptions {
                    app_id: Some(app::APPLICATION_ID.into()),
                    window_bounds: Some(if options.fullscreen {
                        WindowBounds::Fullscreen(bounds)
                    } else {
                        WindowBounds::Windowed(bounds)
                    }),
                    window_min_size: Some(size(px(980.), px(680.))),
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
                            cx.observe_window_activation(
                                window,
                                |view: &mut Desktop, window, cx| {
                                    if !window.is_window_active() {
                                        view.stop_input(cx);
                                    }
                                },
                            ),
                            cx.on_blur(&focus, window, |view: &mut Desktop, _, cx| {
                                view.stop_input(cx);
                            }),
                        ];
                        let weak = cx.weak_entity();
                        window.on_window_should_close(cx, move |_, cx| {
                            weak.update(cx, |view: &mut Desktop, cx| {
                                view.command(Command::RequestClose, cx);
                            })
                            .is_err()
                        });
                        Desktop::start_snapshot_updates(cx);
                        Desktop {
                            sidebar: None,
                            fonts: fonts::Fonts::new(Arc::clone(cx.text_system())),
                            graphics: graphics::GraphicsControls::new(
                                snapshot.renderer,
                                options.graphics_open,
                            ),
                            language: startup.language,
                            preferences: startup.preferences,
                            preferences_path: startup.path,
                            theme_paths: startup.theme_paths,
                            clean_fullscreen: startup.clean_fullscreen,
                            settings_focus: cx.focus_handle(),
                            runner,
                            snapshot_mode: snapshot.mode,
                            snapshot,
                            image: None,
                            icon: OnceCell::new(),
                            debugger_enabled: options.debugger,
                            crt: CrtState::new(startup.crt),
                            last_redraw: Instant::now(),
                            focus,
                            notice: startup.notice,
                            audio,
                            bios: options.bios,
                            game_view: options.game_view,
                            page: if options.debugger {
                                WorkspacePage::Debug
                            } else {
                                WorkspacePage::Console
                            },
                            themes: startup.themes,
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

#[allow(clippy::struct_excessive_bools)] // Independent view, debugger, and appearance preferences.
struct Desktop {
    sidebar: Option<gpui::Entity<sidebar::Sidebar>>,
    fonts: fonts::Fonts,
    language: eidoryn_psx::i18n::Language,
    preferences: eidoryn_psx::preferences::Preferences,
    preferences_path: Option<std::path::PathBuf>,
    theme_paths: std::collections::HashMap<String, std::path::PathBuf>,
    clean_fullscreen: bool,
    settings_focus: FocusHandle,
    runner: Runner,
    snapshot: Snapshot,
    image: Option<VideoImage>,
    icon: OnceCell<Option<Arc<RenderImage>>>,
    debugger_enabled: bool,
    snapshot_mode: SnapshotMode,
    crt: CrtState,
    last_redraw: Instant,
    focus: FocusHandle,
    notice: Option<String>,
    audio: AudioControls,
    bios: Option<std::path::PathBuf>,
    game_view: bool,
    page: WorkspacePage,
    pressed_buttons: u16,
    probe: Option<probe::Probe>,
    themes: ThemeCatalog,
    appearance_open: bool,
    graphics: graphics::GraphicsControls,
    _subscriptions: Vec<gpui::Subscription>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WorkspacePage {
    #[default]
    Console,
    Debug,
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
        if let Some(mut snapshot) = self.runner.take_snapshot() {
            // A queued debug snapshot can arrive after the user closes the view.
            if self.snapshot_mode == SnapshotMode::Display {
                snapshot.mode = SnapshotMode::Display;
                snapshot.disassembly = Vec::new();
            }
            if let Some(probe) = &mut self.probe {
                probe.record(snapshot.video_frames, "receive");
            }
            if let Some(mode) = self.graphics.receive(&snapshot) {
                self.preferences.renderer = mode.label().to_ascii_lowercase();
                self.save_preferences();
            }
            if snapshot.bios_path.is_some() {
                self.bios.clone_from(&snapshot.bios_path);
            }
            let frame_changed = !snapshot.has_same_frame(&self.snapshot);
            if frame_changed && let Some(image) = &mut self.image {
                // Coalesce pending frames without keeping a second RGB allocation.
                image.dirty = true;
            }
            let redraw = frame_changed
                || snapshot.mode != self.snapshot.mode
                || !snapshot.running
                || snapshot.running != self.snapshot.running
                || snapshot.close_ready
                || self.last_redraw.elapsed() >= Duration::from_millis(100);
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

impl Desktop {
    fn tr(&self, key: eidoryn_psx::i18n::Text) -> &'static str {
        self.language.text(key)
    }
    fn set_language(
        &mut self,
        language: eidoryn_psx::i18n::Language,
        cx: &mut gpui::Context<Self>,
    ) {
        self.language = language;
        self.preferences.language = Some(language.id().into());
        self.save_preferences();
        cx.notify();
    }

    fn renderer_label(&self) -> &'static str {
        use eidoryn_psx::i18n::Text;
        use eidoryn_psx::systems::ps1::gpu::RendererMode;
        self.tr(match self.snapshot.renderer {
            RendererMode::Serial => Text::SerialRenderer,
            RendererMode::Threaded => Text::ThreadedRenderer,
            RendererMode::Vulkan => Text::VulkanRenderer,
        })
    }

    fn display_title(&self) -> &str {
        if self.snapshot.title == "Built-in MIPS demo" {
            self.tr(eidoryn_psx::i18n::Text::BuiltinDemo)
        } else {
            &self.snapshot.title
        }
    }
}

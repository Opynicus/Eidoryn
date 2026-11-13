//! Argument parsing and compatibility checks; neither opens files nor starts emulation.
use eidoryn_psx::{
    app::{BINARY_NAME, NAME, VERSION},
    i18n::Language,
    systems::ps1::{cpu_engine::CpuMode, gpu::RendererMode},
    video_filter::CrtFilter,
};
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Default)]
pub(super) struct HeadlessOptions {
    pub frames: Option<u32>,
    pub dump: Option<PathBuf>,
    pub audio_dump: Option<PathBuf>,
}

#[allow(clippy::struct_excessive_bools)] // Independent CLI flags retain their existing semantics.
#[derive(Debug)]
pub(super) struct Options {
    pub source: Option<PathBuf>,
    pub bios_path: Option<PathBuf>,
    pub disc: Option<PathBuf>,
    pub fast_boot: bool,
    pub headless: bool,
    pub fullscreen: bool,
    pub game_view: bool,
    pub debugger: bool,
    pub muted: Option<bool>,
    pub volume: Option<u8>,
    pub clean_fullscreen: Option<bool>,
    pub theme: Option<String>,
    pub language: Option<Language>,
    pub theme_plugin: Option<PathBuf>,
    pub crt: Option<CrtFilter>,
    pub renderer: Option<RendererMode>,
    pub cpu: Option<CpuMode>,
    pub card_paths: [Option<PathBuf>; 2],
    pub custom_cards: bool,
    pub no_cards: bool,
    pub start_running: bool,
    pub execution: HeadlessOptions,
    pub export: Option<PathBuf>,
    pub inspect: Option<PathBuf>,
    pub export_disc_test: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            source: None,
            bios_path: None,
            disc: None,
            fast_boot: false,
            headless: false,
            fullscreen: false,
            game_view: false,
            debugger: false,
            muted: None,
            volume: None,
            clean_fullscreen: None,
            theme: None,
            language: None,
            theme_plugin: None,
            crt: None,
            renderer: None,
            cpu: None,
            card_paths: [
                Some(PathBuf::from("saves/card1.mcd")),
                Some(PathBuf::from("saves/card2.mcd")),
            ],
            custom_cards: false,
            no_cards: false,
            start_running: false,
            execution: HeadlessOptions::default(),
            export: None,
            inspect: None,
            export_disc_test: false,
        }
    }
}

/// `None` requests help immediately, without consuming or validating later arguments.
#[allow(clippy::too_many_lines)] // Keep the hand-written option parser and its error order explicit.
pub(super) fn parse(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Options>, String> {
    let mut args = arguments.into_iter();
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                return Ok(None);
            }
            Some("--inspect-disc") => {
                if options.inspect.is_some() {
                    return Err("Duplicate --inspect-disc".into());
                }
                options.inspect = Some(PathBuf::from(
                    args.next().ok_or("Missing disc inspection path")?,
                ));
            }
            Some("--exe") => {
                if options.source.is_some() {
                    return Err("Duplicate --exe".into());
                }
                options.source = Some(PathBuf::from(args.next().ok_or("Missing EXE path")?));
            }
            Some("--bios") => {
                if options.bios_path.is_some() {
                    return Err("Duplicate --bios".into());
                }
                options.bios_path = Some(PathBuf::from(args.next().ok_or("Missing BIOS path")?));
            }
            Some("--disc") => {
                if options.disc.is_some() {
                    return Err("Duplicate --disc".into());
                }
                options.disc = Some(PathBuf::from(args.next().ok_or("Missing disc path")?));
            }
            Some("--fast-boot") => options.fast_boot = true,
            Some("--headless") => options.headless = true,
            Some("--fullscreen") => options.fullscreen = true,
            Some("--game-view") => options.game_view = true,
            Some("--debugger") => options.debugger = true,
            Some("--mute" | "--unmute") => options.muted = Some(argument == "--mute"),
            Some("--clean-fullscreen" | "--fullscreen-controls") => {
                options.clean_fullscreen = Some(argument == "--clean-fullscreen");
            }
            Some("--volume") => {
                if options.volume.is_some() {
                    return Err("Duplicate --volume".into());
                }
                options.volume = Some(
                    args.next()
                        .ok_or("Missing --volume value")?
                        .to_str()
                        .and_then(|s| s.parse::<u8>().ok())
                        .filter(|v| *v <= 100)
                        .ok_or("--volume expects 0..100")?,
                );
            }
            Some("--cpu") => {
                if options.cpu.is_some() {
                    return Err("Duplicate --cpu".into());
                }
                options.cpu = Some(
                    args.next()
                        .ok_or("Missing --cpu mode: interpreter or jit")?
                        .to_str()
                        .ok_or("CPU mode must be UTF-8")?
                        .parse::<CpuMode>()?,
                );
            }
            Some("--renderer") => {
                if options.renderer.is_some() {
                    return Err("Duplicate --renderer".into());
                }
                options.renderer = Some(
                    args.next()
                        .ok_or("Missing --renderer mode: serial, threaded, or vulkan")?
                        .to_str()
                        .ok_or("Renderer mode must be valid UTF-8")?
                        .parse::<RendererMode>()?,
                );
            }
            Some("--crt") => {
                if options.crt.is_some() {
                    return Err("Duplicate --crt".into());
                }
                options.crt = Some(
                    args.next()
                        .ok_or("Missing --crt mode: off, soft, or classic")?
                        .to_str()
                        .ok_or("CRT mode must be valid UTF-8")?
                        .parse::<CrtFilter>()?,
                );
            }
            Some("--card1" | "--card2") => {
                let slot = usize::from(argument == "--card2");
                options.card_paths[slot] = Some(PathBuf::from(
                    args.next().ok_or("Missing memory card path")?,
                ));
                options.custom_cards = true;
            }
            Some("--no-memory-cards") => options.no_cards = true,
            Some("--language") => {
                if options.language.is_some() {
                    return Err("Choose --language once".into());
                }
                let value = args.next().ok_or("Missing --language value")?;
                options.language = Some(
                    value
                        .to_str()
                        .ok_or("Invalid language encoding")?
                        .parse::<Language>()?,
                );
            }
            Some("--theme") => {
                options.theme = Some(
                    args.next()
                        .ok_or("Missing theme id")?
                        .into_string()
                        .map_err(|_| "Theme id must be valid UTF-8")?,
                );
            }
            Some("--theme-plugin") => {
                options.theme_plugin = Some(PathBuf::from(
                    args.next().ok_or("Missing theme plugin path")?,
                ));
            }
            Some("--run") => options.start_running = true,
            Some("--frames") => {
                let value = args.next().ok_or("Missing --frames count")?;
                let count: u32 = value
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or("--frames must be a positive 32-bit integer")?;
                options.execution.frames = Some(count);
            }
            Some("--dump-frame") => {
                options.execution.dump =
                    Some(PathBuf::from(args.next().ok_or("Missing output path")?));
            }
            Some("--dump-audio") => {
                options.execution.audio_dump = Some(PathBuf::from(
                    args.next().ok_or("Missing audio output path")?,
                ));
            }
            Some("--export-demo" | "--export-disc-test") => {
                if options.export.is_some() {
                    return Err("Choose one export operation".into());
                }
                options.export_disc_test = argument == "--export-disc-test";
                options.export = Some(PathBuf::from(args.next().ok_or("Missing export path")?));
            }
            _ => {
                return Err(format!(
                    "Unknown argument: {}. Use --help.",
                    argument.to_string_lossy()
                ));
            }
        }
    }
    Ok(Some(options))
}

impl Options {
    pub fn auto_run(&self) -> bool {
        self.start_running
            || if self.disc.is_some() {
                self.fast_boot
            } else {
                self.source.is_none() && self.bios_path.is_none()
            }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.no_cards && self.custom_cards {
            return Err("--no-memory-cards cannot be combined with --card1/--card2".into());
        }
        if self.inspect.is_some() {
            if self.has_startup_options() || self.export.is_some() {
                return Err("--inspect-disc is a standalone operation".into());
            }
            return Ok(());
        }
        if self.export.is_some() {
            if self.has_startup_options() {
                return Err("EXE export is a standalone operation".into());
            }
            return Ok(());
        }
        if !self.headless
            && (self.execution.frames.is_some()
                || self.execution.dump.is_some()
                || self.execution.audio_dump.is_some())
        {
            return Err("--frames, --dump-frame and --dump-audio require --headless".into());
        }
        if self.headless && (self.fullscreen || self.game_view) {
            return Err("--fullscreen and --game-view require a desktop window".into());
        }
        if self.debugger && (self.headless || self.fullscreen || self.game_view) {
            return Err("--debugger requires a desktop workspace, without --headless, --fullscreen or --game-view".into());
        }
        if self.headless && self.crt.is_some() {
            return Err(
                "--crt requires a desktop window; frame dumps contain the original pixels".into(),
            );
        }
        if self.headless
            && (self.muted.is_some() || self.volume.is_some() || self.clean_fullscreen.is_some())
        {
            return Err("Audio and fullscreen preferences require a desktop window".into());
        }
        if self.headless && (self.theme.is_some() || self.theme_plugin.is_some()) {
            return Err("--theme and --theme-plugin require a desktop window".into());
        }
        if self.headless && self.language.is_some() {
            return Err("--language requires a desktop window".into());
        }
        if self.headless && self.start_running {
            return Err(
                "--run requires a desktop window; headless already runs automatically".into(),
            );
        }
        if self.fast_boot && self.disc.is_none() {
            return Err("--fast-boot requires --disc".into());
        }
        if self.source.is_some() && self.bios_path.is_some() {
            return Err("Choose only one of --exe and --bios".into());
        }
        if self.source.is_some() && self.disc.is_some() {
            return Err("Choose either --disc or --exe".into());
        }
        #[cfg(not(feature = "desktop"))]
        if !self.headless {
            return Err(
                "Desktop support is disabled. Use --headless or build with default features."
                    .into(),
            );
        }
        Ok(())
    }

    // --no-memory-cards is also valid for standalone inspection and export.
    fn has_startup_options(&self) -> bool {
        self.source.is_some()
            || self.crt.is_some()
            || self.renderer.is_some()
            || self.cpu.is_some()
            || self.muted.is_some()
            || self.volume.is_some()
            || self.clean_fullscreen.is_some()
            || self.execution.audio_dump.is_some()
            || self.custom_cards
            || self.bios_path.is_some()
            || self.disc.is_some()
            || self.fast_boot
            || self.headless
            || self.fullscreen
            || self.game_view
            || self.debugger
            || self.language.is_some()
            || self.theme.is_some()
            || self.theme_plugin.is_some()
            || self.start_running
            || self.execution.frames.is_some()
            || self.execution.dump.is_some()
    }
}

pub(super) fn print_help() {
    println!(
        "{NAME} {VERSION}\n\n\
        Usage: {BINARY_NAME} [--exe PATH | --bios PATH] [--headless]\n\
               {BINARY_NAME} --disc GAME.iso [--bios BIOS | --fast-boot]\n\n\
        --headless          Run without a GPUI window\n\
        --fullscreen        Open the native window in fullscreen\n\
        --game-view         Enlarge the display by hiding inspectors\n\
        --debugger          Enable and open the optional debugger\n\
        --mute / --unmute   Override saved audio mute state\n\
        --volume N          Output volume: 0..100\n\
        --clean-fullscreen  Hide bars in fullscreen (F12 toggles)\n\
        --fullscreen-controls Show bars in fullscreen\n\
        --crt MODE          CRT display: off, soft, classic (overrides saved choice)\n\
        --renderer MODE     GPU: serial, threaded, vulkan (overrides saved choice)\n\
        --cpu MODE          CPU: interpreter (default), jit (experimental)\n\
        --language LANG     Interface language: en or zh-CN\n\
        --theme ID          Start with a built-in or discovered theme\n\
        --theme-plugin PATH Load and select a theme JSON plugin\n\
        --card1 PATH        Slot 1 card (default saves/card1.mcd)\n\
        --card2 PATH        Slot 2 card (default saves/card2.mcd)\n\
        --no-memory-cards   Disconnect both card slots\n\
        --run               Start the loaded BIOS/EXE running in the window\n\
        --frames N          Host slices (not VBlank frames), default 120\n\
        --dump-frame PATH   Write final display as PPM; never overwrite\n\
        --dump-audio PATH   Record headless stereo PCM to WAV; never overwrite\n\
        --export-demo PATH  Export original demo PS-X EXE and exit\n\
        --export-disc-test PATH  Export original CD-ROM/DMA3 test guest\n\
        --inspect-disc PATH Inspect tracks and boot EXE without executing\n\
        --exe PATH          BIOS-independent PS-X EXE\n\
        --bios PATH         512 KiB BIOS (provide your own)\n\
        --disc PATH         CUE/BIN or ISO; requires --bios or --fast-boot\n\
        --fast-boot         Load EXE from disc; skips BIOS, homebrew only\n\n\
        Keys: F5 = run/pause; F9 = slice; F10 = instruction; Ctrl+R = reset.\n\
        F6 = game/inspector view; F7 = cycle CRT; F8 = graphics settings.\n\
        F11 = fullscreen; Esc = leave fullscreen.\n\
        Pad: arrows; X/Z/S/D = cross/square/triangle/circle; Enter = start.\n\
        No games or proprietary BIOS files are bundled."
    );
}

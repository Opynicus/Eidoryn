//! CLI parsing and validation. Loading media and opening devices happen only after validation.
mod operations;
use eidoryn_psx::{
    app::{BINARY_NAME, NAME, VERSION},
    systems::ps1::{cpu_engine::CpuMode, gpu::RendererMode},
};
pub(super) use operations::{export_demo, inspect_disc, run_headless};
use std::{ffi::OsString, path::PathBuf};

#[allow(clippy::struct_excessive_bools)] // Independent CLI flags, not an execution state machine.
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
    pub language: Option<eidoryn_psx::i18n::Language>,
    pub theme_plugin: Option<PathBuf>,
    pub crt: Option<eidoryn_psx::video_filter::CrtFilter>,
    pub renderer: Option<RendererMode>,
    pub cpu: Option<CpuMode>,
    pub card_paths: [Option<PathBuf>; 2],
    pub start_running: bool,
    pub frames: Option<u32>,
    pub dump: Option<PathBuf>,
    pub audio_dump: Option<PathBuf>,
    pub export: Option<PathBuf>,
    pub inspect: Option<PathBuf>,
    pub export_disc_test: bool,
}

/// None means --help was reached; later arguments and conflicts are intentionally ignored.
#[allow(clippy::too_many_lines)] // Keep each flag and its existing duplicate/value diagnostics together.
pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut source = None;
    let mut bios_path = None;
    let mut disc = None;
    let mut fast_boot = false;
    let mut headless = false;
    let mut fullscreen = false;
    let mut game_view = false;
    let mut debugger = false;
    let mut muted = None;
    let mut volume = None;
    let mut clean_fullscreen = None;
    let mut theme = None;
    let mut language = None;
    let mut theme_plugin = None;
    let mut crt = None;
    let mut renderer = None;
    let mut cpu = None;
    let mut card_paths = [
        Some(PathBuf::from("saves/card1.mcd")),
        Some(PathBuf::from("saves/card2.mcd")),
    ];
    let mut custom_cards = false;
    let mut no_cards = false;
    let mut start_running = false;
    let mut frames = None;
    let mut dump = None;
    let mut audio_dump = None;
    let mut export = None;
    let mut inspect = None;
    let mut export_disc_test = false;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--help" | "-h") => {
                return Ok(None);
            }
            Some("--inspect-disc") => {
                if inspect.is_some() {
                    return Err("Duplicate --inspect-disc".into());
                }
                inspect = Some(PathBuf::from(
                    args.next().ok_or("Missing disc inspection path")?,
                ));
            }
            Some("--exe") => {
                if source.is_some() {
                    return Err("Duplicate --exe".into());
                }
                source = Some(PathBuf::from(args.next().ok_or("Missing EXE path")?));
            }
            Some("--bios") => {
                if bios_path.is_some() {
                    return Err("Duplicate --bios".into());
                }
                bios_path = Some(PathBuf::from(args.next().ok_or("Missing BIOS path")?));
            }
            Some("--disc") => {
                if disc.is_some() {
                    return Err("Duplicate --disc".into());
                }
                disc = Some(PathBuf::from(args.next().ok_or("Missing disc path")?));
            }
            Some("--fast-boot") => fast_boot = true,
            Some("--headless") => headless = true,
            Some("--fullscreen") => fullscreen = true,
            Some("--game-view") => game_view = true,
            Some("--debugger") => debugger = true,
            Some("--mute" | "--unmute") => muted = Some(argument == "--mute"),
            Some("--clean-fullscreen" | "--fullscreen-controls") => {
                clean_fullscreen = Some(argument == "--clean-fullscreen");
            }
            Some("--volume") => {
                if volume.is_some() {
                    return Err("Duplicate --volume".into());
                }
                volume = Some(
                    args.next()
                        .ok_or("Missing --volume value")?
                        .to_str()
                        .and_then(|s| s.parse::<u8>().ok())
                        .filter(|v| *v <= 100)
                        .ok_or("--volume expects 0..100")?,
                );
            }
            Some("--cpu") => {
                if cpu.is_some() {
                    return Err("Duplicate --cpu".into());
                }
                cpu = Some(
                    args.next()
                        .ok_or("Missing --cpu mode: interpreter or jit")?
                        .to_str()
                        .ok_or("CPU mode must be UTF-8")?
                        .parse::<CpuMode>()?,
                );
            }
            Some("--renderer") => {
                if renderer.is_some() {
                    return Err("Duplicate --renderer".into());
                }
                renderer = Some(
                    args.next()
                        .ok_or("Missing --renderer mode: serial, threaded, or vulkan")?
                        .to_str()
                        .ok_or("Renderer mode must be valid UTF-8")?
                        .parse::<RendererMode>()?,
                );
            }
            Some("--crt") => {
                if crt.is_some() {
                    return Err("Duplicate --crt".into());
                }
                crt = Some(
                    args.next()
                        .ok_or("Missing --crt mode: off, soft, or classic")?
                        .to_str()
                        .ok_or("CRT mode must be valid UTF-8")?
                        .parse::<eidoryn_psx::video_filter::CrtFilter>()?,
                );
            }
            Some("--card1" | "--card2") => {
                let slot = usize::from(argument == "--card2");
                card_paths[slot] = Some(PathBuf::from(
                    args.next().ok_or("Missing memory card path")?,
                ));
                custom_cards = true;
            }
            Some("--no-memory-cards") => no_cards = true,
            Some("--language") => {
                if language.is_some() {
                    return Err("Choose --language once".into());
                }
                let value = args.next().ok_or("Missing --language value")?;
                language = Some(
                    value
                        .to_str()
                        .ok_or("Invalid language encoding")?
                        .parse::<eidoryn_psx::i18n::Language>()?,
                );
            }
            Some("--theme") => {
                theme = Some(
                    args.next()
                        .ok_or("Missing theme id")?
                        .into_string()
                        .map_err(|_| "Theme id must be valid UTF-8")?,
                );
            }
            Some("--theme-plugin") => {
                theme_plugin = Some(PathBuf::from(
                    args.next().ok_or("Missing theme plugin path")?,
                ));
            }
            Some("--run") => start_running = true,
            Some("--frames") => {
                let value = args.next().ok_or("Missing --frames count")?;
                let count: u32 = value
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or("--frames must be a positive 32-bit integer")?;
                frames = Some(count);
            }
            Some("--dump-frame") => {
                dump = Some(PathBuf::from(args.next().ok_or("Missing output path")?));
            }
            Some("--dump-audio") => {
                audio_dump = Some(PathBuf::from(
                    args.next().ok_or("Missing audio output path")?,
                ));
            }
            Some("--export-demo" | "--export-disc-test") => {
                if export.is_some() {
                    return Err("Choose one export operation".into());
                }
                export_disc_test = argument == "--export-disc-test";
                export = Some(PathBuf::from(args.next().ok_or("Missing export path")?));
            }
            _ => {
                return Err(format!(
                    "Unknown argument: {}. Use --help.",
                    argument.to_string_lossy()
                ));
            }
        }
    }
    if no_cards && custom_cards {
        return Err("--no-memory-cards cannot be combined with --card1/--card2".into());
    }
    if no_cards {
        card_paths = [None, None];
    }
    let options = Options {
        source,
        bios_path,
        disc,
        fast_boot,
        headless,
        fullscreen,
        game_view,
        debugger,
        muted,
        volume,
        clean_fullscreen,
        theme,
        language,
        theme_plugin,
        crt,
        renderer,
        cpu,
        card_paths,
        start_running,
        frames,
        dump,
        audio_dump,
        export,
        inspect,
        export_disc_test,
    };
    options.validate(custom_cards)?;
    Ok(Some(options))
}

impl Options {
    fn validate(&self, custom_cards: bool) -> Result<(), String> {
        let session_options = self.source.is_some()
            || self.crt.is_some()
            || self.renderer.is_some()
            || self.cpu.is_some()
            || self.muted.is_some()
            || self.volume.is_some()
            || self.clean_fullscreen.is_some()
            || self.audio_dump.is_some()
            || custom_cards
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
            || self.frames.is_some()
            || self.dump.is_some();
        if self.inspect.is_some() {
            if session_options || self.export.is_some() {
                return Err("--inspect-disc is a standalone operation".into());
            }
            return Ok(());
        }
        if self.export.is_some() {
            if session_options {
                return Err("EXE export is a standalone operation".into());
            }
            return Ok(());
        }
        if !self.headless
            && (self.frames.is_some() || self.dump.is_some() || self.audio_dump.is_some())
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

//! Command-line syntax and compatibility checks; parsing performs no file or device I/O.
use eidoryn_psx::systems::ps1::{cpu_engine::CpuMode, gpu::RendererMode};
use std::{ffi::OsString, path::PathBuf};

// Parsed only once at startup; boxing the session would add an unnecessary allocation.
#[allow(clippy::large_enum_variant)]
pub(super) enum Invocation {
    Help,
    Inspect(PathBuf),
    Export { path: PathBuf, disc_test: bool },
    Run(SessionOptions),
}

#[allow(clippy::struct_excessive_bools)] // Independent CLI switches, not runtime state.
pub(super) struct SessionOptions {
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
    pub language: Option<eidoryn_psx::i18n::Language>,
    pub theme: Option<String>,
    pub theme_plugin: Option<PathBuf>,
    pub crt: Option<eidoryn_psx::video_filter::CrtFilter>,
    pub renderer: Option<RendererMode>,
    pub cpu: Option<CpuMode>,
    pub card_paths: [Option<PathBuf>; 2],
    pub auto_run: bool,
    pub frames: Option<u32>,
    pub dump: Option<PathBuf>,
    pub audio_dump: Option<PathBuf>,
}

#[allow(clippy::too_many_lines)] // Explicit flag grammar and ordered compatibility checks.
pub(super) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut args = args.into_iter();
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
                return Ok(Invocation::Help);
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
    let has_session_options = source.is_some()
        || crt.is_some()
        || renderer.is_some()
        || cpu.is_some()
        || muted.is_some()
        || volume.is_some()
        || clean_fullscreen.is_some()
        || audio_dump.is_some()
        || custom_cards
        || bios_path.is_some()
        || disc.is_some()
        || fast_boot
        || headless
        || fullscreen
        || game_view
        || debugger
        || language.is_some()
        || theme.is_some()
        || theme_plugin.is_some()
        || start_running
        || frames.is_some()
        || dump.is_some();
    if let Some(path) = inspect {
        if has_session_options || export.is_some() {
            return Err("--inspect-disc is a standalone operation".into());
        }
        return Ok(Invocation::Inspect(path));
    }
    if let Some(path) = export {
        if has_session_options {
            return Err("EXE export is a standalone operation".into());
        }
        return Ok(Invocation::Export {
            path,
            disc_test: export_disc_test,
        });
    }
    if !headless && (frames.is_some() || dump.is_some() || audio_dump.is_some()) {
        return Err("--frames, --dump-frame and --dump-audio require --headless".into());
    }
    if headless && (fullscreen || game_view) {
        return Err("--fullscreen and --game-view require a desktop window".into());
    }
    if debugger && (headless || fullscreen || game_view) {
        return Err("--debugger requires a desktop workspace, without --headless, --fullscreen or --game-view".into());
    }
    if headless && crt.is_some() {
        return Err(
            "--crt requires a desktop window; frame dumps contain the original pixels".into(),
        );
    }
    if headless && (muted.is_some() || volume.is_some() || clean_fullscreen.is_some()) {
        return Err("Audio and fullscreen preferences require a desktop window".into());
    }
    if headless && (theme.is_some() || theme_plugin.is_some()) {
        return Err("--theme and --theme-plugin require a desktop window".into());
    }
    if headless && language.is_some() {
        return Err("--language requires a desktop window".into());
    }
    if headless && start_running {
        return Err("--run requires a desktop window; headless already runs automatically".into());
    }
    if fast_boot && disc.is_none() {
        return Err("--fast-boot requires --disc".into());
    }
    if source.is_some() && bios_path.is_some() {
        return Err("Choose only one of --exe and --bios".into());
    }
    let auto_run = source.is_none() && bios_path.is_none();
    if source.is_some() && disc.is_some() {
        return Err("Choose either --disc or --exe".into());
    }
    #[cfg(not(feature = "desktop"))]
    if !headless {
        return Err(
            "Desktop support is disabled. Use --headless or build with default features.".into(),
        );
    }
    let auto_run = start_running || if disc.is_some() { fast_boot } else { auto_run };
    Ok(Invocation::Run(SessionOptions {
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
        language,
        theme,
        theme_plugin,
        crt,
        renderer,
        cpu,
        card_paths,
        auto_run,
        frames,
        dump,
        audio_dump,
    }))
}

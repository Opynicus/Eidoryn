use eidoryn_psx::{
    app::{BINARY_NAME, NAME, VERSION},
    card_file::CardStore,
    cpu_engine::CpuMode,
    demo,
    gpu::RendererMode,
    machine::Machine,
    media::{load_disc, load_path},
};
use std::{io::Write, path::PathBuf};

#[cfg(feature = "desktop")]
mod desktop;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{BINARY_NAME}: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)] // Small command parser and its dispatch stay in one place.
fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let mut source = None;
    let mut bios_path = None;
    let mut disc = None;
    let mut fast_boot = false;
    let mut headless = false;
    let mut fullscreen = false;
    let mut game_view = false;
    let mut muted = false;
    let mut theme = None;
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
                println!(
                    "{NAME} {VERSION}\n\n\
                    Usage: {BINARY_NAME} [--exe PATH | --bios PATH] [--headless]\n\
                           {BINARY_NAME} --disc GAME.iso [--bios BIOS | --fast-boot]\n\n\
                    --headless          Run without a GPUI window\n\
                    --fullscreen        Open the native window in fullscreen\n\
                    --game-view         Enlarge the display by hiding inspectors\n\
                    --mute              Start the window with audio muted\n\
                    --crt MODE          CRT display: off (default), soft, classic\n\
                    --renderer MODE     GPU: serial (default), threaded, vulkan\n\
                    --cpu MODE          CPU: interpreter (default), jit (experimental)\n\
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
                    Keys: Space = run/pause; F9 = slice; F10 = instruction; Ctrl+R = reset.\n\
                    F6 = game/inspector view; F7 = cycle CRT; F8 = graphics settings.\n\
                    F11 = fullscreen; Esc = leave fullscreen.\n\
                    Pad: arrows; X/Z/S/D = cross/square/triangle/circle; Enter = start.\n\
                    No games or proprietary BIOS files are bundled."
                );
                return Ok(());
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
            Some("--mute") => muted = true,
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
    if let Some(path) = inspect {
        if source.is_some()
            || crt.is_some()
            || renderer.is_some()
            || cpu.is_some()
            || muted
            || audio_dump.is_some()
            || custom_cards
            || bios_path.is_some()
            || disc.is_some()
            || fast_boot
            || headless
            || fullscreen
            || game_view
            || theme.is_some()
            || theme_plugin.is_some()
            || start_running
            || frames.is_some()
            || dump.is_some()
            || export.is_some()
        {
            return Err("--inspect-disc is a standalone operation".into());
        }
        let disc = eidoryn_psx::disc::DiscImage::open(&path).map_err(|e| e.to_string())?;
        println!(
            "Disc: {} | tracks={} sectors={}",
            path.display(),
            disc.tracks().count(),
            disc.sectors
        );
        for track in disc.tracks() {
            for lba in [track.index0_lba, track.start_lba, track.end_lba - 1] {
                disc.read_sector(lba).map_err(|e| e.to_string())?;
            }
            println!(
                "Track {:02}: {:?} | INDEX00 LBA={} INDEX01 LBA={} end LBA={} | boundary reads OK",
                track.number, track.kind, track.index0_lba, track.start_lba, track.end_lba
            );
        }
        if let Ok(config) = disc.read_file("SYSTEM.CNF;1", 4096) {
            println!(
                "SYSTEM.CNF:\n{}",
                String::from_utf8_lossy(&config).trim_end()
            );
        }
        let bytes = disc.boot_executable().map_err(|e| e.to_string())?;
        let machine = Machine::from_exe(&bytes, "Disc inspection").map_err(|e| e.to_string())?;
        println!(
            "Valid PS-X EXE: {} bytes | entry={:08X} SP={:08X}",
            bytes.len(),
            machine.cpu.pc,
            machine.cpu.registers[29]
        );
        return Ok(());
    }
    if let Some(path) = export {
        if source.is_some()
            || crt.is_some()
            || renderer.is_some()
            || cpu.is_some()
            || muted
            || audio_dump.is_some()
            || custom_cards
            || bios_path.is_some()
            || disc.is_some()
            || fast_boot
            || headless
            || fullscreen
            || game_view
            || theme.is_some()
            || theme_plugin.is_some()
            || start_running
            || frames.is_some()
            || dump.is_some()
        {
            return Err("EXE export is a standalone operation".into());
        }
        let mut file =
            std::fs::File::create_new(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(&if export_disc_test {
            demo::disc_probe_executable()
        } else {
            demo::executable()
        })
        .map_err(|e| e.to_string())?;
        println!("Exported MIPS demo to {}", path.display());
        return Ok(());
    }
    if !headless && (frames.is_some() || dump.is_some() || audio_dump.is_some()) {
        return Err("--frames, --dump-frame and --dump-audio require --headless".into());
    }
    if headless && (fullscreen || game_view) {
        return Err("--fullscreen and --game-view require a desktop window".into());
    }
    if headless && crt.is_some() {
        return Err(
            "--crt requires a desktop window; frame dumps contain the original pixels".into(),
        );
    }
    if headless && muted {
        return Err(
            "--mute requires a desktop window; headless records audio without playing it".into(),
        );
    }
    if headless && (theme.is_some() || theme_plugin.is_some()) {
        return Err("--theme and --theme-plugin require a desktop window".into());
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
    let mut machine = if let Some(path) = disc {
        load_disc(&path, bios_path.as_deref(), fast_boot)?
    } else if let Some(path) = source {
        load_path(&path, false)?
    } else if let Some(path) = &bios_path {
        load_path(path, true)?
    } else {
        Machine::demo()
    };
    machine
        .bus
        .gpu
        .set_renderer(renderer.unwrap_or_default())
        .map_err(|error| format!("Cannot start GPU renderer: {error}"))?;
    if machine.bus.gpu.renderer_mode() == eidoryn_psx::gpu::RendererMode::Vulkan {
        eprintln!("Renderer: {}", machine.bus.gpu.renderer_device());
    }
    machine
        .set_cpu_mode(cpu.unwrap_or_default())
        .map_err(|error| format!("Cannot start CPU engine: {error}"))?;
    let (mut card_store, cards) = CardStore::open(card_paths)?;
    machine.bus.sio.cards = cards;
    if headless {
        let mut wav = audio_dump
            .as_deref()
            .map(eidoryn_psx::audio::wav::WavWriter::create)
            .transpose()
            .map_err(|e| format!("Audio export: {e}"))?;
        let mut failure = None;
        let mut audio_failure = None;
        for _ in 0..frames.unwrap_or(120) {
            if let Err(error) = machine.run_slice() {
                failure = Some(error);
                break;
            }
            if let Some(wav) = &mut wav
                && let Err(error) = wav.write(machine.bus.spu.drain_audio())
            {
                audio_failure = Some(format!("Audio export: {error}"));
                break;
            }
            card_store.flush_if_due(&machine.bus.sio.cards)?;
        }
        card_store.flush(&machine.bus.sio.cards)?;
        if let Some(error) = audio_failure {
            return Err(error);
        }
        if let Some(mut wav) = wav {
            wav.write(machine.bus.spu.drain_audio())
                .map_err(|e| e.to_string())?;
            wav.finish().map_err(|e| e.to_string())?;
        }
        if let Some(path) = dump {
            write_frame(
                &path,
                machine.bus.gpu.dimensions(),
                &machine.bus.gpu.display_rgb(),
            )?;
        }
        println!(
            "{} | self-built slices={} instructions={} GP0 commands={} CD sectors={} PC={:08X}",
            machine.title,
            machine.slices,
            machine.cpu.instructions,
            machine.bus.gpu.commands,
            machine.bus.cdrom.sectors_read,
            machine.cpu.pc
        );
        if let Some(error) = failure {
            return Err(format!(
                "{error}; fault PC={:08X}, CD commands={}, last CD command={:?}, SPU transfers={} halfwords",
                machine.cpu.instruction_address(),
                machine.bus.cdrom.commands,
                machine.bus.cdrom.last_command,
                machine.bus.spu.transferred_halfwords
            ));
        }
        Ok(())
    } else {
        #[cfg(feature = "desktop")]
        {
            desktop::run(
                machine,
                desktop::Options {
                    muted,
                    auto_run,
                    bios: bios_path,
                    fullscreen,
                    game_view,
                    theme,
                    theme_plugin,
                    crt: crt.unwrap_or_default(),
                    graphics_open: false,
                },
                card_store,
            )
        }
        #[cfg(not(feature = "desktop"))]
        {
            let _ = auto_run;
            Err(
                "Desktop support is disabled. Use --headless or build with default features."
                    .into(),
            )
        }
    }
}

fn write_frame(path: &std::path::Path, (w, h): (usize, usize), rgb: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut file = std::io::BufWriter::new(file);
    write!(file, "P6\n{w} {h}\n255\n")
        .and_then(|()| file.write_all(rgb))
        .and_then(|()| file.flush())
        .map_err(|e| e.to_string())
}

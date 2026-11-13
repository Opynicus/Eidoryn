//! Startup dispatch. Argument parsing and headless execution have independent boundaries.
mod headless;
mod options;

use eidoryn_psx::app::{BINARY_NAME, NAME, VERSION};
use eidoryn_psx::systems::ps1::{
    card_file::CardStore,
    demo,
    machine::Machine,
    media::{load_disc, load_path},
};
use options::{Invocation, SessionOptions};
use std::{io::Write, path::Path};

pub(crate) fn run() -> Result<(), String> {
    match options::parse(std::env::args_os().skip(1))? {
        Invocation::Help => {
            print_help();
            Ok(())
        }
        Invocation::Inspect(path) => inspect_disc(&path),
        Invocation::Export { path, disc_test } => export_demo(&path, disc_test),
        Invocation::Run(options) => launch(options),
    }
}

fn print_help() {
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

fn inspect_disc(path: &Path) -> Result<(), String> {
    let disc = eidoryn_psx::systems::ps1::disc::DiscImage::open(path).map_err(|e| e.to_string())?;
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
    Ok(())
}

fn export_demo(path: &Path, disc_test: bool) -> Result<(), String> {
    let mut file =
        std::fs::File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(&if disc_test {
        demo::disc_probe_executable()
    } else {
        demo::executable()
    })
    .map_err(|e| e.to_string())?;
    println!("Exported MIPS demo to {}", path.display());
    Ok(())
}

fn launch(options: SessionOptions) -> Result<(), String> {
    let mut machine = load_machine(&options)?;
    let SessionOptions {
        bios_path,
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
        ..
    } = options;
    if headless {
        machine
            .bus
            .gpu
            .set_renderer(renderer.unwrap_or_default())
            .map_err(|error| format!("Cannot start GPU renderer: {error}"))?;
    }
    if machine.bus.gpu.renderer_mode() == eidoryn_psx::systems::ps1::gpu::RendererMode::Vulkan {
        eprintln!("Renderer: {}", machine.bus.gpu.renderer_device());
    }
    machine
        .set_cpu_mode(cpu.unwrap_or_default())
        .map_err(|error| format!("Cannot start CPU engine: {error}"))?;
    let (card_store, cards) = CardStore::open(card_paths)?;
    machine.bus.sio.cards = cards;
    if headless {
        headless::run(
            machine,
            card_store,
            frames,
            dump.as_deref(),
            audio_dump.as_deref(),
        )
    } else {
        #[cfg(feature = "desktop")]
        {
            crate::desktop::run(
                machine,
                crate::desktop::Options {
                    debugger,
                    language,
                    muted,
                    volume,
                    renderer,
                    clean_fullscreen,
                    auto_run,
                    bios_path,
                    bios: bios_path,
                    fullscreen,
                    game_view,
                    theme,
                    theme_plugin,
                    crt,
                    graphics_open: false,
                },
                card_store,
            )
        }
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (
                auto_run,
                bios_path,
                language,
                fullscreen,
                game_view,
                debugger,
                muted,
                volume,
                clean_fullscreen,
                theme,
                theme_plugin,
                crt,
            );
            Err(
                "Desktop support is disabled. Use --headless or build with default features."
                    .into(),
            )
        }
    }
}

fn load_machine(options: &SessionOptions) -> Result<Machine, String> {
    if let Some(path) = &options.disc {
        load_disc(path, options.bios_path.as_deref(), options.fast_boot)
    } else if let Some(path) = &options.source {
        load_path(path, false)
    } else if let Some(path) = &options.bios_path {
        load_path(path, true)
    } else {
        Ok(Machine::demo())
    }
}

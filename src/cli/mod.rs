//! CLI orchestration and startup dispatch.
mod args;
mod headless;
#[cfg(test)]
mod tests;

#[cfg(feature = "desktop")]
use crate::desktop;
use args::Options;
use eidoryn_psx::systems::ps1::{
    card_file::CardStore,
    demo,
    machine::Machine,
    media::{load_disc, load_path},
};
use std::{ffi::OsString, io::Write, path::Path};

pub(super) fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let Some(options) = args::parse(arguments)? else {
        args::print_help();
        return Ok(());
    };
    options.validate()?;
    if options.list_renderers {
        list_renderers();
        return Ok(());
    }
    if let Some(path) = &options.inspect {
        return inspect_disc(path);
    }
    if let Some(path) = &options.export {
        return export_executable(path, options.export_disc_test);
    }
    launch(options)
}

fn list_renderers() {
    let backends = eidoryn_psx::systems::ps1::gpu::hardware_backends();
    println!("serial: available (CPU software rasterizer)");
    println!("threaded: available (CPU software rasterizer on a worker thread)");
    println!(
        "hardware: {} (automatic platform preference)",
        if backends.iter().any(|backend| backend.device.is_some()) {
            "available"
        } else {
            "unavailable"
        }
    );
    for backend in backends {
        if let Some(device) = backend.device {
            println!("{}: available ({device})", backend.mode.id());
        } else {
            println!(
                "{}: unavailable ({})",
                backend.mode.id(),
                backend.reason.as_deref().unwrap_or("No supported device")
            );
        }
    }
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

fn export_executable(path: &Path, export_disc_test: bool) -> Result<(), String> {
    let mut file =
        std::fs::File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(&if export_disc_test {
        demo::disc_probe_executable()
    } else {
        demo::executable()
    })
    .map_err(|e| e.to_string())?;
    println!("Exported MIPS demo to {}", path.display());
    Ok(())
}

fn launch(options: Options) -> Result<(), String> {
    let auto_run = options.auto_run();
    let mut machine = if let Some(path) = options.disc {
        load_disc(&path, options.bios_path.as_deref(), options.fast_boot)?
    } else if let Some(path) = options.source {
        load_path(&path, false)?
    } else if let Some(path) = &options.bios_path {
        load_path(path, true)?
    } else {
        Machine::demo()
    };
    if options.headless {
        machine
            .bus
            .gpu
            .set_renderer(options.renderer.unwrap_or_default())
            .map_err(|error| format!("Cannot start GPU renderer: {error}"))?;
    }
    if machine.bus.gpu.renderer_mode().is_hardware() {
        eprintln!("Renderer: {}", machine.bus.gpu.renderer_device());
    }
    machine
        .set_cpu_mode(options.cpu.unwrap_or_default())
        .map_err(|error| format!("Cannot start CPU engine: {error}"))?;
    let (card_store, cards) = CardStore::open(if options.no_cards {
        [None, None]
    } else {
        options.card_paths
    })?;
    machine.bus.sio.cards = cards;
    if options.headless {
        headless::run(machine, card_store, options.execution)
    } else {
        #[cfg(feature = "desktop")]
        {
            desktop::run(
                machine,
                desktop::Options {
                    debugger: options.debugger,
                    language: options.language,
                    muted: options.muted,
                    volume: options.volume,
                    renderer: options.renderer,
                    clean_fullscreen: options.clean_fullscreen,
                    auto_run,
                    bios: options.bios_path,
                    fullscreen: options.fullscreen,
                    game_view: options.game_view,
                    theme: options.theme,
                    theme_plugin: options.theme_plugin,
                    crt: options.crt,
                    graphics_open: false,
                },
                card_store,
            )
        }
        #[cfg(not(feature = "desktop"))]
        {
            let _ = (auto_run, options.language);
            Err(
                "Desktop support is disabled. Use --headless or build with default features."
                    .into(),
            )
        }
    }
}

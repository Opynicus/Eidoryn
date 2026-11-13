use eidoryn_psx::app::BINARY_NAME;
use eidoryn_psx::systems::ps1::{
    card_file::CardStore,
    gpu::RendererMode,
    machine::Machine,
    media::{load_disc, load_path},
};
mod cli;

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

fn run() -> Result<(), String> {
    let Some(options) = cli::parse(std::env::args_os().skip(1))? else {
        cli::print_help();
        return Ok(());
    };
    if let Some(path) = &options.inspect {
        return cli::inspect_disc(path);
    }
    if let Some(path) = &options.export {
        return cli::export_demo(path, options.export_disc_test);
    }
    let auto_run = options.source.is_none() && options.bios_path.is_none();
    let auto_run = options.start_running
        || if options.disc.is_some() {
            options.fast_boot
        } else {
            auto_run
        };
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
    if machine.bus.gpu.renderer_mode() == RendererMode::Vulkan {
        eprintln!("Renderer: {}", machine.bus.gpu.renderer_device());
    }
    machine
        .set_cpu_mode(options.cpu.unwrap_or_default())
        .map_err(|error| format!("Cannot start CPU engine: {error}"))?;
    let (card_store, cards) = CardStore::open(options.card_paths)?;
    machine.bus.sio.cards = cards;
    if options.headless {
        cli::run_headless(
            machine,
            card_store,
            options.frames,
            options.dump.as_deref(),
            options.audio_dump.as_deref(),
        )
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
            let _ = auto_run;
            Err(
                "Desktop support is disabled. Use --headless or build with default features."
                    .into(),
            )
        }
    }
}

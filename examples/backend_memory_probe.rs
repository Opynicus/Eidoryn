//! Native UI diagnostic: change CPU and GPU backends separately in one process.
//! Usage: `backend_memory_probe TRACE.csv jit-first|vulkan-first [BIOS DISC]`
//! Optional media is loaded through the worker after the window is created.
//! Memory-card slots remain disconnected; media files are only read.
#[cfg(feature = "desktop")]
#[path = "../src/desktop/mod.rs"]
#[allow(dead_code)] // This diagnostic uses the opt-in probe entry point.
mod desktop;

#[cfg(feature = "desktop")]
fn main() -> Result<(), String> {
    use eidoryn_psx::{
        card_file::CardStore, cpu_engine::CpuMode, gpu::RendererMode, machine::Machine,
        runner::Command,
    };
    use std::{path::PathBuf, time::Duration};

    const USAGE: &str = "Expected TRACE.csv jit-first|vulkan-first [BIOS DISC]";
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or(USAGE)?);
    let order = args.next().ok_or(USAGE)?;
    let mut commands = Vec::new();
    match (args.next(), args.next(), args.next()) {
        (None, None, None) => (),
        (Some(bios), Some(disc), None) => {
            let bios = PathBuf::from(bios);
            commands.extend([
                (
                    2,
                    Command::Load {
                        path: bios.clone(),
                        bios: true,
                    },
                ),
                (
                    4,
                    Command::LoadDisc {
                        path: PathBuf::from(disc),
                        bios: Some(bios),
                        fast_boot: false,
                    },
                ),
                (5, Command::Toggle),
            ]);
        }
        _ => return Err(USAGE.into()),
    }
    let switches = match order.to_str() {
        Some("jit-first") => [
            Command::SetCpuMode(CpuMode::Jit),
            Command::SetRenderer(RendererMode::Vulkan),
            Command::SetCpuMode(CpuMode::Interpreter),
            Command::SetRenderer(RendererMode::Serial),
        ],
        Some("vulkan-first") => [
            Command::SetRenderer(RendererMode::Vulkan),
            Command::SetCpuMode(CpuMode::Jit),
            Command::SetRenderer(RendererMode::Serial),
            Command::SetCpuMode(CpuMode::Interpreter),
        ],
        _ => return Err(USAGE.into()),
    };
    commands.extend([12, 17, 22, 26].into_iter().zip(switches));
    desktop::run_with_probe(
        Machine::demo(),
        desktop::Options {
            muted: Some(true),
            auto_run: true,
            renderer: Some(RendererMode::Serial),
            ..Default::default()
        },
        CardStore::default(),
        Some(desktop::ProbeOptions {
            seconds: 30,
            buttons: 0,
            path,
            commands: commands
                .into_iter()
                .map(|(seconds, command)| (Duration::from_secs(seconds), command))
                .collect(),
        }),
    )
}

#[cfg(not(feature = "desktop"))]
fn main() -> Result<(), String> {
    Err("Backend memory probe requires the desktop feature".into())
}

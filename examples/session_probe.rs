//! Exercise session transitions in a real GPUI window, then close normally.
//! Usage: `cargo run --example session_probe -- TRACE.csv DEMO.exe DISC.iso`
//! Media is read only and both memory-card slots are disconnected.
#[cfg(feature = "desktop")]
#[path = "../src/desktop/mod.rs"]
#[allow(dead_code)] // This diagnostic uses the existing opt-in window probe.
mod desktop;

#[cfg(feature = "desktop")]
fn main() -> Result<(), String> {
    use eidoryn_psx::{card_file::CardStore, gpu::RendererMode, machine::Machine, runner::Command};
    use std::{path::PathBuf, time::Duration};

    let mut args = std::env::args_os().skip(1);
    let usage = "Expected TRACE.csv DEMO.exe DISC.iso";
    let trace = PathBuf::from(args.next().ok_or(usage)?);
    let exe = PathBuf::from(args.next().ok_or(usage)?);
    let disc = PathBuf::from(args.next().ok_or(usage)?);
    if args.next().is_some() {
        return Err(usage.into());
    }
    let commands = [
        (1, Command::Toggle),
        (2, Command::Step),
        (3, Command::Slice),
        (4, Command::Toggle),
        (5, Command::SetRenderer(RendererMode::Threaded)),
        (6, Command::SetRenderer(RendererMode::Serial)),
        (7, Command::Reset),
        (
            8,
            Command::Load {
                path: exe,
                bios: false,
            },
        ),
        (9, Command::Toggle),
        (
            11,
            Command::Load {
                path: PathBuf::new(),
                bios: false,
            },
        ),
        (12, Command::Toggle),
        (
            14,
            Command::LoadDisc {
                path: PathBuf::new(),
                bios: None,
                fast_boot: true,
            },
        ),
        (15, Command::Demo),
        (
            17,
            Command::LoadDisc {
                path: disc,
                bios: None,
                fast_boot: true,
            },
        ),
        (20, Command::Reset),
        (21, Command::Toggle),
        (23, Command::SetRenderer(RendererMode::Threaded)),
        (24, Command::SetRenderer(RendererMode::Serial)),
        (25, Command::FlushCards),
    ];
    desktop::run_with_probe(
        Machine::demo(),
        desktop::Options {
            auto_run: true,
            muted: Some(true),
            renderer: Some(RendererMode::Serial),
            ..Default::default()
        },
        CardStore::default(),
        Some(desktop::ProbeOptions {
            seconds: 28,
            buttons: 0,
            path: trace,
            commands: commands
                .into_iter()
                .map(|(seconds, command)| (Duration::from_secs(seconds), command))
                .collect(),
        }),
    )
}

#[cfg(not(feature = "desktop"))]
fn main() -> Result<(), String> {
    Err("Session probe requires the desktop feature".into())
}

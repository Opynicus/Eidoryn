//! Native UI diagnostic: serial -> threaded -> serial -> Vulkan -> serial -> Vulkan.
//! Usage: `renderer_memory_probe TRACE.csv [BIOS DISC]`
//! Optional media is read only; both memory-card slots are disconnected.
#[cfg(feature = "desktop")]
#[path = "../src/desktop/mod.rs"]
#[allow(dead_code)] // This diagnostic uses run_with_probe rather than the normal entry point.
mod desktop;

#[cfg(feature = "desktop")]
fn main() -> Result<(), String> {
    use eidoryn_psx::{card_file::CardStore, gpu::RendererMode, machine::Machine};
    use std::{path::PathBuf, time::Duration};

    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("Expected TRACE.csv [BIOS DISC]")?);
    let machine = match (args.next(), args.next(), args.next()) {
        (None, None, None) => Machine::demo(),
        (Some(bios), Some(disc), None) => {
            eidoryn_psx::media::load_disc(&PathBuf::from(disc), Some(&PathBuf::from(bios)), false)?
        }
        _ => return Err("Expected TRACE.csv [BIOS DISC]".into()),
    };
    desktop::run_with_probe(
        machine,
        desktop::Options {
            language: Some(eidoryn_psx::i18n::Language::En),
            muted: Some(true),
            auto_run: true,
            renderer: Some(RendererMode::Serial),
            theme: Some("midnight".into()),
            crt: Some(eidoryn_psx::video_filter::CrtFilter::Off),
            ..Default::default()
        },
        CardStore::default(),
        Some(desktop::ProbeOptions {
            seconds: 30,
            buttons: 0,
            path,
            renderer_changes: [
                (5, RendererMode::Threaded),
                (10, RendererMode::Serial),
                (15, RendererMode::Vulkan),
                (20, RendererMode::Serial),
                (25, RendererMode::Vulkan),
            ]
            .map(|(seconds, mode)| (Duration::from_secs(seconds), mode))
            .into(),
        }),
    )
}

#[cfg(not(feature = "desktop"))]
fn main() -> Result<(), String> {
    Err("Renderer memory probe requires the desktop feature".into())
}

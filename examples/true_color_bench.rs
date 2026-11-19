//! Compare presentation precision from a fixed, read-only PS1 checkpoint.
#[path = "support/frames.rs"]
mod frames;

use eidoryn_psx::{
    gpu::{ColorDepth, RenderScale, RendererMode},
    media::load_disc,
    systems::ps1::state_file::StateStore,
};
use std::path::Path;

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [
        bios,
        disc,
        checkpoint,
        backend,
        scale,
        count,
        buttons,
        output,
    ] = args.as_slice()
    else {
        return Err(
            "Usage: true_color_bench BIOS DISC STATE BACKEND 1x|2x|4x FRAMES HEX_BUTTONS OUTPUT_DIR".into(),
        );
    };
    let backend: RendererMode = backend.to_str().ok_or("Invalid backend")?.parse()?;
    if !backend.is_hardware() {
        return Err("Choose a hardware renderer that supports true color".into());
    }
    let scale = match scale.to_str() {
        Some("1x") => RenderScale::Native,
        Some("2x") => RenderScale::X2,
        Some("4x") => RenderScale::X4,
        _ => return Err("Expected 1x, 2x or 4x".into()),
    };
    let count = count
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or("Invalid frame count")?;
    let buttons = u16::from_str_radix(buttons.to_str().ok_or("Invalid button mask")?, 16)
        .map_err(|e| e.to_string())?;
    let output = Path::new(output);
    let boot = load_disc(Path::new(disc), Some(Path::new(bios)), false)?;
    let store = StateStore::new(output, &boot);
    let (mut machine, _) = store.load(Path::new(checkpoint))?;
    std::fs::create_dir(output).map_err(|e| format!("Cannot create {}: {e}", output.display()))?;
    for (depth, label) in [
        (ColorDepth::Native, "native"),
        (ColorDepth::TrueColor, "true-color"),
    ] {
        machine
            .bus
            .gpu
            .set_renderer_config(backend, scale, depth)
            .map_err(|e| e.to_string())?;
        println!(
            "{label}: {}, {}x, state PC={:08x}",
            machine.bus.gpu.renderer_device(),
            scale.factor(),
            machine.cpu.pc
        );
        let prefix = output.join(label);
        frames::benchmark(
            &machine,
            count,
            buttons,
            &prefix,
            Some(&prefix.with_extension("ppm")),
        )?;
    }
    Ok(())
}

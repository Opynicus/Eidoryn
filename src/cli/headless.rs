//! Bounded CLI execution and diagnostic exports, with final card synchronization.
use eidoryn_psx::systems::ps1::{card_file::CardStore, machine::Machine};
use std::{io::Write, path::Path};

pub(super) fn run(
    mut machine: Machine,
    mut card_store: CardStore,
    frames: Option<u32>,
    dump: Option<&Path>,
    audio_dump: Option<&Path>,
) -> Result<(), String> {
    let mut wav = audio_dump
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
            path,
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
}

fn write_frame(path: &std::path::Path, (w, h): (usize, usize), rgb: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut file = std::io::BufWriter::new(file);
    write!(file, "P6\n{w} {h}\n255\n")
        .and_then(|()| file.write_all(rgb))
        .and_then(|()| file.flush())
        .map_err(|e| e.to_string())
}

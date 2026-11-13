//! Non-window operations, including save flushing and diagnostic output on guest failure.
use eidoryn_psx::systems::ps1::{card_file::CardStore, demo, machine::Machine};
use std::{io::Write, path::Path};

pub(crate) fn inspect_disc(path: &Path) -> Result<(), String> {
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

pub(crate) fn export_demo(path: &Path, export_disc_test: bool) -> Result<(), String> {
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

pub(crate) fn run_headless(
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

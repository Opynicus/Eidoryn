//! Host file loading and validation, independent of execution threads and GPUI.
use crate::systems::ps1::{
    bus::{BIOS_SIZE, RAM_SIZE},
    machine::Machine,
};

pub fn load_path(path: &std::path::Path, bios: bool) -> Result<Machine, String> {
    let bytes = read_image(path, bios)?;
    let title = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    if bios {
        Machine::from_bios(&bytes, &title)
    } else {
        Machine::from_exe(&bytes, &title)
    }
    .map_err(|e| e.to_string())
}

fn read_image(path: &std::path::Path, bios: bool) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err("Select a regular BIOS or PS-X EXE file".into());
    }
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ["iso", "cue", "chd"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return Err(format!(
            "{} is a game disc image. Use Open Disc or --disc for the game, and select a separate 512 KiB BIOS firmware file with Open BIOS or --bios.",
            path.display()
        ));
    }
    if bios && metadata.len() != BIOS_SIZE as u64 {
        return Err(format!(
            "BIOS firmware must be exactly 512 KiB ({BIOS_SIZE} bytes); {} is {} bytes. Select the game with Open Disc or --disc, and the BIOS firmware with Open BIOS or --bios.",
            path.display(),
            metadata.len()
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Also bounds reads from special files or files changed during the read.
    let limit = if bios { BIOS_SIZE } else { RAM_SIZE + 0x800 };
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > limit {
        return Err(if bios {
            "BIOS file changed while reading; select a 512 KiB firmware file".into()
        } else {
            format!(
                "PS-X EXE exceeds {limit} bytes. For game discs, use Open Disc or --disc with a separate BIOS firmware file."
            )
        });
    }
    // Keep BIOS validation (and its error) ahead of disc opening, even if the
    // file was truncated after metadata(). Loading a disc needs only the bytes,
    // not a throwaway Machine with RAM, VRAM and SPU buffers.
    if bios && bytes.len() != BIOS_SIZE {
        return Err(crate::systems::ps1::EmuError::InvalidImage(format!(
            "BIOS must be exactly 512 KiB; got {} bytes",
            bytes.len()
        ))
        .to_string());
    }
    Ok(bytes)
}

pub fn load_disc(
    path: &std::path::Path,
    bios: Option<&std::path::Path>,
    fast_boot: bool,
) -> Result<Machine, String> {
    let bios = bios.map(|path| read_image(path, true)).transpose()?;
    Machine::from_disc(path, bios.as_deref(), fast_boot).map_err(|e| e.to_string())
}

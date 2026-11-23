use eidoryn_gba::BIOS_LEN;
use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

pub(super) fn installed_path(data_dir: &Path) -> PathBuf {
    data_dir.join("firmware/gba_bios.bin")
}

fn at_path(path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn read_bios(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "expected a regular GBA BIOS file",
        ));
    }
    if metadata.len() != BIOS_LEN as u64 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "GBA BIOS must be exactly 16 KiB ({BIOS_LEN} bytes), found {} bytes",
                metadata.len()
            ),
        ));
    }
    let mut bytes = Vec::with_capacity(BIOS_LEN);
    File::open(path)?
        .take(BIOS_LEN as u64 + 1)
        .read_to_end(&mut bytes)?;
    eidoryn_gba::validate_bios(&bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
    Ok(bytes)
}

pub(super) fn load(installed: &Path, legacy: Option<&Path>) -> io::Result<Vec<u8>> {
    for path in std::iter::once(installed).chain(legacy) {
        match read_bios(path) {
            Ok(bytes) => return Ok(bytes),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(at_path(path, &e)),
        }
    }
    Err(io::Error::new(
        ErrorKind::NotFound,
        format!("GBA BIOS missing at {}", installed.display()),
    ))
}

/// Checks whether starting a GBA cartridge requires importing firmware.
///
/// # Errors
/// Returns an I/O error if an existing firmware file cannot be accessed.
pub fn needs_firmware_import(data_dir: &Path, legacy_bios: Option<&Path>) -> io::Result<bool> {
    match load(&installed_path(data_dir), legacy_bios) {
        Ok(_) => Ok(false),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::InvalidData) => Ok(true),
        Err(e) => Err(e),
    }
}

/// Installs a validated copy of the user's GBA firmware for all cartridges and regions.
/// Concurrent imports keep the first usable installation.
///
/// # Errors
/// Returns an error if the source is invalid or the firmware cannot be installed.
pub fn import_firmware(data_dir: &Path, source: &Path) -> io::Result<()> {
    let bytes = read_bios(source).map_err(|e| at_path(source, &e))?;
    let installed = installed_path(data_dir);
    let write = || -> io::Result<()> {
        let dir = installed
            .parent()
            .expect("installed firmware path has a directory");
        fs::create_dir_all(dir)?;
        // Serialize repairs as well as first installs. Keep the lock file so another
        // process always locks the same inode even when the firmware is replaced.
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(installed.with_extension("lock"))?;
        lock.lock()?;
        match read_bios(&installed) {
            Ok(_) => return Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::InvalidData) => {}
            Err(e) => return Err(e),
        }
        let mut pending = tempfile::NamedTempFile::new_in(dir)?;
        pending.write_all(&bytes)?;
        pending.as_file().sync_all()?;
        pending.persist(&installed)?;
        Ok(())
    };
    write().map_err(|e| at_path(&installed, &e))
}

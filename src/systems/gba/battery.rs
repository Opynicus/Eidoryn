use super::Cartridge;
use crate::systems::persistence::atomic_write;
use eidoryn_gba::Machine;
use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

fn read_image(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > eidoryn_gba::MAX_BATTERY_LEN as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a regular GBA save file of at most 128 KiB",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(eidoryn_gba::MAX_BATTERY_LEN as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn load_existing(machine: &mut Machine, path: &Path) -> Result<(), String> {
    let bytes = match read_image(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if bytes.len() == 32 * 1024 && machine.validate_battery_size(bytes.len()).is_err() {
        // Earlier releases exposed every cartridge as SRAM. Keep the complete original
        // before replacing it. EEPROM commands never wrote meaningful parallel SRAM.
        let legacy = path.with_extension("legacy-sram.sav");
        match atomic_write(&legacy, &bytes, true) {
            Ok(()) => {}
            Err(e)
                if e.kind() == io::ErrorKind::AlreadyExists
                    && read_image(&legacy).is_ok_and(|saved| saved == bytes) => {}
            Err(e) => return Err(format!("{}: {e}", legacy.display())),
        }
        let size = machine.battery_save().len();
        if size > bytes.len() {
            let mut expanded = bytes;
            expanded.resize(size, 0xff);
            machine
                .restore_battery_save(&expanded)
                .map_err(|e| e.to_string())?;
        }
        return write_image(path, machine.battery_save());
    }
    machine
        .restore_battery_save(&bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn check_extension(path: &Path) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sav"))
    {
        return Err(format!("Expected a .sav file: {}", path.display()));
    }
    Ok(())
}

fn write_image(path: &Path, bytes: &[u8]) -> Result<(), String> {
    atomic_write(path, bytes, false).map_err(|e| format!("{}: {e}", path.display()))
}

impl Cartridge {
    pub(super) fn flush(&mut self) -> Result<(), String> {
        self.pending_save |= self.machine.take_battery_dirty();
        if self.pending_save {
            write_image(&self.save, self.machine.battery_save())?;
            self.pending_save = false;
        }
        Ok(())
    }

    pub(super) fn sync_battery(&mut self) -> Result<(), String> {
        // A manual save also materializes an untouched (erased) chip.
        self.pending_save = true;
        self.flush()
    }

    pub(super) fn import_battery(&mut self, path: &Path) -> Result<(), String> {
        check_extension(path)?;
        let bytes = read_image(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.machine
            .validate_battery_size(bytes.len())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // A failed disk write leaves the running chip and its dirty flag untouched.
        write_image(&self.save, &bytes)?;
        self.machine
            .restore_battery_save(&bytes)
            .map_err(|e| e.to_string())?;
        self.pending_save = false;
        Ok(())
    }

    pub(super) fn export_battery(&self, path: &Path) -> Result<(), String> {
        check_extension(path)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let dir = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let managed = fs::canonicalize(self.save.parent().expect("managed save directory"))
            .map_err(|e| e.to_string())?;
        let destination = dir.join(path.file_name().expect("validated .sav filename"));
        if dir.starts_with(&managed)
            && destination != managed.join(self.save.file_name().expect("managed save filename"))
        {
            return Err("Choose a file outside the managed cartridge save directory".into());
        }
        write_image(&destination, self.machine.battery_save())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_export_and_sync_publish_only_complete_cartridge_images() {
        let dir = tempfile::tempdir().unwrap();
        let rom = dir.path().join("probe.gba");
        fs::write(&rom, vec![0; eidoryn_gba::HEADER_LEN]).unwrap();
        let saves = dir.path().join("saves");
        let (mut cart, _) = Cartridge::load(&rom, &saves, None).unwrap();
        cart.sync_battery().unwrap();
        let source = dir.path().join("source.sav");
        let destination = dir.path().join("exported.sav");
        let raw = vec![0x1b; 32 * 1024];
        fs::write(&source, &raw).unwrap();
        cart.import_battery(&source).unwrap();
        assert_eq!(fs::read(&cart.save).unwrap(), raw);
        cart.export_battery(&destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), raw);
        fs::write(&source, [1, 2, 3]).unwrap();
        assert!(cart.import_battery(&source).is_err());
        assert_eq!(cart.machine.battery_save(), raw);
        assert_eq!(fs::read(&cart.save).unwrap(), raw);
        assert!(cart.export_battery(&rom).is_err());
        let other = saves.join("other-game.sav");
        assert!(cart.export_battery(&other).is_err());
        assert!(!other.exists());
        drop(cart);
        let (fresh, _) = Cartridge::load(&rom, &saves, None).unwrap();
        assert_eq!(fresh.machine.battery_save(), raw);
    }

    #[test]
    fn migration_preserves_the_original_parallel_sram_and_only_runs_for_managed_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut rom = vec![0; eidoryn_gba::HEADER_LEN];
        rom.extend_from_slice(b"FLASH1M_V103");
        let mut machine = Machine::from_rom(rom).unwrap();
        let save = dir.path().join("probe.sav");
        let legacy = vec![0x7c; 32 * 1024];
        fs::write(&save, &legacy).unwrap();
        load_existing(&mut machine, &save).unwrap();
        assert_eq!(
            fs::read(save.with_extension("legacy-sram.sav")).unwrap(),
            legacy
        );
        assert_eq!(machine.battery_save()[..legacy.len()], legacy);
        assert!(
            machine.battery_save()[legacy.len()..]
                .iter()
                .all(|&b| b == 0xff)
        );
        assert_eq!(fs::read(&save).unwrap(), machine.battery_save());
        assert!(machine.validate_battery_size(legacy.len()).is_err());
    }
}

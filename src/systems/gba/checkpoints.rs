//! Persist and restore the active cartridge, publishing directory changes to the UI.
use super::{Cartridge, QuickSlot, StateDirectory, Worker};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

impl Worker {
    fn states(&self) -> Result<&StateDirectory, String> {
        self.states
            .as_ref()
            .ok_or_else(|| "Load a cartridge first".into())
    }

    fn cart(&mut self) -> Result<&mut Cartridge, String> {
        self.cartridge
            .as_mut()
            .ok_or_else(|| "Load a cartridge first".into())
    }

    fn refresh_quick_slot(&mut self) -> Result<(), String> {
        self.snapshot.quick_slot = QuickSlot::Unavailable;
        self.snapshot.quick_slot = self
            .states()?
            .quick_slot_modified()?
            .map_or(QuickSlot::Empty, QuickSlot::Saved);
        Ok(())
    }

    pub(super) fn refresh_states(&mut self) -> Result<(), String> {
        let quick = self.refresh_quick_slot();
        self.snapshot.save_states = self.states()?.list()?.into();
        quick
    }

    pub(super) fn save_state(&mut self, quick: bool) -> Result<(), String> {
        let bytes = self
            .cart()?
            .machine
            .save_state()
            .map_err(|e| e.to_string())?;
        let action = if quick {
            self.states()?.save_quick(&bytes)?;
            self.snapshot.quick_slot_revision = self.snapshot.quick_slot_revision.wrapping_add(1);
            "Quick-saved state"
        } else {
            self.states()?.save(&bytes)?;
            "Saved state"
        };
        self.refresh_states()?;
        self.snapshot.success_message = Some(action.into());
        Ok(())
    }

    pub(super) fn load_state(&mut self, path: &Path) -> Result<(), String> {
        let states = self.states()?;
        if path != states.quick_path() {
            states
                .history_metadata(path)
                .map_err(|e| format!("Cannot load {}: {e}", path.display()))?;
        }
        let file = File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
        let max = eidoryn_gba::MAX_STATE_BYTES;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > max as u64 {
            return Err(format!("Invalid GBA checkpoint size: {}", path.display()));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let keys = self.held;
        let cart = self.cart()?;
        cart.machine
            .load_state(&bytes)
            .map_err(|e| format!("Cannot restore {}: {e}", path.display()))?;
        cart.machine.set_buttons(keys);
        self.snapshot.rgb = Arc::from(cart.machine.pixels());
        self.reset_frame_rate();
        self.reset_rewind()?;
        self.snapshot.success_message = Some(format!("Loaded state: {}", path.display()));
        Ok(())
    }

    pub(super) fn quick_save_if_unchanged(
        &mut self,
        revision: u64,
        slot: QuickSlot,
    ) -> Result<(), String> {
        self.refresh_quick_slot()?;
        if self.snapshot.quick_slot_revision != revision || self.snapshot.quick_slot != slot {
            return Err("Quick slot changed. Check the current slot and try again.".into());
        }
        self.save_state(true)
    }

    pub(super) fn quick_load(&mut self) -> Result<(), String> {
        self.refresh_quick_slot()?;
        self.load_state(&self.states()?.quick_path())
    }

    pub(super) fn rename_state(&mut self, path: PathBuf, name: &str) -> Result<(), String> {
        let dest = self.states()?.rename(&path, name)?;
        self.snapshot.renamed_state = Some((path, dest));
        self.refresh_states()?;
        self.snapshot.success_message = Some(format!("Renamed snapshot: {name}"));
        Ok(())
    }

    pub(super) fn delete_state(&mut self, path: &Path, modified: SystemTime) -> Result<(), String> {
        self.states()?.delete(path, modified)?;
        self.refresh_states()?;
        self.snapshot.success_message = Some("Deleted snapshot".into());
        Ok(())
    }

    pub(super) fn sync_battery(&mut self) -> Result<(), String> {
        self.cart()?.sync_battery()?;
        self.snapshot.success_message = Some("Saved cartridge battery".into());
        Ok(())
    }

    pub(super) fn import_battery(&mut self, path: &Path) -> Result<(), String> {
        let keys = self.held;
        let cart = self.cart()?;
        cart.import_battery(path)?;
        cart.machine.reset();
        cart.machine.set_buttons(keys);
        self.snapshot.rgb = Arc::from(cart.machine.pixels());
        self.reset_rewind()?;
        self.reset_frame_rate();
        self.snapshot.success_message = Some(format!("Imported battery save: {}", path.display()));
        Ok(())
    }

    pub(super) fn export_battery(&mut self, path: &Path) -> Result<(), String> {
        self.cart()?.export_battery(path)?;
        self.snapshot.success_message = Some(format!("Exported battery save: {}", path.display()));
        Ok(())
    }
}

//! Atomic full-machine history and a dedicated quick-save slot, separate from memory cards.
use super::{input::PadInput, machine::Machine};
use crate::systems::checkpoints::StateDirectory;
pub use crate::systems::checkpoints::{SaveStateInfo, checkpoint_time};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAGIC: &[u8; 8] = b"EIDORYN\x01";
const HEADER_SIZE: usize = 8 + 8 + 69 + 8;
const MAX_SIZE: u64 = 128 * 1024 * 1024;

pub struct StateStore {
    history: StateDirectory,
    game_id: u64,
}

impl StateStore {
    /// Derive identity from the boot media before execution mutates RAM.
    pub fn new(root: &Path, initial: &Machine) -> Self {
        let disc = initial.bus.cdrom.disc_identity();
        let game_id = checksum(
            initial
                .bus
                .bios
                .iter()
                .copied()
                .chain(initial.bus.ram.iter().copied())
                .chain(initial.cpu.pc.to_le_bytes())
                .chain(
                    initial
                        .cpu
                        .registers
                        .iter()
                        .flat_map(|value| value.to_le_bytes()),
                )
                .chain(disc.as_deref().unwrap_or("").bytes()),
        );
        let title = initial.title.split(" / ").next().unwrap_or(&initial.title);
        let name = StateDirectory::file_prefix(title);
        Self {
            history: StateDirectory::new(
                root.join(format!("{name}-{game_id:016x}")),
                name,
                "ps1state",
            ),
            game_id,
        }
    }

    pub fn list(&self) -> Result<Vec<SaveStateInfo>, String> {
        self.history.list()
    }

    pub fn rename(&self, path: &Path, name: &str) -> Result<PathBuf, String> {
        self.history.rename(path, name)
    }

    pub fn delete(&self, path: &Path, modified: SystemTime) -> Result<(), String> {
        self.history.delete(path, modified)
    }

    pub fn save(&self, machine: &Machine, input: &PadInput) -> Result<PathBuf, String> {
        self.history.save(&self.encode(machine, input)?)
    }

    pub(crate) fn quick_slot_modified(&self) -> Result<Option<SystemTime>, String> {
        self.history.quick_slot_modified()
    }

    pub fn quick_path(&self) -> PathBuf {
        self.history.quick_path()
    }

    pub fn save_quick(&self, machine: &Machine, input: &PadInput) -> Result<PathBuf, String> {
        self.history.save_quick(&self.encode(machine, input)?)
    }

    fn encode(&self, machine: &Machine, input: &PadInput) -> Result<Vec<u8>, String> {
        let state = machine.save_state()?;
        let mut bytes = Vec::with_capacity(HEADER_SIZE + state.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.game_id.to_le_bytes());
        bytes.extend_from_slice(&input.save_state());
        let hash = checksum(bytes.iter().copied().chain(state.iter().copied()));
        bytes.extend_from_slice(&hash.to_le_bytes());
        bytes.extend_from_slice(&state);
        if bytes.len() as u64 > MAX_SIZE {
            return Err("Save state exceeds the 128 MiB size limit".into());
        }
        Ok(bytes)
    }

    /// Decode and validate into a separate machine; callers install only on success.
    pub fn load(&self, path: &Path) -> Result<(Machine, PadInput), String> {
        let read = || -> Result<_, String> {
            let file = File::open(path).map_err(|e| e.to_string())?;
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.len() > MAX_SIZE {
                return Err("Expected a save state file smaller than 128 MiB".into());
            }
            let mut bytes = Vec::new();
            file.take(MAX_SIZE + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() < HEADER_SIZE || bytes.len() as u64 > MAX_SIZE || &bytes[..8] != MAGIC {
                return Err("Invalid or unsupported save state format".into());
            }
            if bytes[8..16] != self.game_id.to_le_bytes() {
                return Err("Save state belongs to a different game or BIOS".into());
            }
            let hash = checksum(
                bytes[..85]
                    .iter()
                    .copied()
                    .chain(bytes[HEADER_SIZE..].iter().copied()),
            );
            if bytes[85..HEADER_SIZE] != hash.to_le_bytes() {
                return Err("Save state checksum mismatch".into());
            }
            let input = PadInput::load_state(bytes[16..85].try_into().expect("validated header"))?;
            let machine = Machine::load_state(&bytes[HEADER_SIZE..])?;
            Ok((machine, input))
        };
        read().map_err(|error| format!("Cannot load state {}: {error}", path.display()))
    }
}

fn checksum(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::UNIX_EPOCH};

    #[test]
    fn checkpoints_keep_history_and_reject_wrong_games_and_damage() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-states-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut machine = Machine::demo();
        machine.title = "即时存档测试".repeat(40);
        let store = StateStore::new(&root, &machine);
        assert_eq!(store.list().unwrap(), []);
        let mut input = PadInput::default();
        input.configure(
            super::super::input::TurboConfig::new(0x8000, 10).unwrap(),
            0,
        );
        input.set_buttons(0x8020, 13);
        machine.run_slice().unwrap();
        let first = store.save(&machine, &input).unwrap();
        machine.run_slice().unwrap();
        let second = store.save(&machine, &input).unwrap();
        assert_ne!(first, second);
        let history = store.list().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].path, second);
        assert_eq!(
            history[0].name,
            second.file_stem().unwrap().to_str().unwrap()
        );
        assert!(first.starts_with(&store.history.directory));
        let (restored, restored_input) = store.load(&first).unwrap();
        assert_eq!(restored.slices, 1);
        assert!(restored.bus.sio.cards.iter().all(Option::is_none));
        assert_eq!(restored_input.save_state(), input.save_state());
        let other_game = StateStore::new(&root, &machine);
        assert!(
            other_game
                .load(&first)
                .err()
                .unwrap()
                .contains("different game")
        );
        let mut bytes = fs::read(&first).unwrap();
        bytes[20] ^= 1;
        fs::write(&first, &bytes).unwrap();
        assert!(store.load(&first).err().unwrap().contains("checksum"));
        fs::write(&first, &bytes[..20]).unwrap();
        assert!(store.load(&first).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quick_slot_replaces_only_its_game_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-quick-slot-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut machine = Machine::demo();
        let store = StateStore::new(&root, &machine);
        let mut input = PadInput::default();
        let historical = store.save(&machine, &input).unwrap();
        let historical = store.rename(&historical, "quick").unwrap();
        let historical_bytes = fs::read(&historical).unwrap();
        assert!(store.quick_slot_modified().unwrap().is_none());
        let first = store.save_quick(&machine, &input).unwrap();
        assert_eq!(
            store.quick_slot_modified().unwrap(),
            Some(fs::metadata(&first).unwrap().modified().unwrap())
        );
        assert_eq!(first, store.quick_path());
        machine.run_slice().unwrap();
        input.set_buttons(0x8020, machine.bus.elapsed_cycles());
        let expected = machine.save_state().unwrap();
        assert_eq!(store.save_quick(&machine, &input).unwrap(), first);
        assert_eq!(fs::read_dir(first.parent().unwrap()).unwrap().count(), 1);
        let (restored, restored_input) = store.load(&first).unwrap();
        assert_eq!(restored.save_state().unwrap(), expected);
        assert_eq!(restored_input.save_state(), input.save_state());
        assert_eq!(fs::read(&historical).unwrap(), historical_bytes);
        let history = store.list().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].path, historical);
        assert!(store.rename(&first, "move-quick-slot").is_err());

        let mut other = Machine::demo();
        other.cpu.registers[5] = 91;
        let other_store = StateStore::new(&root, &other);
        assert_ne!(other_store.quick_path(), first);
        assert!(!other_store.quick_path().exists());
        assert!(other_store.load(&first).is_err());
        other_store
            .save_quick(&other, &PadInput::default())
            .unwrap();
        assert_eq!(
            store.load(&first).unwrap().0.save_state().unwrap(),
            expected
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_names_matching_automatic_names_are_displayed_in_full() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-rename-label-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let machine = Machine::demo();
        let store = StateStore::new(&root, &machine);
        let first = store.save(&machine, &PadInput::default()).unwrap();
        let second = store.save(&machine, &PadInput::default()).unwrap();
        let stamp = "2026-11-15T12-00-00.123456789Z";
        let prefixed = format!("{}_{stamp}", store.history.name);
        let first = store.rename(&first, &prefixed).unwrap();
        let second = store.rename(&second, stamp).unwrap();
        let history = store.list().unwrap();
        assert_eq!(history.len(), 2);
        for (path, name) in [(&first, prefixed.as_str()), (&second, stamp)] {
            let state = history.iter().find(|state| &state.path == path).unwrap();
            assert_eq!(state.name, name);
            assert_eq!(store.rename(path, &state.name).unwrap(), *path);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_requires_the_selected_history_file_and_its_original_timestamp() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-delete-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let machine = Machine::demo();
        let store = StateStore::new(&root, &machine);
        fs::create_dir_all(store.quick_path().parent().unwrap()).unwrap();
        let history = store.history.directory.join("桥前.ps1state");
        fs::write(&history, b"first").unwrap();
        let selected = store.list().unwrap().pop().unwrap();

        let foreign = root.join("other-game.ps1state");
        let card = store.history.directory.join("card.mcd");
        let quick = store.quick_path();
        for protected in [&foreign, &card, &quick] {
            fs::write(protected, b"other").unwrap();
            let modified = fs::metadata(protected).unwrap().modified().unwrap();
            assert!(store.delete(protected, modified).is_err());
            assert_eq!(fs::read(protected).unwrap(), b"other");
        }
        #[cfg(unix)]
        {
            let link = store.history.directory.join("link.ps1state");
            std::os::unix::fs::symlink(&foreign, &link).unwrap();
            let modified = fs::symlink_metadata(&link).unwrap().modified().unwrap();
            assert!(store.delete(&link, modified).is_err());
            assert!(link.is_symlink());
        }

        fs::remove_file(&history).unwrap();
        fs::write(&history, b"replacement").unwrap();
        File::options()
            .write(true)
            .open(&history)
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        assert!(store.delete(&history, selected.modified).is_err());
        assert_eq!(fs::read(&history).unwrap(), b"replacement");
        let replacement = store.list().unwrap().pop().unwrap();
        store
            .delete(&replacement.path, replacement.modified)
            .unwrap();
        assert_eq!(store.list().unwrap(), []);
        assert!(
            store
                .delete(&replacement.path, replacement.modified)
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_preserves_state_and_save_order_without_overwriting() {
        let root = std::env::temp_dir().join(format!(
            "eidoryn-rename-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let machine = Machine::demo();
        let store = StateStore::new(&root, &machine);
        let original = store.save(&machine, &PadInput::default()).unwrap();
        let newest = store.save(&machine, &PadInput::default()).unwrap();
        File::options()
            .write(true)
            .open(&original)
            .unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(100))
            .unwrap();
        let before = fs::read(&original).unwrap();
        let renamed = store.rename(&original, "关卡一_入口").unwrap();
        assert!(!original.exists());
        assert_eq!(renamed.file_name().unwrap(), "关卡一_入口.ps1state");
        assert_eq!(fs::read(&renamed).unwrap(), before);
        assert_eq!(store.list().unwrap()[0].path, newest);
        assert_eq!(store.list().unwrap()[1].name, "关卡一_入口");
        assert!(store.load(&renamed).is_ok());
        assert_eq!(store.rename(&renamed, "关卡一_入口").unwrap(), renamed);
        let newest_bytes = fs::read(&newest).unwrap();
        assert!(
            store
                .rename(&newest, "关卡一_入口")
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(fs::read(&newest).unwrap(), newest_bytes);
        assert_eq!(fs::read(&renamed).unwrap(), before);
        for invalid in [
            "",
            "  ",
            "..",
            "../outside",
            "a/b",
            "a\\b",
            "NUL",
            "CON.txt",
            "LPT9",
            "COM¹",
            "bad:name",
            "bad.",
            "bad\nname",
        ] {
            assert!(
                store.rename(&renamed, invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(store.rename(&renamed, &"汉".repeat(81)).is_err());
        let foreign = root.join("foreign.ps1state");
        fs::write(&foreign, &before).unwrap();
        assert!(store.rename(&foreign, "out-of-game").is_err());
        assert!(foreign.exists());
        #[cfg(unix)]
        {
            let link = store.history.directory.join("symlink.ps1state");
            std::os::unix::fs::symlink(&foreign, &link).unwrap();
            assert!(store.rename(&link, "linked").is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}

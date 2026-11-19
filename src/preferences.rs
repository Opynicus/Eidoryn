//! Desktop preferences. CLI overrides are applied in memory, never saved implicitly.
use crate::{
    i18n::Language,
    keybindings::KeyBindings,
    systems::ps1::{
        bus::BIOS_SIZE,
        gpu::{ColorDepth, RenderScale, RendererMode},
        input::TurboConfig,
        runner::{DEFAULT_REWIND_SECONDS, MAX_REWIND_SECONDS, MIN_REWIND_SECONDS},
    },
    theme::DEFAULT_THEME_ID,
    video_filter::CrtFilter,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentDisc {
    pub path: PathBuf,
    #[serde(default)]
    pub fast_boot: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Keep the saved tag even when this build does not include that catalog.
    pub language: Option<String>,
    pub theme: String,
    pub theme_plugin: Option<PathBuf>,
    pub crt: String,
    pub volume: u8,
    pub muted: bool,
    pub renderer: String,
    /// Preferred hardware raster resolution, retained in software mode.
    pub render_scale: RenderScale,
    /// Preferred hardware presentation color depth, retained in software mode.
    pub color_depth: ColorDepth,
    pub clean_fullscreen: bool,
    pub turbo_buttons: u16,
    pub turbo_hz: u8,
    pub keybindings: KeyBindings,
    pub bios_path: Option<PathBuf>,
    pub recent_discs: Vec<RecentDisc>,
    pub rewind_enabled: bool,
    pub rewind_seconds: u16,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: None,
            theme: DEFAULT_THEME_ID.into(),
            theme_plugin: None,
            crt: "off".into(),
            volume: 75,
            muted: false,
            renderer: "serial".into(),
            render_scale: RenderScale::Native,
            color_depth: ColorDepth::Native,
            clean_fullscreen: true,
            turbo_buttons: 0,
            turbo_hz: TurboConfig::default().hz(),
            keybindings: KeyBindings::default(),
            bios_path: None,
            recent_discs: Vec::new(),
            rewind_enabled: false,
            rewind_seconds: DEFAULT_REWIND_SECONDS,
        }
    }
}

impl Preferences {
    /// Record successfully opened media, with the most recent choice first.
    pub fn remember_disc(&mut self, path: &Path, fast_boot: bool) -> Result<(), String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !path.is_file() {
            return Err(format!("Disc is not a regular file: {}", path.display()));
        }
        self.recent_discs.retain(|disc| disc.path != path);
        self.recent_discs.insert(0, RecentDisc { path, fast_boot });
        self.recent_discs.truncate(10);
        Ok(())
    }

    /// Valid defaults become canonical paths; missing defaults are cleared.
    /// Temporary read failures retain the user's choice.
    /// The caller persists the configuration if `bios_path` changed.
    pub fn validate_saved_bios(&mut self) -> Result<Option<PathBuf>, String> {
        let Some(path) = self.bios_path.clone() else {
            return Ok(None);
        };
        let check = || -> std::io::Result<PathBuf> {
            let metadata = std::fs::metadata(&path)?;
            if !metadata.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "not a regular file",
                ));
            }
            if metadata.len() != BIOS_SIZE as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "BIOS must be exactly 512 KiB",
                ));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)?
                .take(BIOS_SIZE as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() != BIOS_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "BIOS must be exactly 512 KiB",
                ));
            }
            path.canonicalize()
        };
        match check() {
            Ok(path) => {
                self.bios_path = Some(path.clone());
                Ok(Some(path))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.bios_path = None;
                Err(format!(
                    "Saved BIOS {} is missing or is not a regular file; its default setting was removed. Select a BIOS again.",
                    path.display()
                ))
            }
            Err(error) => Err(format!("Cannot use saved BIOS {}: {error}", path.display())),
        }
    }

    pub fn language(&self) -> Language {
        self.language
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(Language::preferred)
    }

    pub fn crt(&self) -> CrtFilter {
        self.crt.parse().unwrap_or_default()
    }

    pub fn renderer(&self) -> RendererMode {
        self.renderer.parse().unwrap_or_default()
    }

    pub fn turbo(&self) -> Result<TurboConfig, String> {
        TurboConfig::new(self.turbo_buttons, self.turbo_hz)
    }

    fn validate(&self) -> Result<(), String> {
        self.crt.parse::<CrtFilter>()?;
        self.renderer.parse::<RendererMode>()?;
        self.turbo()?;
        self.keybindings.validate()?;
        if self.volume > 100 || self.theme.is_empty() {
            return Err("Volume must be 0..100 and theme must not be empty".into());
        }
        if !(MIN_REWIND_SECONDS..=MAX_REWIND_SECONDS).contains(&self.rewind_seconds) {
            return Err(format!(
                "Rewind duration must be {MIN_REWIND_SECONDS}..{MAX_REWIND_SECONDS} seconds"
            ));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(65_537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65_536 {
            return Err("Preferences exceed 64 KiB".into());
        }
        let mut settings: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        settings.recent_discs.truncate(10);
        settings.validate()?;
        Ok(settings)
    }

    /// Replace only after the complete new file is written. A failed save keeps the old file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temporary = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::File::create_new(&temporary).map_err(|e| e.to_string())?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|e| e.to_string())
    }
}

pub fn config_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    root.map(|root| root.join("eidoryn/settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_language_survives_loading_and_saving_other_preferences() {
        let path = std::env::temp_dir().join(format!("eidoryn-locale-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"language":"qaa-x-eidoryn","volume":42}"#).unwrap();
        let mut settings = Preferences::load(&path).unwrap();
        assert_eq!(settings.language(), Language::preferred());
        assert_eq!(settings.volume, 42);
        settings.muted = true;
        settings.save(&path).unwrap();
        let reloaded = Preferences::load(&path).unwrap();
        assert_eq!(reloaded.language.as_deref(), Some("qaa-x-eidoryn"));
        assert!(reloaded.muted);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn recent_discs_are_persistent_unique_and_bounded() {
        let root = std::env::temp_dir().join(format!("eidoryn-recents-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut preferences = Preferences::default();
        for index in 0..12 {
            let path = root.join(format!("game-{index}.iso"));
            std::fs::write(&path, []).unwrap();
            preferences.remember_disc(&path, false).unwrap();
        }
        assert_eq!(preferences.recent_discs.len(), 10);
        let reused = root.join("game-7.iso").canonicalize().unwrap();
        preferences.remember_disc(&reused, true).unwrap();
        assert_eq!(preferences.recent_discs[0].path, reused);
        assert!(preferences.recent_discs[0].fast_boot);
        assert_eq!(
            preferences
                .recent_discs
                .iter()
                .filter(|disc| disc.path == reused)
                .count(),
            1
        );
        let settings = root.join("settings.json");
        preferences.save(&settings).unwrap();
        assert_eq!(Preferences::load(&settings).unwrap(), preferences);
        assert!(
            preferences
                .remember_disc(&root.join("missing.iso"), false)
                .is_err()
        );
        assert_eq!(Preferences::load(&settings).unwrap(), preferences);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_bios_is_cleared_but_invalid_contents_keep_the_setting() {
        let root = std::env::temp_dir().join(format!("eidoryn-bios-check-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bios = root.join("bios.bin");
        let mut preferences = Preferences {
            bios_path: Some(bios.clone()),
            ..Preferences::default()
        };
        assert!(
            preferences
                .validate_saved_bios()
                .unwrap_err()
                .contains("removed")
        );
        assert!(preferences.bios_path.is_none());
        std::fs::write(&bios, [0; 8]).unwrap();
        preferences.bios_path = Some(bios.clone());
        assert!(
            preferences
                .validate_saved_bios()
                .unwrap_err()
                .contains("512 KiB")
        );
        assert_eq!(preferences.bios_path, Some(bios.clone()));
        std::fs::write(&bios, vec![0; BIOS_SIZE]).unwrap();
        preferences.bios_path = Some(root.join(".").join("bios.bin"));
        let canonical = bios.canonicalize().unwrap();
        assert_eq!(
            preferences.validate_saved_bios().unwrap(),
            Some(canonical.clone())
        );
        assert_eq!(
            preferences.bios_path.as_deref().unwrap().as_os_str(),
            canonical.as_os_str()
        );
        preferences.bios_path = Some(root.clone());
        assert!(
            preferences
                .validate_saved_bios()
                .unwrap_err()
                .contains("removed")
        );
        assert!(preferences.bios_path.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_settings_default_to_native_color_and_preserve_resolution() {
        let legacy: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.render_scale, RenderScale::Native);
        assert_eq!(legacy.color_depth, ColorDepth::Native);
        let legacy: Preferences = serde_json::from_str(r#"{"render_scale":"4x"}"#).unwrap();
        assert_eq!(legacy.render_scale, RenderScale::X4);
        assert_eq!(legacy.color_depth, ColorDepth::Native);
    }

    #[test]
    fn hardware_resolution_and_color_depth_round_trip_independently() {
        for (scale, id) in [
            (RenderScale::Native, "1x"),
            (RenderScale::X2, "2x"),
            (RenderScale::X4, "4x"),
        ] {
            for (depth, depth_id) in [
                (ColorDepth::Native, "native"),
                (ColorDepth::TrueColor, "true-color"),
            ] {
                let preferences = Preferences {
                    render_scale: scale,
                    color_depth: depth,
                    ..Preferences::default()
                };
                let json = serde_json::to_value(&preferences).unwrap();
                assert_eq!(json["render_scale"], id);
                assert_eq!(json["color_depth"], depth_id);
                assert_eq!(
                    serde_json::from_value::<Preferences>(json).unwrap(),
                    preferences
                );
            }
        }
    }

    #[test]
    fn rendering_ids_preserve_json_values_and_parse_back() {
        for (renderer, renderer_id) in [
            (RendererMode::Serial, "serial"),
            (RendererMode::Threaded, "threaded"),
            (RendererMode::Vulkan, "vulkan"),
            (RendererMode::OpenGl, "opengl"),
            (RendererMode::Hardware, "hardware"),
            (RendererMode::Dx12, "dx12"),
            (RendererMode::Metal, "metal"),
        ] {
            for (crt, crt_id) in [
                (CrtFilter::Off, "off"),
                (CrtFilter::Soft, "soft"),
                (CrtFilter::Classic, "classic"),
            ] {
                let mut settings = Preferences::default();
                renderer.id().clone_into(&mut settings.renderer);
                crt.id().clone_into(&mut settings.crt);
                let json = serde_json::to_value(&settings).unwrap();
                assert_eq!(json["renderer"], renderer_id);
                assert_eq!(json["crt"], crt_id);
                let restored: Preferences = serde_json::from_value(json).unwrap();
                restored.validate().unwrap();
                assert_eq!(restored.renderer(), renderer);
                assert_eq!(restored.crt(), crt);
            }
        }
    }

    #[test]
    fn preferences_round_trip_and_reject_invalid_updates() {
        let path =
            std::env::temp_dir().join(format!("eidoryn-settings-{}.json", std::process::id()));
        let mut settings = Preferences {
            language: Some("zh-CN".into()),
            theme: "ocean".into(),
            crt: "classic".into(),
            volume: 42,
            renderer: "threaded".into(),
            theme_plugin: Some(PathBuf::from("themes/海洋.json")),
            clean_fullscreen: true,
            turbo_buttons: 0x8400,
            turbo_hz: 15,
            rewind_enabled: true,
            rewind_seconds: 90,
            ..Preferences::default()
        };
        settings
            .keybindings
            .rebind(crate::keybindings::PadButton::Cross, "j")
            .unwrap();
        settings.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), settings);
        settings.volume = 101;
        assert!(settings.save(&path).is_err());
        assert_eq!(Preferences::load(&path).unwrap().volume, 42);
        settings.volume = 42;
        for seconds in [MIN_REWIND_SECONDS - 1, MAX_REWIND_SECONDS + 1] {
            settings.rewind_seconds = seconds;
            assert!(settings.save(&path).is_err());
            assert_eq!(Preferences::load(&path).unwrap().rewind_seconds, 90);
        }
        settings.rewind_seconds = 90;
        std::fs::write(&path, b"{\"keybindings\":{\"cross\":\"z\"}}").unwrap();
        assert!(Preferences::load(&path).is_err());
        settings.save(&path).unwrap();
        settings.turbo_hz = 0;
        assert!(settings.save(&path).is_err());
        assert_eq!(Preferences::load(&path).unwrap().turbo_hz, 15);
        std::fs::write(&path, b"{broken").unwrap();
        assert!(Preferences::load(&path).is_err());
        std::fs::write(&path, b"{\"theme\":\"forest\"}").unwrap();
        assert_eq!(Preferences::load(&path).unwrap().volume, 75);
        assert_eq!(
            Preferences::load(&path).unwrap().keybindings,
            KeyBindings::default()
        );
        assert_eq!(
            Preferences::load(&path).unwrap().turbo().unwrap(),
            TurboConfig::default()
        );
        assert!(Preferences::load(&path).unwrap().bios_path.is_none());
        assert_eq!(Preferences::load(&path).unwrap().recent_discs, []);
        assert!(!Preferences::load(&path).unwrap().rewind_enabled);
        assert_eq!(Preferences::load(&path).unwrap().rewind_seconds, 30);
        std::fs::write(&path, b"{\"rewind_enabled\":true}").unwrap();
        assert!(Preferences::load(&path).unwrap().rewind_enabled);
        assert_eq!(Preferences::load(&path).unwrap().rewind_seconds, 30);
        for (seconds, valid) in [(4, false), (5, true), (120, true), (121, false)] {
            std::fs::write(&path, format!("{{\"rewind_seconds\":{seconds}}}")).unwrap();
            assert_eq!(Preferences::load(&path).is_ok(), valid);
        }
        std::fs::write(&path, b"{\"turbo_buttons\":32}").unwrap();
        assert!(Preferences::load(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
        assert!(!Preferences::default().rewind_enabled);
        assert_eq!(Preferences::default().rewind_seconds, 30);
    }
}

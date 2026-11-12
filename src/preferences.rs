//! Desktop preferences. CLI overrides are applied in memory, never saved implicitly.
use crate::{
    i18n::Language,
    systems::ps1::{gpu::RendererMode, input::TurboConfig},
    video_filter::CrtFilter,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub language: Option<String>,
    pub theme: String,
    pub theme_plugin: Option<PathBuf>,
    pub crt: String,
    pub volume: u8,
    pub muted: bool,
    pub renderer: String,
    pub clean_fullscreen: bool,
    pub turbo_buttons: u16,
    pub turbo_hz: u8,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: None,
            theme: "midnight".into(),
            theme_plugin: None,
            crt: "off".into(),
            volume: 75,
            muted: false,
            renderer: "serial".into(),
            clean_fullscreen: false,
            turbo_buttons: 0,
            turbo_hz: TurboConfig::default().hz(),
        }
    }
}

impl Preferences {
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
        if let Some(language) = &self.language {
            language.parse::<Language>()?;
        }
        self.crt.parse::<CrtFilter>()?;
        self.renderer.parse::<RendererMode>()?;
        self.turbo()?;
        if self.volume > 100 || self.theme.is_empty() {
            return Err("Volume must be 0..100 and theme must not be empty".into());
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
        let settings: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
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
            ..Preferences::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), settings);
        settings.volume = 101;
        assert!(settings.save(&path).is_err());
        assert_eq!(Preferences::load(&path).unwrap().volume, 42);
        settings.volume = 42;
        settings.turbo_hz = 0;
        assert!(settings.save(&path).is_err());
        assert_eq!(Preferences::load(&path).unwrap().turbo_hz, 15);
        std::fs::write(&path, b"{broken").unwrap();
        assert!(Preferences::load(&path).is_err());
        std::fs::write(&path, b"{\"theme\":\"forest\"}").unwrap();
        assert_eq!(Preferences::load(&path).unwrap().volume, 75);
        assert_eq!(
            Preferences::load(&path).unwrap().turbo().unwrap(),
            TurboConfig::default()
        );
        std::fs::write(&path, b"{\"turbo_buttons\":32}").unwrap();
        assert!(Preferences::load(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
    }
}

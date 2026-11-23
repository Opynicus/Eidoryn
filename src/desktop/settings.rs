use super::{Desktop, Options};
use eidoryn_psx::{
    i18n::{Language, Text},
    preferences::{Preferences, config_path},
    systems::ps1::{gpu::RendererMode, machine::Machine},
    theme::ThemeCatalog,
    video_filter::CrtFilter,
};
use std::{collections::HashMap, path::PathBuf};

pub(super) struct Startup {
    pub preferences: Preferences,
    pub path: Option<PathBuf>,
    pub themes: ThemeCatalog,
    pub theme_paths: HashMap<String, PathBuf>,
    pub notice: Option<String>,
    pub language: Language,
    pub volume: u8,
    pub muted: bool,
    pub crt: CrtFilter,
    pub clean_fullscreen: bool,
    pub bios: Option<PathBuf>,
    pub gba_bios: Option<PathBuf>,
}

pub(super) fn load(machine: &mut Machine, options: &Options) -> Result<Startup, String> {
    load_at(machine, options, config_path())
}

fn load_at(
    machine: &mut Machine,
    options: &Options,
    path: Option<PathBuf>,
) -> Result<Startup, String> {
    let mut errors = Vec::new();
    let mut preferences = path.as_deref().map_or_else(Preferences::default, |path| {
        Preferences::load(path).unwrap_or_else(|error| {
            errors.push(format!("{}: {error}", path.display()));
            Preferences::default()
        })
    });
    let previous_bios = preferences.bios_path.clone();
    let previous_gba = preferences.gba_bios_path.clone();
    let gba_bios =
        eidoryn_psx::preferences::validate_gba_bios_choice(&mut preferences.gba_bios_path)
            .unwrap_or_else(|error| {
                errors.push(error);
                None
            });
    let saved_bios = match preferences.validate_saved_bios() {
        Ok(bios) => bios,
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let changed = |a: &Option<PathBuf>, b: &Option<PathBuf>| {
        a.as_ref().map(|p| p.as_os_str()) != b.as_ref().map(|p| p.as_os_str())
    };
    if (changed(&preferences.bios_path, &previous_bios)
        || changed(&preferences.gba_bios_path, &previous_gba))
        && let Some(path) = &path
        && let Err(error) = preferences.save(path)
    {
        errors.push(format!(
            "Cannot update saved BIOS in {}: {error}",
            path.display()
        ));
    }
    let bios = options.bios.clone().or(saved_bios);
    let mut themes = ThemeCatalog::default();
    errors.extend(themes.discover(std::path::Path::new("plugins/themes")));
    let mut theme_paths = HashMap::new();
    if let Some(path) = &preferences.theme_plugin {
        match themes.load_plugin(path) {
            Ok(id) => {
                theme_paths.insert(id, path.clone());
            }
            Err(error) => errors.push(error),
        }
    }
    if let Some(path) = &options.theme_plugin {
        let id = themes.load_plugin(path)?;
        themes.select(&id)?;
        theme_paths.insert(id, std::path::absolute(path).map_err(|e| e.to_string())?);
    } else if options.theme.is_none()
        && let Err(error) = themes.select(&preferences.theme)
    {
        errors.push(error);
    }
    if let Some(id) = &options.theme {
        themes.select(id)?;
    }
    if let Err(error) = machine.bus.gpu.set_renderer_config(
        options.renderer.unwrap_or_else(|| preferences.renderer()),
        preferences.render_scale,
        preferences.color_depth,
    ) {
        if options.renderer.is_some() {
            return Err(format!("Cannot start GPU renderer: {error}"));
        }
        machine
            .bus
            .gpu
            .set_renderer(RendererMode::Serial)
            .map_err(|e| e.to_string())?;
        errors.push(format!(
            "Cannot restore preferred renderer: {error}. Using serial software rendering."
        ));
    }
    machine.bus.gpu.set_anti_aliasing(preferences.anti_aliasing);
    Ok(Startup {
        bios,
        gba_bios,
        language: options.language.unwrap_or_else(|| preferences.language()),
        volume: options.volume.unwrap_or(preferences.volume).min(100),
        muted: options.muted.unwrap_or(preferences.muted),
        crt: options.crt.unwrap_or_else(|| preferences.crt()),
        clean_fullscreen: options
            .clean_fullscreen
            .unwrap_or(preferences.clean_fullscreen),
        preferences,
        path,
        themes,
        theme_paths,
        notice: (!errors.is_empty()).then(|| errors.join("\n")),
    })
}

impl Desktop {
    pub(super) fn save_preferences(&mut self) {
        let result = self
            .preferences_path
            .as_deref()
            .ok_or_else(|| "No configuration directory available".to_owned())
            .and_then(|path| self.preferences.save(path));
        if let Err(error) = result {
            self.notice = Some(format!("{}: {error}", self.tr(Text::ConfigError)));
        }
    }

    pub(super) fn select_theme(&mut self, id: &str, cx: &mut gpui::Context<Self>) {
        match self.themes.select(id) {
            Ok(()) => {
                id.clone_into(&mut self.preferences.theme);
                self.preferences.theme_plugin = self.theme_paths.get(id).cloned();
                self.save_preferences();
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    pub(super) fn persist_volume(&mut self) {
        self.preferences.volume = self.audio.volume;
        self.preferences.muted = self.audio.muted;
        self.save_preferences();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eidoryn_psx::systems::ps1::gpu::{AntiAliasing, ColorDepth, RenderScale};

    #[test]
    fn startup_validates_the_gba_default_independently_of_ps1() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("settings.json");
        let gba = root.path().join("gba.bin");
        let ps1 = root.path().join("ps1.bin");
        let mut firmware = vec![0; eidoryn_gba::BIOS_LEN];
        firmware[..4].copy_from_slice(&0xeaff_fffe_u32.to_le_bytes()); // reset: B .
        std::fs::write(&gba, firmware).unwrap();
        std::fs::write(&ps1, vec![0; eidoryn_psx::bus::BIOS_SIZE]).unwrap();
        let preferences = Preferences {
            bios_path: Some(ps1.clone()),
            gba_bios_path: Some(gba.clone()),
            ..Preferences::default()
        };
        preferences.save(&config).unwrap();
        let mut machine = Machine::demo();
        let mut restart =
            || load_at(&mut machine, &Options::default(), Some(config.clone())).unwrap();
        assert_eq!(restart().gba_bios, Some(gba.canonicalize().unwrap()));
        std::fs::write(&gba, [0; 16]).unwrap();
        let invalid = restart();
        assert!(invalid.notice.as_deref().unwrap().contains("16 KiB"));
        assert_eq!(invalid.bios, Some(ps1.canonicalize().unwrap()));
        assert!(invalid.gba_bios.is_none());
        assert_eq!(
            Preferences::load(&config).unwrap().gba_bios_path,
            Some(gba.clone())
        );
        std::fs::remove_file(&gba).unwrap();
        assert!(restart().gba_bios.is_none());
        let persisted = Preferences::load(&config).unwrap();
        assert!(persisted.gba_bios_path.is_none());
        assert_eq!(persisted.bios_path, Some(ps1.canonicalize().unwrap()));
    }

    #[test]
    fn saved_bios_is_selected_and_missing_defaults_are_removed_on_restart() {
        let root =
            std::env::temp_dir().join(format!("eidoryn-bios-startup-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bios = root.join("bios.bin");
        let config = root.join("settings.json");
        std::fs::write(&bios, vec![0; eidoryn_psx::bus::BIOS_SIZE]).unwrap();
        let bios = bios.canonicalize().unwrap();
        let mut saved = Preferences {
            bios_path: Some(root.join(".").join("bios.bin")),
            theme: "forest".into(),
            ..Preferences::default()
        };
        saved.save(&config).unwrap();
        saved.bios_path = Some(bios.clone());
        let mut machine = Machine::demo();
        let startup = load_at(&mut machine, &Options::default(), Some(config.clone())).unwrap();
        assert_eq!(startup.bios, Some(bios.clone()));
        assert!(startup.notice.is_none());
        assert_eq!(
            startup
                .preferences
                .bios_path
                .as_deref()
                .unwrap()
                .as_os_str(),
            bios.as_os_str()
        );
        assert_eq!(
            Preferences::load(&config)
                .unwrap()
                .bios_path
                .as_deref()
                .unwrap()
                .as_os_str(),
            bios.as_os_str()
        );
        assert_eq!(Preferences::load(&config).unwrap(), saved);
        let override_path = root.join("override.bin");
        let startup = load_at(
            &mut machine,
            &Options {
                bios: Some(override_path.clone()),
                ..Options::default()
            },
            Some(config.clone()),
        )
        .unwrap();
        assert_eq!(startup.bios, Some(override_path));
        assert_eq!(Preferences::load(&config).unwrap(), saved);
        std::fs::remove_file(bios).unwrap();
        let startup = load_at(&mut machine, &Options::default(), Some(config.clone())).unwrap();
        assert!(startup.bios.is_none());
        assert!(startup.notice.as_deref().unwrap().contains("removed"));
        let restored = Preferences::load(&config).unwrap();
        assert!(restored.bios_path.is_none());
        assert_eq!(restored.theme, "forest");
        let startup = load_at(&mut machine, &Options::default(), Some(config)).unwrap();
        assert!(startup.bios.is_none());
        assert!(startup.notice.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_saved_renderer_falls_back_but_explicit_override_fails() {
        for (mode, supported) in [
            (RendererMode::Vulkan, cfg!(feature = "vulkan")),
            (RendererMode::OpenGl, cfg!(feature = "opengl")),
            (
                RendererMode::Dx12,
                cfg!(all(feature = "dx12", target_os = "windows")),
            ),
            (
                RendererMode::Metal,
                cfg!(all(feature = "metal", target_os = "macos")),
            ),
        ] {
            if supported {
                continue;
            }
            let path = std::env::temp_dir().join(format!(
                "eidoryn-fallback-{}-{}.json",
                std::process::id(),
                mode.id()
            ));
            Preferences {
                renderer: mode.id().into(),
                ..Preferences::default()
            }
            .save(&path)
            .unwrap();
            let mut machine = Machine::demo();
            let startup = load_at(&mut machine, &Options::default(), Some(path.clone())).unwrap();
            assert_eq!(machine.bus.gpu.renderer_mode(), RendererMode::Serial);
            assert!(startup.notice.is_some());
            assert_eq!(Preferences::load(&path).unwrap().renderer, mode.id());
            assert!(
                load_at(
                    &mut machine,
                    &Options {
                        renderer: Some(mode),
                        ..Options::default()
                    },
                    Some(path.clone())
                )
                .is_err()
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn cli_overrides_saved_preferences_without_rewriting_them() {
        let path =
            std::env::temp_dir().join(format!("eidoryn-startup-{}.json", std::process::id()));
        let mut machine = Machine::demo();
        let fresh = load_at(&mut machine, &Options::default(), None).unwrap();
        assert_eq!(fresh.themes.active().id, "midnight");
        assert_eq!(fresh.preferences.theme, fresh.themes.active().id);
        let mut saved = Preferences {
            theme: "ocean".into(),
            language: Some("zh-CN".into()),
            crt: "classic".into(),
            volume: 42,
            muted: true,
            renderer: "threaded".into(),
            render_scale: RenderScale::X2,
            color_depth: ColorDepth::TrueColor,
            anti_aliasing: AntiAliasing::Fxaa,
            clean_fullscreen: true,
            ..Preferences::default()
        };
        saved
            .keybindings
            .rebind(eidoryn_psx::keybindings::PadButton::Cross, "j")
            .unwrap();
        saved.save(&path).unwrap();
        let startup = load_at(&mut machine, &Options::default(), Some(path.clone())).unwrap();
        assert_eq!(startup.themes.active().id, "ocean");
        assert_eq!(startup.language, Language::ZhCn);
        assert_eq!(startup.crt, CrtFilter::Classic);
        assert_eq!(startup.volume, 42);
        assert!(startup.muted && startup.clean_fullscreen);
        assert_eq!(startup.preferences.keybindings, saved.keybindings);
        assert_eq!(machine.bus.gpu.renderer_mode(), RendererMode::Threaded);
        assert_eq!(machine.bus.gpu.color_depth(), ColorDepth::Native);
        assert_eq!(machine.bus.gpu.anti_aliasing(), AntiAliasing::Off);
        assert_eq!(
            machine.bus.gpu.preferred_anti_aliasing(),
            AntiAliasing::Fxaa
        );
        assert_eq!(startup.preferences.color_depth, ColorDepth::TrueColor);
        let options = Options {
            theme: Some("forest".into()),
            language: Some(Language::En),
            crt: Some(CrtFilter::Off),
            volume: Some(80),
            muted: Some(false),
            renderer: Some(RendererMode::Serial),
            clean_fullscreen: Some(false),
            ..Options::default()
        };
        let startup = load_at(&mut machine, &options, Some(path.clone())).unwrap();
        assert_eq!(startup.themes.active().id, "forest");
        assert_eq!(startup.language, Language::En);
        assert_eq!(startup.crt, CrtFilter::Off);
        assert_eq!(startup.volume, 80);
        assert!(!startup.muted && !startup.clean_fullscreen);
        assert_eq!(machine.bus.gpu.renderer_mode(), RendererMode::Serial);
        assert_eq!(Preferences::load(&path).unwrap(), saved);
        std::fs::write(&path, b"invalid").unwrap();
        let startup = load_at(&mut machine, &Options::default(), Some(path.clone())).unwrap();
        assert!(startup.notice.is_some());
        assert_eq!(startup.themes.active().id, "midnight");
        std::fs::remove_file(path).unwrap();
    }
}

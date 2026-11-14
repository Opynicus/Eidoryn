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
    let preferences = path.as_deref().map_or_else(Preferences::default, |path| {
        Preferences::load(path).unwrap_or_else(|error| {
            errors.push(format!("{}: {error}", path.display()));
            Preferences::default()
        })
    });
    let mut themes = ThemeCatalog::default();
    themes.select("midnight")?;
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
    if let Err(error) = machine
        .bus
        .gpu
        .set_renderer(options.renderer.unwrap_or_else(|| preferences.renderer()))
    {
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
    Ok(Startup {
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
        let mut saved = Preferences {
            theme: "ocean".into(),
            language: Some("zh-CN".into()),
            crt: "classic".into(),
            volume: 42,
            muted: true,
            renderer: "threaded".into(),
            clean_fullscreen: true,
            ..Preferences::default()
        };
        saved
            .keybindings
            .rebind(eidoryn_psx::keybindings::PadButton::Cross, "j")
            .unwrap();
        saved.save(&path).unwrap();
        let mut machine = Machine::demo();
        let startup = load_at(&mut machine, &Options::default(), Some(path.clone())).unwrap();
        assert_eq!(startup.themes.active().id, "ocean");
        assert_eq!(startup.language, Language::ZhCn);
        assert_eq!(startup.crt, CrtFilter::Classic);
        assert_eq!(startup.volume, 42);
        assert!(startup.muted && startup.clean_fullscreen);
        assert_eq!(startup.preferences.keybindings, saved.keybindings);
        assert_eq!(machine.bus.gpu.renderer_mode(), RendererMode::Threaded);
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

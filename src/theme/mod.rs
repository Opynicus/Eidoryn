//! Built-in themes and data-only theme plugins. The emulation core has no theme dependency.
mod palette;
mod plugin;

pub use palette::Palette;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub palette: Palette,
}

pub fn builtins() -> Vec<Theme> {
    [
        ("lavender", "Lavender", Palette::LAVENDER),
        ("midnight", "Midnight", Palette::MIDNIGHT),
        ("ocean", "Ocean", Palette::OCEAN),
        ("forest", "Forest", Palette::FOREST),
    ]
    .into_iter()
    .map(|(id, name, palette)| Theme {
        id: id.into(),
        name: name.into(),
        palette,
    })
    .collect()
}

pub struct ThemeCatalog {
    themes: Vec<Theme>,
    selected: usize,
}

impl Default for ThemeCatalog {
    fn default() -> Self {
        Self {
            themes: builtins(),
            selected: 0,
        }
    }
}

impl ThemeCatalog {
    pub fn active(&self) -> &Theme {
        &self.themes[self.selected]
    }

    pub fn themes(&self) -> &[Theme] {
        &self.themes
    }

    pub fn select(&mut self, id: &str) -> Result<(), String> {
        self.selected = self
            .themes
            .iter()
            .position(|theme| theme.id == id)
            .ok_or_else(|| format!("Unknown theme: {id}"))?;
        Ok(())
    }

    /// Validate before replacing a plugin, keeping the current theme on failure.
    pub fn load_plugin(&mut self, path: &Path) -> Result<String, String> {
        let theme = plugin::load(path)?;
        let id = theme.id.clone();
        if let Some(index) = self.themes.iter().position(|entry| entry.id == id) {
            self.themes[index] = theme;
        } else {
            self.themes.push(theme);
        }
        Ok(id)
    }

    /// Discover local theme plugins in deterministic order, reporting invalid files.
    pub fn discover(&mut self, directory: &Path) -> Vec<String> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => return vec![format!("{}: {error}", directory.display())],
        };
        let mut errors = Vec::new();
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "json") => {
                    paths.push(entry.path());
                }
                Ok(_) => {}
                Err(error) => errors.push(error.to_string()),
            }
        }
        paths.sort();
        for path in paths {
            if let Err(error) = self.load_plugin(&path) {
                errors.push(error);
            }
        }
        errors
    }
}

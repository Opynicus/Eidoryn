use super::{BUILTINS, Theme};
use serde::Deserialize;
use std::{collections::BTreeMap, io::Read, path::Path};

const MAX_PLUGIN_BYTES: u64 = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    api_version: u32,
    kind: String,
    id: String,
    name: String,
    base: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

pub(super) fn load(path: &Path) -> Result<Theme, String> {
    let read = || -> Result<Theme, String> {
        if !std::fs::metadata(path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err("Select a regular theme JSON file".into());
        }
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Select a regular theme JSON file".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_PLUGIN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_PLUGIN_BYTES {
            return Err("Theme plugin exceeds 64 KiB".into());
        }
        parse(&bytes)
    };
    read().map_err(|error| format!("{}: {error}", path.display()))
}

fn parse(bytes: &[u8]) -> Result<Theme, String> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if manifest.api_version != 1 || manifest.kind != "theme" {
        return Err("Expected a theme plugin with api_version 1".into());
    }
    if manifest.id.is_empty()
        || manifest.id.len() > 64
        || !manifest
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Theme id must contain 1–64 lowercase letters, digits or hyphens".into());
    }
    if manifest.name.trim().is_empty()
        || manifest.name.chars().count() > 32
        || manifest.name.chars().any(char::is_control)
    {
        return Err("Theme name must contain 1–32 visible characters".into());
    }
    if BUILTINS.iter().any(|theme| theme.id == manifest.id) {
        return Err("Theme plugin id must differ from built-in theme ids".into());
    }
    let mut palette = BUILTINS
        .iter()
        .find(|theme| theme.id == manifest.base)
        .ok_or_else(|| format!("Unknown base theme: {}", manifest.base))?
        .palette;
    for (key, value) in manifest.colors {
        let hex = value
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("Color {key} must use #RRGGBB"))?;
        let color = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
        palette.set(&key, color)?;
    }
    Ok(Theme {
        id: manifest.id,
        name: manifest.name.trim().into(),
        palette,
    })
}

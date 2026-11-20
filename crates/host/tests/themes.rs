#![cfg(feature = "themes")]
use eidoryn_host::theme::{Palette, ThemeCatalog};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "eidoryn-theme-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plugin() -> Value {
    json!({ "api_version": 1, "kind": "theme", "id": "custom", "name": "Custom",
        "base": "midnight", "colors": { "primary": "#ABCDEF" } })
}

#[test]
fn themes_switch_and_plugins_inherit_then_reload_without_duplicate_entries() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
    let builtins = catalog.themes().len();
    assert_eq!(catalog.active().id, "midnight");
    assert_eq!(catalog.active().palette, Palette::MIDNIGHT);
    for id in ["lavender", "midnight", "ocean", "forest", "lemon-green"] {
        catalog.select(id).unwrap();
        assert_eq!(catalog.active().id, id);
    }
    let mut manifest = plugin();
    let path = fixture.write("theme.json", &manifest);
    let id = catalog.load_plugin(&path).unwrap();
    catalog.select(&id).unwrap();
    assert_eq!(catalog.active().palette.primary, 0x00ab_cdef);
    assert_eq!(
        catalog.active().palette.background,
        Palette::MIDNIGHT.background
    );
    manifest["colors"]["primary"] = json!("#123456");
    fixture.write("theme.json", &manifest);
    catalog.load_plugin(&path).unwrap();
    assert_eq!(catalog.themes().len(), builtins + 1);
    assert_eq!(catalog.active().palette.primary, 0x0012_3456);
}

#[test]
fn lemon_green_exposes_the_prototype_colors_to_plugins() {
    let mut catalog = ThemeCatalog::default();
    catalog.select("lemon-green").unwrap();
    let theme = catalog.active();
    let palette = theme.palette;
    assert_eq!(theme.name, "Lemon Green");
    assert_eq!(palette.background, 0x0010_1114);
    assert_eq!(palette.primary, 0x00d3_f572);
    assert_eq!(palette.on_primary, 0x0018_2008);
    assert_eq!(palette.sidebar(), 0x0014_1519);

    let fixture = Fixture::new();
    let mut manifest = plugin();
    manifest["base"] = json!("lemon-green");
    let id = catalog
        .load_plugin(&fixture.write("theme.json", &manifest))
        .unwrap();
    catalog.select(&id).unwrap();
    let inherited = catalog.active().palette;
    assert_eq!(inherited.primary, 0x00ab_cdef);
    assert_eq!(inherited.background, palette.background);
    assert_eq!(inherited.sidebar(), palette.sidebar());
}

#[test]
fn component_colors_follow_base_overrides_and_allow_individual_customization() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
    let mut manifest = plugin();
    manifest["colors"] = json!({
        "button_surface": "#445566",
        "surface_variant": "#223344",
        "muted": "#99AABB"
    });
    let id = catalog
        .load_plugin(&fixture.write("theme.json", &manifest))
        .unwrap();
    catalog.select(&id).unwrap();
    let p = &catalog.active().palette;
    assert_eq!(p.button_surface(), 0x0044_5566);
    assert_eq!(p.settings_group(), 0x0022_3344);
    assert_eq!(p.soft_text(), 0x0099_aabb);
    assert_eq!(p.sidebar(), Palette::MIDNIGHT.sidebar());

    manifest["base"] = json!("ocean");
    manifest["colors"] = json!({});
    catalog
        .load_plugin(&fixture.write("theme.json", &manifest))
        .unwrap();
    assert_eq!(catalog.active().palette, Palette::OCEAN);
}

#[test]
fn malformed_plugins_preserve_the_selected_theme_and_catalog() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
    let builtins = catalog.themes().len();
    catalog.select("ocean").unwrap();
    for (key, value) in [
        ("api_version", json!(2)),
        ("kind", json!("script")),
        ("id", json!("ocean")),
        ("id", json!("../bad")),
        ("name", json!("\n")),
        ("base", json!("missing")),
        ("colors", json!({ "primary": "#zzzzzz" })),
        ("colors", json!({ "text": "#fff" })),
        ("colors", json!({ "typo": "#123456" })),
        ("unknown", json!(true)),
    ] {
        let mut manifest = plugin();
        manifest[key] = value;
        let path = fixture.write("invalid.json", &manifest);
        assert!(catalog.load_plugin(&path).is_err(), "{manifest}");
        assert_eq!(catalog.active().id, "ocean");
        assert_eq!(catalog.themes().len(), builtins);
    }
    assert!(catalog.select("missing").is_err());
    assert_eq!(catalog.active().id, "ocean");
}

#[test]
fn plugin_file_limits_and_discovery_report_errors_without_losing_valid_themes() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
    let builtins = catalog.themes().len();
    fixture.write("valid.json", &plugin());
    std::fs::write(fixture.0.join("broken.json"), b"{").unwrap();
    std::fs::write(fixture.0.join("notes.txt"), b"ignored").unwrap();
    assert_eq!(catalog.discover(&fixture.0).len(), 1);
    assert_eq!(catalog.themes().len(), builtins + 1);
    assert_eq!(
        catalog.discover(&fixture.0.join("missing")),
        Vec::<String>::new()
    );
    let path = fixture.0.join("oversize.json");
    std::fs::write(&path, vec![b' '; 65_537]).unwrap();
    assert!(catalog.load_plugin(&path).unwrap_err().contains("64 KiB"));
    assert!(catalog.load_plugin(&fixture.0).is_err());
}

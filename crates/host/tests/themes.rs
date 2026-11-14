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
    for id in ["lavender", "midnight", "ocean", "forest"] {
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
    assert_eq!(catalog.themes().len(), 5);
    assert_eq!(catalog.active().palette.primary, 0x0012_3456);
}

#[test]
fn malformed_plugins_preserve_the_selected_theme_and_catalog() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
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
        assert_eq!(catalog.themes().len(), 4);
    }
    assert!(catalog.select("missing").is_err());
    assert_eq!(catalog.active().id, "ocean");
}

#[test]
fn plugin_file_limits_and_discovery_report_errors_without_losing_valid_themes() {
    let fixture = Fixture::new();
    let mut catalog = ThemeCatalog::default();
    fixture.write("valid.json", &plugin());
    std::fs::write(fixture.0.join("broken.json"), b"{").unwrap();
    std::fs::write(fixture.0.join("notes.txt"), b"ignored").unwrap();
    assert_eq!(catalog.discover(&fixture.0).len(), 1);
    assert_eq!(catalog.themes().len(), 5);
    assert_eq!(
        catalog.discover(&fixture.0.join("missing")),
        Vec::<String>::new()
    );
    let path = fixture.0.join("oversize.json");
    std::fs::write(&path, vec![b' '; 65_537]).unwrap();
    assert!(catalog.load_plugin(&path).unwrap_err().contains("64 KiB"));
    assert!(catalog.load_plugin(&fixture.0).is_err());
}

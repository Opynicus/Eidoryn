#![cfg(feature = "desktop")]
use eidoryn_psx::theme::{Palette, ThemeCatalog};
use std::path::PathBuf;

#[test]
fn bundled_ember_plugin_loads_through_the_public_plugin_api() {
    let mut catalog = ThemeCatalog::default();
    let id = catalog
        .load_plugin(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/themes/ember.json"))
        .unwrap();
    catalog.select(&id).unwrap();
    assert_eq!(catalog.active().name, "Ember");
    assert_ne!(catalog.active().palette, Palette::MIDNIGHT);
}

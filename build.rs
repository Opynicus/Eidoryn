use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    env,
    error::Error,
    fmt::Write,
    fs,
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    messages: BTreeMap<String, String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=locales");
    let mut catalogs = BTreeMap::new();
    for entry in fs::read_dir("locales")? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("Invalid locale filename: {}", path.display()))?;
        let source =
            fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let catalog: Catalog =
            toml::from_str(&source).map_err(|err| format!("{}: {err}", path.display()))?;
        catalogs.insert(id.to_owned(), catalog);
    }

    let english = catalogs.remove("en").ok_or("Missing locales/en.toml")?;
    if english.messages.is_empty() {
        return Err("locales/en.toml must define the interface message keys".into());
    }
    let mut output = String::from("catalogs! {\nkeys {\n");
    for key in english.messages.keys() {
        if !key.starts_with(|c: char| c.is_ascii_uppercase())
            || !key.bytes().all(|b| b.is_ascii_alphanumeric())
            || key == "Self"
        {
            return Err(format!("locales/en.toml: invalid message key {key:?}").into());
        }
        writeln!(output, "{key},")?;
    }
    output.push_str("}\nlanguages {\n");
    let mut tags = HashSet::new();
    let mut variants = HashSet::new();
    for (id, catalog) in std::iter::once(("en", &english))
        .chain(catalogs.iter().map(|(id, catalog)| (id.as_str(), catalog)))
    {
        for tag in std::iter::once(id).chain(catalog.aliases.iter().map(String::as_str)) {
            if !valid_tag(tag) {
                return Err(format!("locales/{id}.toml: invalid language tag {tag:?}").into());
            }
            if !tags.insert(tag.to_ascii_lowercase()) {
                return Err(format!("locales/{id}.toml: duplicate language tag {tag:?}").into());
            }
        }
        let variant = variant_name(id);
        if variant == "Self" || !variants.insert(variant.clone()) {
            return Err(format!("locales/{id}.toml: unusable language variant {variant}").into());
        }
        if catalog.name.trim().is_empty() {
            return Err(format!("locales/{id}.toml: empty native language name").into());
        }
        for (key, value) in &catalog.messages {
            if !english.messages.contains_key(key) {
                return Err(format!("locales/{id}.toml: unknown message key {key:?}").into());
            }
            if value.trim().is_empty() {
                return Err(format!("locales/{id}.toml: empty translation for {key}").into());
            }
        }
        if id == "en" {
            output.push_str("#[default]\n");
        }
        write!(output, "{variant} => ({id:?}, {:?}, [", catalog.name)?;
        for alias in &catalog.aliases {
            write!(output, "{alias:?},")?;
        }
        output.push_str("], [\n");
        for (key, fallback) in &english.messages {
            let translation = catalog.messages.get(key).unwrap_or(fallback);
            writeln!(output, "{translation:?},")?;
        }
        output.push_str("]),\n");
    }
    output.push_str("}\n}\n");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    fs::write(out_dir.join("catalogs.rs"), output)?;
    Ok(())
}

fn valid_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let primary = parts.next().unwrap_or_default();
    (2..=8).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())
        })
}

fn variant_name(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let mut word = part.to_ascii_lowercase();
            word[..1].make_ascii_uppercase();
            word
        })
        .collect()
}

use eidoryn_gba::{HEADER_LEN, MAX_ROM_LEN};
use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

fn cartridge_files(source: &Path) -> io::Result<Vec<PathBuf>> {
    if !fs::metadata(source)?.is_dir() {
        return Ok(vec![source.to_owned()]);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(source)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gba"))
            && path.is_file()
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn load(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_ROM_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if !(HEADER_LEN..=MAX_ROM_LEN).contains(&bytes.len()) {
        return Err(format!(
            "expected a GBA cartridge of {HEADER_LEN} through {MAX_ROM_LEN} bytes"
        ));
    }
    Ok(bytes)
}

/// Runs all matching local images, retaining the path and header identity for each failure.
pub fn check(
    source: Option<OsString>,
    accepts: impl Fn(&[u8]) -> bool,
    run: impl Fn(Vec<u8>) -> Result<(), String>,
) {
    let source = source.map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests"),
        PathBuf::from,
    );
    let files = cartridge_files(&source).unwrap_or_else(|e| panic!("{}: {e}", source.display()));
    let mut checked = 0;
    let mut failures = Vec::new();
    for path in files {
        let rom = match load(&path) {
            Ok(rom) => rom,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if !accepts(&rom) {
            continue;
        }
        checked += 1;
        let code = String::from_utf8_lossy(&rom[0xac..0xb0]);
        let label = format!(
            "{} [{} rev {}]",
            path.display(),
            code.escape_default(),
            rom[0xbc]
        );
        match run(rom) {
            Ok(()) => eprintln!("PASS {label}"),
            Err(e) => failures.push(format!("{label}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        checked > 0,
        "no matching GBA cartridges in {}",
        source.display()
    );
}

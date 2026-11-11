use std::{path::PathBuf, process::Command};

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eidoryn"));
    command.arg("--no-memory-cards");
    command
}

struct TempDirectory(PathBuf);
impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "eidoryn-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn help_does_not_require_display_server() {
    let output = cli()
        .arg("--help")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--headless"));
}

#[test]
fn invalid_options_fail_without_opening_a_window() {
    for args in [
        vec!["--unknown"],
        vec!["--headless", "--frames", "0"],
        vec!["--headless", "--frames", "-1"],
        vec!["--frames", "1"],
        vec!["--exe"],
        vec!["--exe", "a", "--bios", "b"],
        vec!["--fast-boot"],
        vec!["--disc"],
        vec!["--disc", "a.iso", "--exe", "b.exe"],
        vec!["--export-demo", "a.exe", "--fast-boot"],
        vec!["--headless", "--fullscreen"],
        vec!["--headless", "--game-view"],
        vec!["--headless", "--run"],
        vec!["--headless", "--mute"],
        vec!["--crt"],
        vec!["--renderer"],
        vec!["--cpu"],
        vec!["--cpu", "unknown"],
        vec!["--cpu", "interpreter", "--cpu", "jit"],
        vec!["--export-demo", "a.exe", "--cpu", "jit"],
        vec!["--inspect-disc", "a.cue", "--cpu", "interpreter"],
        vec!["--renderer", "unknown"],
        vec!["--renderer", "serial", "--renderer", "threaded"],
        vec!["--export-demo", "a.exe", "--renderer", "threaded"],
        vec!["--inspect-disc", "a.cue", "--renderer", "serial"],
        vec!["--crt", "unknown"],
        vec!["--crt", "soft", "--crt", "classic"],
        vec!["--headless", "--crt", "off"],
        vec!["--headless", "--crt", "classic"],
        vec!["--export-demo", "a.exe", "--crt", "soft"],
        vec!["--inspect-disc", "a.cue", "--crt", "classic"],
        vec!["--dump-audio"],
        vec!["--dump-audio", "a.wav"],
        vec!["--export-demo", "a.exe", "--mute"],
        vec!["--volume"],
        vec!["--volume", "101"],
        vec!["--volume", "-1"],
        vec!["--volume", "abc"],
        vec!["--volume", "10", "--volume", "20"],
        vec!["--headless", "--volume", "50"],
        vec!["--headless", "--unmute"],
        vec!["--headless", "--clean-fullscreen"],
        vec!["--export-demo", "a.exe", "--fullscreen-controls"],
        vec!["--language"],
        vec!["--language", "fr"],
        vec!["--language", "en", "--language", "zh-CN"],
        vec!["--headless", "--language", "en"],
        vec!["--export-demo", "a.exe", "--language", "en"],
        vec!["--theme"],
        vec!["--theme-plugin"],
        vec!["--card1"],
        vec!["--card2", "card.mcd"], // Conflicts with the helper's disconnected slots.
        vec!["--headless", "--theme", "midnight"],
        vec!["--headless", "--theme-plugin", "missing.json"],
        vec!["--inspect-disc", "a.cue", "--theme", "forest"],
        vec!["--export-demo", "a.exe", "--theme-plugin", "missing.json"],
        vec!["--inspect-disc"],
        vec!["--inspect-disc", "a.cue", "--fullscreen"],
    ] {
        let output = cli().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn headless_audio_export_needs_no_device_and_preserves_existing_files() {
    let directory = TempDirectory::new();
    let exe = directory.0.join("guest.exe");
    std::fs::write(&exe, eidoryn_psx::demo::executable()).unwrap();
    let wav = directory.0.join("guest.wav");
    let run = || {
        cli()
            .args(["--headless", "--frames", "2", "--exe"])
            .arg(&exe)
            .arg("--dump-audio")
            .arg(&wav)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap()
    };
    assert!(run().status.success());
    let before = std::fs::read(&wav).unwrap();
    assert_eq!(&before[..4], b"RIFF");
    assert_eq!(before.len(), 44 + (40_000 / 768) * 4);
    assert!(!run().status.success());
    assert_eq!(std::fs::read(wav).unwrap(), before);
}

#[test]
fn memory_card_defaults_custom_paths_and_reopening_preserve_existing_data() {
    let directory = TempDirectory::new();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_eidoryn"))
            .current_dir(&directory.0)
            .args(["--headless", "--frames", "1"])
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["--no-memory-cards"]).status.success());
    assert!(!directory.0.join("saves").exists());
    assert!(run(&[]).status.success());
    for slot in [1, 2] {
        let path = directory.0.join(format!("saves/card{slot}.mcd"));
        assert_eq!(
            std::fs::read(path).unwrap(),
            eidoryn_psx::memory_card::MemoryCard::formatted().bytes()
        );
    }
    let path = directory.0.join("custom.mcd");
    let mut bytes = eidoryn_psx::memory_card::MemoryCard::formatted()
        .bytes()
        .to_vec();
    bytes[8192..8320].fill(0x7b);
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        run(&["--card1", "custom.mcd", "--card2", "second.mcd"])
            .status
            .success()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::write(&path, b"invalid existing save").unwrap();
    let output = run(&["--card1", "custom.mcd"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("131072"));
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid existing save");
}

#[test]
fn disc_selected_as_bios_or_exe_reports_the_correct_loading_route() {
    let directory = TempDirectory::new();
    let iso = directory.0.join("game.ISO");
    let bin = directory.0.join("disc.bin");
    for path in [&iso, &bin] {
        std::fs::File::create_new(path)
            .unwrap()
            .set_len(600 * 1024)
            .unwrap();
    }
    for flag in ["--bios", "--exe"] {
        let output = cli()
            .arg(flag)
            .arg(&iso)
            .arg("--headless")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("game disc image"), "{error}");
        assert!(error.contains("--disc"), "{error}");
        assert!(error.contains("separate 512 KiB BIOS"), "{error}");
        assert!(!error.contains("Image exceeds"), "{error}");
    }
    let output = cli()
        .arg("--bios")
        .arg(&bin)
        .arg("--headless")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("BIOS firmware must be exactly 512 KiB"),
        "{error}"
    );
    assert!(error.contains("614400 bytes"), "{error}");
    assert!(error.contains("--disc"), "{error}");
}

#[test]
fn headless_exports_real_frame_and_never_overwrites_existing_output() {
    let directory = TempDirectory::new();
    let mut reference = None;
    for mode in ["serial", "threaded"] {
        let path = directory.0.join(format!("{mode}.ppm"));
        let run = || {
            cli()
                .args([
                    "--renderer",
                    mode,
                    "--headless",
                    "--frames",
                    "1",
                    "--dump-frame",
                ])
                .arg(&path)
                .output()
                .unwrap()
        };
        assert!(run().status.success());
        let bytes = std::fs::read(&path).unwrap();
        let header = b"P6\n320 240\n255\n";
        assert!(bytes.starts_with(header));
        assert_eq!(bytes.len(), header.len() + 320 * 240 * 3);
        assert!(!run().status.success());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        if let Some(reference) = &reference {
            assert_eq!(&bytes, reference);
        } else {
            reference = Some(bytes);
        }
    }
}

#[test]
fn exported_guest_program_loads_through_exe_path() {
    let directory = TempDirectory::new();
    let path = directory.0.join("demo.exe");
    assert!(
        cli()
            .arg("--export-demo")
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = cli()
        .args(["--headless", "--frames", "1", "--exe"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("instructions=20000"));
}

#[test]
fn headless_failure_keeps_diagnostics_and_exports_last_frame() {
    let directory = TempDirectory::new();
    let path = directory.0.join("fault.exe");
    let frame = directory.0.join("fault.ppm");
    let mut exe = eidoryn_psx::demo::executable();
    exe[0x800..0x804].copy_from_slice(&0xffff_ffff_u32.to_le_bytes());
    std::fs::write(&path, exe).unwrap();
    let output = cli()
        .arg("--exe")
        .arg(path)
        .args(["--headless", "--frames", "1", "--dump-frame"])
        .arg(&frame)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fault PC=80010000"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("instructions=1")); // Includes the attempted instruction.
    assert!(std::fs::read(frame).unwrap().starts_with(b"P6\n"));
}

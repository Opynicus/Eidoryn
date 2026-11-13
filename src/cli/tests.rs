use super::args::{Options, parse};
use std::{ffi::OsString, path::PathBuf};

fn options(args: &[&str]) -> Result<Options, String> {
    parse(args.iter().map(OsString::from)).map(|result| result.expect("not a help request"))
}

#[test]
fn help_returns_at_its_position_without_consuming_later_arguments() {
    let args = ["--fast-boot", "--help"]
        .into_iter()
        .map(OsString::from)
        .chain(std::iter::from_fn(|| {
            panic!("arguments after help were consumed")
        }));
    assert!(parse(args).unwrap().is_none());
    assert!(
        parse(["-h", "--unknown"].map(OsString::from))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        options(&["--volume", "bad", "--help"]).unwrap_err(),
        "--volume expects 0..100"
    );
    assert_eq!(
        options(&["--unknown", "--help"]).unwrap_err(),
        "Unknown argument: --unknown. Use --help."
    );
    assert_eq!(
        options(&["--exe", "a", "--exe", "--help"]).unwrap_err(),
        "Duplicate --exe"
    );
    // A value resembling a flag is still consumed as that option's value.
    assert_eq!(
        options(&["--exe", "--help"]).unwrap().source,
        Some(PathBuf::from("--help"))
    );
}

#[test]
fn defaults_and_repeated_overrides_keep_their_existing_meaning() {
    let defaults = options(&[]).unwrap();
    assert_eq!(
        defaults.card_paths,
        [
            Some("saves/card1.mcd".into()),
            Some("saves/card2.mcd".into())
        ]
    );
    assert_eq!(defaults.execution.frames, None);
    assert!(defaults.renderer.is_none() && defaults.cpu.is_none() && defaults.crt.is_none());
    assert!(defaults.volume.is_none() && defaults.muted.is_none() && defaults.language.is_none());
    assert!(defaults.clean_fullscreen.is_none() && !defaults.headless && !defaults.no_cards);
    let edited = options(&[
        "--frames",
        "1",
        "--frames",
        "2",
        "--theme",
        "ocean",
        "--theme",
        "forest",
        "--card1",
        "old.mcd",
        "--card1",
        "new.mcd",
        "--mute",
        "--unmute",
        "--clean-fullscreen",
        "--fullscreen-controls",
    ])
    .unwrap();
    assert_eq!(edited.execution.frames, Some(2));
    assert_eq!(edited.theme.as_deref(), Some("forest"));
    assert_eq!(edited.card_paths[0], Some("new.mcd".into()));
    assert!(edited.custom_cards);
    assert_eq!(edited.muted, Some(false));
    assert_eq!(edited.clean_fullscreen, Some(false));
    for (args, expected) in [
        (vec![], true),
        (vec!["--exe", "guest.exe"], false),
        (vec!["--bios", "bios.bin"], false),
        (vec!["--disc", "game.iso"], false),
        (vec!["--disc", "game.iso", "--fast-boot"], true),
        (vec!["--bios", "bios.bin", "--disc", "game.iso"], false),
        (vec!["--exe", "guest.exe", "--run"], true),
    ] {
        assert_eq!(options(&args).unwrap().auto_run(), expected, "{args:?}");
    }
}

#[test]
fn compatibility_errors_keep_their_priority_without_opening_files() {
    for (args, expected) in [
        (
            vec![
                "--no-memory-cards",
                "--card1",
                "a",
                "--inspect-disc",
                "missing.iso",
                "--export-demo",
                "b",
            ],
            "--no-memory-cards cannot be combined with --card1/--card2",
        ),
        (
            vec![
                "--export-demo",
                "a",
                "--inspect-disc",
                "missing.iso",
                "--fast-boot",
            ],
            "--inspect-disc is a standalone operation",
        ),
        (
            vec!["--export-demo", "a", "--headless", "--fullscreen"],
            "EXE export is a standalone operation",
        ),
        (
            vec!["--frames", "1", "--debugger", "--fullscreen", "--fast-boot"],
            "--frames, --dump-frame and --dump-audio require --headless",
        ),
        (
            vec!["--headless", "--fullscreen", "--debugger", "--crt", "soft"],
            "--fullscreen and --game-view require a desktop window",
        ),
        (
            vec!["--headless", "--debugger", "--crt", "soft"],
            "--debugger requires a desktop workspace, without --headless, --fullscreen or --game-view",
        ),
        (
            vec!["--headless", "--crt", "soft", "--mute"],
            "--crt requires a desktop window; frame dumps contain the original pixels",
        ),
        (
            vec!["--headless", "--mute", "--theme", "ocean"],
            "Audio and fullscreen preferences require a desktop window",
        ),
        (
            vec!["--headless", "--theme", "ocean", "--language", "en"],
            "--theme and --theme-plugin require a desktop window",
        ),
        (
            vec!["--headless", "--language", "en", "--run"],
            "--language requires a desktop window",
        ),
        (
            vec!["--headless", "--run", "--fast-boot"],
            "--run requires a desktop window; headless already runs automatically",
        ),
        (
            vec!["--headless", "--fast-boot", "--exe", "a", "--bios", "b"],
            "--fast-boot requires --disc",
        ),
        (
            vec!["--exe", "a", "--bios", "b", "--disc", "missing.iso"],
            "Choose only one of --exe and --bios",
        ),
        (
            vec!["--exe", "a", "--disc", "missing.iso"],
            "Choose either --disc or --exe",
        ),
    ] {
        assert_eq!(
            options(&args).unwrap().validate().unwrap_err(),
            expected,
            "{args:?}"
        );
    }
    // Media validity remains an execution concern, after argument compatibility.
    options(&["--headless", "--disc", "does-not-exist.iso"])
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn standalone_operations_share_all_startup_exclusions() {
    for operation in ["--inspect-disc", "--export-demo", "--export-disc-test"] {
        options(&[operation, "missing", "--no-memory-cards"])
            .unwrap()
            .validate()
            .unwrap();
        let expected = if operation == "--inspect-disc" {
            "--inspect-disc is a standalone operation"
        } else {
            "EXE export is a standalone operation"
        };
        for startup in [
            vec!["--exe", "missing"],
            vec!["--bios", "missing"],
            vec!["--disc", "missing"],
            vec!["--crt", "off"],
            vec!["--renderer", "serial"],
            vec!["--cpu", "interpreter"],
            vec!["--unmute"],
            vec!["--volume", "75"],
            vec!["--fullscreen-controls"],
            vec!["--dump-audio", "missing"],
            vec!["--card1", "saves/card1.mcd"],
            vec!["--card2", "saves/card2.mcd"],
            vec!["--fast-boot"],
            vec!["--headless"],
            vec!["--fullscreen"],
            vec!["--game-view"],
            vec!["--debugger"],
            vec!["--language", "en"],
            vec!["--theme", "midnight"],
            vec!["--theme-plugin", "missing"],
            vec!["--run"],
            vec!["--frames", "120"],
            vec!["--dump-frame", "missing"],
        ] {
            let args: Vec<_> = [operation, "missing"].into_iter().chain(startup).collect();
            assert_eq!(
                options(&args).unwrap().validate().unwrap_err(),
                expected,
                "{args:?}"
            );
        }
    }
}

#[cfg(not(feature = "desktop"))]
#[test]
fn disabled_desktop_is_checked_after_argument_compatibility() {
    assert_eq!(
        options(&[]).unwrap().validate().unwrap_err(),
        "Desktop support is disabled. Use --headless or build with default features."
    );
    options(&["--headless"]).unwrap().validate().unwrap();
}

#[cfg(unix)]
#[test]
fn paths_keep_non_utf8_bytes_while_text_values_reject_them() {
    use std::os::unix::ffi::OsStringExt;
    let path = OsString::from_vec(vec![b'a', 0xff]);
    let parsed = parse([
        OsString::from("--exe"),
        path.clone(),
        OsString::from("--headless"),
    ])
    .unwrap()
    .unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.source, Some(PathBuf::from(path.clone())));
    assert_eq!(
        parse([OsString::from("--cpu"), path]).unwrap_err(),
        "CPU mode must be UTF-8"
    );
}

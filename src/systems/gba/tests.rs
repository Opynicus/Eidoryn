use super::{Command, Runner, Snapshot, firmware, import_firmware, needs_firmware_import};
use crate::library::{GamePlatform, Library};
use std::{
    thread,
    time::{Duration, Instant},
};

#[path = "../../../tests/support/gba_firmware.rs"]
mod firmware_fixture;
#[path = "../../../tests/support/gba.rs"]
mod fixture;

fn firmware_path(dir: &std::path::Path) -> std::path::PathBuf {
    let folder = dir.join("firmware");
    std::fs::create_dir_all(&folder).unwrap();
    folder.join("gba_bios.bin")
}

fn wait_for(runner: &Runner, matches: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(snapshot) = runner.take_snapshot() {
            assert!(
                snapshot.error.is_none(),
                "worker error: {:?}",
                snapshot.error
            );
            if matches(&snapshot) {
                return snapshot;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for GBA snapshot"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn expect_pixels(runner: &Runner, first: [u8; 3], boot_red: u8) -> Snapshot {
    wait_for(runner, |frame| {
        frame.running
            && frame.rgb.len() == eidoryn_gba::WIDTH * eidoryn_gba::HEIGHT * 3
            && frame.rgb[..3] == first
            && frame.rgb[3..6] == [boot_red, 0, 0]
    })
}

#[test]
fn cartridge_worker_displays_input_pauses_and_restores_battery_save() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("runner-probe.gba");
    let bios = firmware_path(temp.path());
    let library = Library::open(&temp.path().join("games.sqlite")).unwrap();
    fixture::write_cartridge(&rom);
    std::fs::write(&bios, firmware_fixture::firmware()).unwrap();

    let blue = [0, 0, 255];
    let red = [255, 0, 0];
    let load = || Command::Load {
        path: rom.clone(),
        title: "Runner Probe".into(),
    };
    {
        let runner = Runner::start(Some(library.clone()), temp.path().to_owned(), None).unwrap();
        runner.send(load()).unwrap();
        let first = expect_pixels(&runner, blue, 0);
        assert_eq!(first.path.as_deref(), Some(rom.as_path()));
        assert_eq!(first.title, "Runner Probe");

        runner.send(Command::Buttons(1)).unwrap();
        expect_pixels(&runner, red, 0);
        runner.send(Command::Pause).unwrap();
        let paused = wait_for(&runner, |frame| !frame.running);
        assert_eq!(paused.rgb[..3], red);
        runner.send(Command::Buttons(1)).unwrap();
        runner.send(Command::Toggle).unwrap();
        expect_pixels(&runner, blue, 0);

        runner.send(Command::Reset).unwrap();
        expect_pixels(&runner, blue, 8);
    }

    // Opening a fresh worker also exercises lock release and file-backed SRAM restore.
    let runner = Runner::start(Some(library.clone()), temp.path().to_owned(), None).unwrap();
    runner.send(load()).unwrap();
    expect_pixels(&runner, blue, 16);
    drop(runner);
    let games = library.games().unwrap();
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].platform_kind(), Some(GamePlatform::Gba));
    assert_eq!(games[0].locations[0].path, rom.canonicalize().unwrap());
}

#[test]
fn closing_retries_a_failed_battery_save_with_the_same_cartridge() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.gba");
    let bios = firmware_path(dir.path());
    std::fs::write(&bios, firmware_fixture::firmware()).unwrap();
    fixture::write_cartridge(&path);
    let mut worker = super::Worker::new(dir.path(), None, None);
    worker
        .apply(Command::Load {
            path,
            title: "Probe".into(),
        })
        .unwrap();
    worker
        .cartridge
        .as_mut()
        .unwrap()
        .machine
        .run_frame()
        .unwrap();
    let save = worker.cartridge.as_ref().unwrap().save.clone();
    // A directory at the file destination makes the atomic replacement fail on all hosts.
    std::fs::create_dir(&save).unwrap();
    let close = |worker: &mut super::Worker| {
        let (tx, rx) = std::sync::mpsc::channel();
        worker.apply(Command::PrepareClose(tx)).unwrap();
        rx.recv().unwrap()
    };
    assert!(close(&mut worker).is_err());
    assert!(!worker.snapshot.running);
    std::fs::remove_dir(&save).unwrap();
    close(&mut worker).unwrap();
    assert_eq!(
        std::fs::read(save).unwrap(),
        worker.cartridge.unwrap().machine.sram()
    );
}

#[test]
fn launch_revalidates_firmware_and_keeps_the_running_cartridge_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.gba");
    fixture::write_cartridge(&path);
    let bios = firmware_path(dir.path());
    let saves = dir.path().join("saves/gba");
    let mut worker = super::Worker::new(dir.path(), None, None);
    let load = || Command::Load {
        path: path.clone(),
        title: "Probe".into(),
    };

    for bad_file in [
        None,
        Some(vec![0xbb; 512 * 1024]),
        Some(vec![0xff; eidoryn_gba::BIOS_LEN]),
    ] {
        if let Some(bytes) = bad_file {
            std::fs::write(&bios, bytes).unwrap();
        }
        worker.apply(load()).unwrap();
        let error = worker.snapshot.error.as_ref().unwrap();
        assert!(error.contains(bios.to_str().unwrap()), "{error}");
        assert!(worker.cartridge.is_none());
        assert!(!saves.exists());
    }

    std::fs::write(&bios, firmware_fixture::firmware()).unwrap();
    worker.apply(load()).unwrap();
    worker.frame().unwrap();
    assert_eq!(worker.cartridge.as_ref().unwrap().machine.sram()[0x80], 0);
    let revision = worker.snapshot.media_revision;
    std::fs::write(&bios, [0u8; 16]).unwrap();
    worker.apply(load()).unwrap();
    assert!(worker.snapshot.error.as_deref().unwrap().contains("16 KiB"));
    assert!(worker.snapshot.running);
    assert_eq!(worker.snapshot.media_revision, revision);

    worker.apply(Command::Reset).unwrap();
    worker.frame().unwrap();
    assert_eq!(worker.cartridge.as_ref().unwrap().machine.sram()[0x80], 1);
}

#[test]
fn canonical_firmware_takes_precedence_over_an_old_saved_path() {
    let dir = tempfile::tempdir().unwrap();
    let prior = dir.path().join("original.bin");
    let canonical = firmware_path(dir.path());
    let old = firmware_fixture::firmware();
    std::fs::write(&prior, &old).unwrap();
    let legacy = Some(prior.as_path());
    assert!(!needs_firmware_import(dir.path(), legacy).unwrap());
    assert_eq!(firmware::load(&canonical, legacy).unwrap(), old);
    let mut replacement = old;
    replacement[0x200] = 0x42;
    std::fs::write(&canonical, &replacement).unwrap();
    assert_eq!(firmware::load(&canonical, legacy).unwrap(), replacement);
    std::fs::write(&canonical, [0u8; 16]).unwrap();
    assert!(needs_firmware_import(dir.path(), legacy).unwrap());
    assert_eq!(
        firmware::load(&canonical, legacy).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn first_import_installs_one_validated_copy_independent_of_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("fresh-profile");
    let installed = firmware::installed_path(&data);
    let source = dir.path().join("console.bin");
    assert!(needs_firmware_import(&data, Some(&source)).unwrap());
    std::fs::write(&source, vec![0xff; eidoryn_gba::BIOS_LEN]).unwrap();
    assert_eq!(
        import_firmware(&data, &source).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(!data.exists());

    let original = firmware_fixture::firmware();
    std::fs::write(&source, &original).unwrap();
    import_firmware(&data, &source).unwrap();
    let mut different = original.clone();
    different[0x200] ^= 0xff;
    std::fs::write(&source, &different).unwrap();
    import_firmware(&data, &source).unwrap();
    std::fs::remove_file(source).unwrap();
    assert_eq!(std::fs::read(installed).unwrap(), original);
    assert!(!needs_firmware_import(&data, None).unwrap());
}

#[test]
fn concurrent_imports_repair_a_corrupt_firmware_with_one_complete_image() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let installed = firmware_path(data);
    std::fs::write(&installed, [0; 12]).unwrap();
    let first = firmware_fixture::firmware();
    let mut second = first.clone();
    second[0x200] ^= 0xff;
    let paths = [data.join("first.bin"), data.join("second.bin")];
    std::fs::write(&paths[0], &first).unwrap();
    std::fs::write(&paths[1], &second).unwrap();
    let start = std::sync::Barrier::new(paths.len());
    thread::scope(|scope| {
        for path in &paths {
            scope.spawn(|| {
                start.wait();
                import_firmware(data, path).unwrap();
            });
        }
    });
    let bytes = std::fs::read(installed).unwrap();
    assert!(bytes == first || bytes == second);
    assert!(!needs_firmware_import(data, None).unwrap());
}

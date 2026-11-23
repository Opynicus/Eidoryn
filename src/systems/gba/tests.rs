use super::{Command, Runner, Snapshot};
use crate::library::{GamePlatform, Library};
use std::{
    thread,
    time::{Duration, Instant},
};

#[path = "../../../tests/support/gba.rs"]
mod fixture;

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
    let saves = temp.path().join("saves");
    let library = Library::open(&temp.path().join("games.sqlite")).unwrap();
    fixture::write_cartridge(&rom);

    let blue = [0, 0, 255];
    let red = [255, 0, 0];
    let load = || Command::Load(rom.clone(), "Runner Probe".into());
    {
        let runner = Runner::start(Some(library.clone()), saves.clone()).unwrap();
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
    let runner = Runner::start(Some(library.clone()), saves).unwrap();
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
    fixture::write_cartridge(&path);
    let mut worker = super::Worker::new(dir.path().join("saves"), None);
    worker.apply(Command::Load(path, "Probe".into())).unwrap();
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

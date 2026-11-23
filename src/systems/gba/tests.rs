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
    let library = Library::open(&temp.path().join("games.sqlite")).unwrap();
    fixture::write_cartridge(&rom);
    let blue = [0, 0, 255];
    let red = [255, 0, 0];
    let load = || Command::Load {
        path: rom.clone(),
        title: "Runner Probe".into(),
    };
    {
        let runner = Runner::start(Some(library.clone()), temp.path().to_owned()).unwrap();
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
    let runner = Runner::start(Some(library.clone()), temp.path().to_owned()).unwrap();
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
    let mut worker = super::Worker::new(dir.path(), None);
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
fn stale_firmware_from_an_older_installation_does_not_affect_hle() {
    let data = tempfile::tempdir().unwrap();
    let firmware_dir = data.path().join("firmware");
    std::fs::create_dir(&firmware_dir).unwrap();
    let old_file = firmware_dir.join("gba_bios.bin");
    std::fs::write(&old_file, b"old invalid firmware").unwrap();
    let rom = data.path().join("homebrew.gba");
    fixture::write_cartridge(&rom);

    let runner = Runner::start(None, data.path().to_owned()).unwrap();
    runner
        .send(Command::Load {
            path: rom,
            title: "Homebrew".into(),
        })
        .unwrap();
    expect_pixels(&runner, [0, 0, 255], 0);
    assert_eq!(std::fs::read(old_file).unwrap(), b"old invalid firmware");
}

#[test]
fn graphics_requests_keep_the_loaded_cartridge_running_with_or_without_a_gpu() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("gpu-probe.gba");
    fixture::write_cartridge(&rom);
    let runner = Runner::start(None, temp.path().to_owned()).unwrap();
    runner
        .send(Command::Load {
            path: rom,
            title: "GPU probe".into(),
        })
        .unwrap();
    let initial = expect_pixels(&runner, [0, 0, 255], 0);
    runner
        .send(Command::SetHardware {
            enabled: true,
            request: 1,
        })
        .unwrap();
    let switched = wait_for(&runner, |s| s.graphics_request == 1);
    assert!(switched.running);
    assert_eq!(
        switched.graphics_device.is_some(),
        switched.graphics_error.is_none()
    );
    assert_eq!(initial.media_revision, switched.media_revision);
    runner.send(Command::Buttons(1)).unwrap();
    expect_pixels(&runner, [255, 0, 0], 0);
    runner
        .send(Command::SetHardware {
            enabled: false,
            request: 2,
        })
        .unwrap();
    let software = wait_for(&runner, |s| s.graphics_request == 2);
    assert!(software.graphics_device.is_none() && software.graphics_error.is_none());
    assert!(software.running);
}

#[test]
fn frame_rate_uses_completed_frames_and_elapsed_wall_time() {
    let start = Instant::now();
    let mut meter = super::FrameRate::new(start);
    for frame in 1..60 {
        assert!(
            meter
                .completed_frame(start + Duration::from_millis(16 * frame))
                .is_none()
        );
    }
    assert_eq!(
        meter.completed_frame(start + Duration::from_secs(1)),
        Some(60.0)
    );
    // Taking two seconds to finish one frame reports half a frame per second.
    assert_eq!(
        meter.completed_frame(start + Duration::from_secs(3)),
        Some(0.5)
    );
}

#[test]
fn static_picture_counts_while_playing_and_debug_commands_reset_rate() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("fps-probe.gba");
    fixture::write_cartridge(&rom);
    let runner = Runner::start(None, temp.path().to_owned()).unwrap();
    runner
        .send(Command::Load {
            path: rom,
            title: "Frame rate".into(),
        })
        .unwrap();
    let first = expect_pixels(&runner, [0, 0, 255], 0);
    let sampled = wait_for(&runner, |s| s.video_fps > 0.0);
    assert!(std::sync::Arc::ptr_eq(&first.rgb, &sampled.rgb));
    runner.send(Command::Pause).unwrap();
    let paused = wait_for(&runner, |s| !s.running);
    assert!(paused.video_fps.abs() < f64::EPSILON);
    for command in [Command::Step, Command::Slice] {
        runner.send(command).unwrap();
        let stepped = wait_for(&runner, |s| !s.running);
        assert!(stepped.video_fps.abs() < f64::EPSILON);
    }
    runner.send(Command::Toggle).unwrap();
    let restarted = wait_for(&runner, |s| s.running);
    assert!(restarted.video_fps.abs() < f64::EPSILON);
}

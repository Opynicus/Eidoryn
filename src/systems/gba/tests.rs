use super::{Command, Runner, Snapshot};
use crate::library::{GamePlatform, Library};
use crate::systems::handheld_tone as tone;
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

#[test]
fn quick_slot_and_history_round_trip_across_sessions_and_cartridge_moves() {
    use crate::systems::checkpoints::QuickSlot;
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("probe.gba");
    fixture::write_cartridge(&rom);
    let load = |path| Command::Load {
        path,
        title: "Probe".into(),
    };
    let mut worker = super::Worker::new(temp.path(), None, None);
    worker.apply(load(rom.clone())).unwrap();
    worker.frame().unwrap();
    let initial = worker.snapshot.rgb.clone();
    let revision = worker.snapshot.quick_slot_revision;
    let quick = || Command::QuickSaveIfUnchanged {
        revision,
        slot: QuickSlot::Empty,
    };
    worker.apply(quick()).unwrap();
    assert!(matches!(worker.snapshot.quick_slot, QuickSlot::Saved(_)));
    worker.apply(Command::Buttons(1)).unwrap();
    worker.frame().unwrap();
    worker.frame().unwrap();
    assert_ne!(worker.snapshot.rgb, initial);
    worker.apply(quick()).unwrap();
    assert!(worker.snapshot.error.is_some());
    assert!(worker.snapshot.running);
    worker.apply(Command::SaveState).unwrap();
    let saved = worker.snapshot.rgb.clone();
    let entry = worker.snapshot.save_states[0].clone();
    worker
        .apply(Command::RenameState {
            path: entry.path,
            name: "before boss".into(),
        })
        .unwrap();
    let entry = worker.snapshot.save_states[0].clone();
    assert_eq!(entry.name, "before boss");
    worker.apply(Command::QuickLoad).unwrap();
    assert_eq!(worker.snapshot.rgb, initial);
    assert!(worker.snapshot.running);
    worker.flush().unwrap();
    drop(worker);

    let moved = temp.path().join("relocated.gba");
    fs::rename(rom, &moved).unwrap();
    let other = temp.path().join("other.gba");
    let mut modified = fs::read(&moved).unwrap();
    modified[0xad] ^= 1;
    fs::write(&other, modified).unwrap();
    let mut worker = super::Worker::new(temp.path(), None, None);
    worker.apply(load(other)).unwrap();
    assert!(worker.snapshot.save_states.is_empty());
    assert_eq!(worker.snapshot.quick_slot, QuickSlot::Empty);
    worker
        .apply(Command::LoadState {
            path: entry.path.clone(),
        })
        .unwrap();
    assert!(worker.snapshot.error.is_some());
    assert!(worker.snapshot.running);
    worker.apply(load(moved)).unwrap();
    assert_eq!(
        worker.snapshot.save_states.as_ref(),
        std::slice::from_ref(&entry)
    );
    worker.apply(Command::Pause).unwrap();
    worker
        .apply(Command::LoadState {
            path: entry.path.clone(),
        })
        .unwrap();
    assert!(!worker.snapshot.running);
    assert_eq!(worker.snapshot.rgb, saved);
    worker
        .apply(Command::DeleteState {
            path: entry.path.clone(),
            modified: std::time::UNIX_EPOCH,
        })
        .unwrap();
    assert!(worker.snapshot.error.is_some());
    assert!(entry.path.exists());
    worker
        .apply(Command::DeleteState {
            path: entry.path,
            modified: entry.modified,
        })
        .unwrap();
    assert!(worker.snapshot.save_states.is_empty());
    assert!(matches!(worker.snapshot.quick_slot, QuickSlot::Saved(_)));
}

#[test]
fn held_rewind_restores_live_input_and_paused_rewind_stays_paused() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("rewind.gba");
    fixture::write_cartridge(&rom);
    let mut worker = super::Worker::new(temp.path(), None, None);
    worker
        .apply(Command::Load {
            path: rom,
            title: "Probe".into(),
        })
        .unwrap();
    worker.frame().unwrap();
    let blue = worker.snapshot.rgb.clone();
    worker.apply(Command::SetRewindEnabled(true)).unwrap();
    worker.apply(Command::Buttons(1)).unwrap();
    for _ in 0..16 {
        worker.frame().unwrap();
    }
    assert_eq!(worker.snapshot.rewind_states, 2);
    let red = worker.snapshot.rgb.clone();
    assert_ne!(red, blue);
    worker.apply(Command::SetRewinding(true)).unwrap();
    assert!(worker.snapshot.rewinding);
    assert!(!worker.snapshot.running);
    assert_eq!(worker.snapshot.rgb, red);
    worker.rewind_one().unwrap();
    assert_eq!(worker.snapshot.rgb, blue);
    assert_eq!(worker.snapshot.rewind_states, 0);
    assert!(!worker.rewind_one().unwrap());
    worker.apply(Command::Buttons(1)).unwrap();
    worker.apply(Command::SetRewinding(false)).unwrap();
    assert!(worker.snapshot.running);
    worker.frame().unwrap();
    worker.frame().unwrap();
    assert_eq!(worker.snapshot.rgb, red);
    worker.apply(Command::Pause).unwrap();
    worker.apply(Command::SetRewinding(true)).unwrap();
    assert!(worker.snapshot.rewinding);
    worker.apply(Command::SetRewinding(false)).unwrap();
    assert!(!worker.snapshot.running);
    worker.apply(Command::SetRewindEnabled(false)).unwrap();
    assert_eq!(worker.snapshot.rewind_states, 0);
    assert!(worker.snapshot.rewind_seconds.abs() < f64::EPSILON);
}

#[test]
fn sav_import_and_export_require_the_original_session_and_restart_after_import() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("probe.gba");
    fixture::write_cartridge(&rom);
    let mut worker = super::Worker::new(temp.path(), None, None);
    worker
        .apply(Command::Load {
            path: rom,
            title: "Probe".into(),
        })
        .unwrap();
    worker.frame().unwrap();
    let revision = worker.snapshot.media_revision;
    let export = temp.path().join("player.sav");
    worker
        .apply(Command::ExportBattery {
            path: export.clone(),
            media_revision: revision - 1,
        })
        .unwrap();
    assert!(worker.snapshot.error.is_some());
    assert!(!export.exists());
    worker
        .apply(Command::ExportBattery {
            path: export.clone(),
            media_revision: revision,
        })
        .unwrap();
    let mut image = std::fs::read(&export).unwrap();
    image[0] = 10;
    std::fs::write(&export, &image).unwrap();
    worker.apply(Command::SetRewindEnabled(true)).unwrap();
    worker.frame().unwrap();
    worker
        .apply(Command::ImportBattery {
            path: export,
            media_revision: revision,
        })
        .unwrap();
    assert!(worker.snapshot.error.is_none());
    assert_eq!(worker.snapshot.rewind_states, 0);
    assert_eq!(
        std::fs::read(worker.snapshot.battery_path.as_ref().unwrap()).unwrap(),
        image
    );
    worker.frame().unwrap();
    worker.frame().unwrap();
    worker.apply(Command::SyncBattery).unwrap();
    let persisted = std::fs::read(worker.snapshot.battery_path.as_ref().unwrap()).unwrap();
    assert_eq!(persisted[0], 11);
}

#[test]
fn rewind_defers_battery_writes_and_only_persists_the_resumed_timeline() {
    let temp = tempfile::tempdir().unwrap();
    let rom = temp.path().join("rewind.gba");
    fixture::write_cartridge(&rom);
    let mut worker = super::Worker::new(temp.path(), None, None);
    worker
        .apply(Command::Load {
            path: rom,
            title: "Probe".into(),
        })
        .unwrap();
    worker.frame().unwrap();
    worker.apply(Command::SyncBattery).unwrap();
    let path = worker.snapshot.battery_path.clone().unwrap();
    let oldest = std::fs::read(&path).unwrap();
    worker.apply(Command::SetRewindEnabled(true)).unwrap();
    let mut latest = oldest.clone();
    latest[10] ^= 0xff;
    worker
        .cartridge
        .as_mut()
        .unwrap()
        .machine
        .restore_sram(&latest)
        .unwrap();
    worker.apply(Command::SyncBattery).unwrap();
    worker.frame().unwrap();
    worker.apply(Command::SetRewinding(true)).unwrap();
    assert!(worker.snapshot.rewinding);
    assert_eq!(worker.cartridge.as_ref().unwrap().machine.sram(), oldest);
    assert_eq!(std::fs::read(&path).unwrap(), latest);
    worker.apply(Command::SetRewinding(false)).unwrap();
    worker.apply(Command::Pause).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), oldest);
}

#[test]
fn game_audio_reaches_the_device_and_drops_old_pcm_on_timeline_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tone.gba");
    fixture::write_cartridge(&path);
    let mut rom = std::fs::read(&path).unwrap();
    rom.truncate(eidoryn_gba::HEADER_LEN);
    rom.extend(tone::register_program(&[
        (0x0400_0084, 0x0080), // Master power
        (0x0400_0080, 0x1177), // Square 1 to both speakers
        (0x0400_0082, 2),
        (0x0400_0062, 0xf080),
        (0x0400_0064, 0x8700), // Trigger square wave
    ]));
    std::fs::write(&path, rom).unwrap();
    let audio = eidoryn_host::audio::AudioBuffer::with_sample_rate(
        std::num::NonZeroU32::new(eidoryn_gba::AUDIO_SAMPLE_RATE).unwrap(),
    );
    let mut worker = super::Worker::new(dir.path(), None, Some(audio.clone()));
    worker
        .apply(Command::Load {
            path,
            title: "Tone".into(),
        })
        .unwrap();
    worker.apply(Command::SetRewindEnabled(true)).unwrap();
    for _ in 0..4 {
        worker.frame().unwrap();
    }
    tone::assert_audible(&audio);
    worker.apply(Command::QuickSave).unwrap();
    worker.apply(Command::QuickLoad).unwrap();
    assert_eq!(audio.stats().queued_frames, 0);
    for _ in 0..4 {
        worker.frame().unwrap();
    }
    worker.begin_rewind().unwrap();
    assert_eq!(audio.stats().queued_frames, 0);
    worker.end_rewind();
    for _ in 0..4 {
        worker.frame().unwrap();
    }
    tone::assert_audible(&audio);
    worker.apply(Command::Pause).unwrap();
    worker.apply(Command::Slice).unwrap();
    assert_eq!(audio.stats().queued_frames, 0);
}

use super::*;
use crate::systems::handheld_tone as tone;

#[path = "../../../tests/support/nds.rs"]
mod fixture;

fn await_snapshot(runner: &Runner, ready: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if let Some(snapshot) = runner.take_snapshot() {
            assert_eq!(snapshot.error, None);
            if ready(&snapshot) {
                return snapshot;
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("NDS worker did not publish the requested state");
}

#[test]
fn desktop_worker_runs_both_processors_from_a_cartridge_without_bios() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.nds");
    fixture::write_cartridge(&file);
    let runner = Runner::start(None, dir.path().to_owned()).unwrap();
    runner
        .send(Command::Load {
            path: file.clone(),
            title: "Dual LCD".into(),
        })
        .unwrap();
    let first = await_snapshot(&runner, |s| s.rgb.starts_with(&[255, 0, 0]));
    let bottom = eidoryn_nds::WIDTH * eidoryn_nds::SCREEN_HEIGHT * 3;
    assert!(first.running);
    assert_eq!(first.path, Some(file));
    assert_eq!(&first.rgb[bottom..bottom + 3], &[0, 0, 255]);
    assert!((0x0200_0000..0x0200_0100).contains(&first.arm9_pc));
    assert!((0x0380_0000..0x0380_0100).contains(&first.arm7_pc));
    runner.send(Command::Buttons(1)).unwrap();
    await_snapshot(&runner, |s| s.rgb.starts_with(&[0, 255, 0]));
    // The DS program reads both touch ADC channels and paints the lower LCD.
    // Moving horizontally changes red alone; vertically changes green alone.
    let pen_color = |x, y, ready: &dyn Fn(&[u8]) -> bool| {
        runner.send(Command::Touch(Some((x, y)))).unwrap();
        let state = await_snapshot(&runner, |s| ready(&s.rgb[bottom..bottom + 3]));
        <[u8; 3]>::try_from(&state.rgb[bottom..bottom + 3]).unwrap()
    };
    let first = pen_color(32, 40, &|p| p[2] == 0);
    let horizontal = pen_color(200, 40, &|p| p[0] > first[0] && p[1] == first[1]);
    pen_color(200, 160, &|p| p[0] == horizontal[0] && p[1] > horizontal[1]);
    runner.send(Command::Touch(None)).unwrap();
    await_snapshot(&runner, |s| s.rgb[bottom..bottom + 3] == [0, 0, 255]);
    runner.send(Command::Pause).unwrap();
    await_snapshot(&runner, |s| !s.running);
    runner.send(Command::Frame).unwrap();
    await_snapshot(&runner, |s| !s.running && s.rgb.starts_with(&[255, 0, 0]));
}

#[test]
fn quick_slot_is_cartridge_scoped_and_restores_the_frame_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.nds");
    fixture::write_cartridge(&path);
    let open = |w: &mut Worker, path| {
        w.apply(Command::Load {
            path,
            title: "Dual LCD".into(),
        })
        .unwrap()
    };
    let mut worker = Worker::new(dir.path(), None, None);
    open(&mut worker, path.clone());
    worker.apply(Command::Buttons(1)).unwrap();
    worker.frame().unwrap();
    let revision = worker.state.quick_slot_revision;
    worker
        .apply(Command::QuickSaveIfUnchanged {
            revision,
            slot: QuickSlot::Empty,
        })
        .unwrap();
    assert!(
        worker
            .apply(Command::QuickSaveIfUnchanged {
                revision,
                slot: QuickSlot::Empty
            })
            .is_err()
    );
    drop(worker);

    let moved = dir.path().join("renamed.nds");
    std::fs::rename(path, &moved).unwrap();
    let mut worker = Worker::new(dir.path(), None, None);
    open(&mut worker, moved);
    assert!(matches!(worker.state.quick_slot, QuickSlot::Saved(_)));
    worker.apply(Command::QuickLoad).unwrap();
    assert_eq!(&worker.cart().unwrap().machine.pixels()[..3], &[0, 255, 0]);
    worker.frame().unwrap(); // Restoring releases keys that are no longer held.
    assert_eq!(&worker.cart().unwrap().machine.pixels()[..3], &[255, 0, 0]);
    let other = dir.path().join("other.nds");
    let mut rom = fixture::cartridge();
    rom[0] = b'X';
    std::fs::write(&other, rom).unwrap();
    open(&mut worker, other);
    assert_eq!(worker.state.quick_slot, QuickSlot::Empty);
}

#[test]
fn battery_survives_relocation_and_failed_writes_are_retried_before_switching() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.nds");
    fixture::write_cartridge(&path);
    let load = |worker: &mut Worker, path: &Path| {
        worker.apply(Command::Load {
            path: path.to_owned(),
            title: "Cartridge".into(),
        })
    };
    let mut worker = Worker::new(dir.path(), None, None);
    load(&mut worker, &path).unwrap();
    let save = worker.cart().unwrap().battery.clone();
    let bytes = vec![0x77; 8192];
    worker.cart().unwrap().machine.load_backup(&bytes).unwrap();
    let saves = dir.path().join("saves");
    fs::write(&saves, b"temporarily blocked").unwrap();
    assert!(load(&mut worker, &path).is_err());
    assert_eq!(
        worker.cart().unwrap().machine.backup(),
        Some(bytes.as_slice())
    );
    fs::remove_file(&saves).unwrap();
    worker.apply(Command::Pause).unwrap();
    assert_eq!(fs::read(&save).unwrap(), bytes);
    let moved = dir.path().join("renamed.nds");
    fs::rename(&path, &moved).unwrap();
    drop(worker);
    let mut worker = Worker::new(dir.path(), None, None);
    load(&mut worker, &moved).unwrap();
    assert_eq!(
        worker.cart().unwrap().machine.backup(),
        Some(bytes.as_slice())
    );
    worker.apply(Command::Reset).unwrap();
    assert_eq!(
        worker.cart().unwrap().machine.backup(),
        Some(bytes.as_slice())
    );
    fs::write(&save, b"invalid backup").unwrap();
    assert!(load(&mut worker, &moved).is_err());
    assert_eq!(fs::read(save).unwrap(), b"invalid backup");
}

#[test]
#[ignore = "requires the US Order of Ecclesia cartridge"]
fn ecclesia_desktop_worker_persists_the_games_own_eeprom_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let rom = std::env::var_os("EIDORYN_ECCLESIA_ROM").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/Castlevania - Order of Ecclesia (USA) (En,Fr).nds")
        },
        PathBuf::from,
    );
    let start = || {
        let mut worker = Worker::new(dir.path(), None, None);
        worker
            .load(rom.clone(), "Order of Ecclesia".to_owned())
            .unwrap();
        worker
    };
    let mut worker = start();
    for frame in 1..=220 {
        worker
            .apply(Command::Buttons(u16::from((90..95).contains(&frame))))
            .unwrap();
        worker.frame().unwrap();
    }
    let (tx, rx) = mpsc::channel();
    worker.apply(Command::PrepareClose(tx)).unwrap();
    rx.recv().unwrap().unwrap();
    let cart = worker.cart().unwrap();
    let data = fs::read(&cart.battery).unwrap();
    assert_eq!(data.len(), 8192);
    assert!(data.iter().any(|&b| b != 0xff));
    assert_eq!(cart.machine.backup(), Some(data.as_slice()));
    drop(worker);
    let mut reopened = start();
    assert_eq!(
        reopened.cart().unwrap().machine.backup(),
        Some(data.as_slice())
    );
    // With its saved language preference the game goes directly to the
    // "Licensed by Nintendo" screen. An erased EEPROM shows the picker here.
    for _ in 0..150 {
        reopened.frame().unwrap();
    }
    let latest = Mutex::default();
    reopened.publish(&latest);
    let snapshot = latest.into_inner().unwrap().unwrap();
    let (upper, lower) = snapshot
        .rgb
        .split_at(eidoryn_nds::WIDTH * eidoryn_nds::SCREEN_HEIGHT * 3);
    let lettering = upper
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] > 100 && p[0] == p[1] && p[1] == p[2])
        .count();
    assert!((450..850).contains(&lettering));
    assert!(lower.iter().all(|&c| c == 0));
}

#[test]
fn arm7_audio_reaches_the_device_and_drops_old_pcm_on_timeline_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tone.nds");
    let mut rom = fixture::cartridge();
    let routine = tone::register_program(&[
        (0x0400_0304, 1),      // Enable the sound circuit
        (0x0400_0500, 0x807f), // Master enable and volume
        (0x0400_0504, 0x0200), // DAC bias
        (0x0400_0488, 0xf000), // Channel 8 PSG timer
        (0x0400_0480, 0x007f), // Channel volume
        (0x0400_0482, 0xe340), // Start square wave, center pan
    ]);
    rom[0x3c..0x40].copy_from_slice(&u32::try_from(routine.len()).unwrap().to_le_bytes());
    rom[0x9000..0x9000 + routine.len()].copy_from_slice(&routine);
    std::fs::write(&path, rom).unwrap();
    let audio = AudioBuffer::with_sample_rate(
        std::num::NonZeroU32::new(eidoryn_nds::AUDIO_SAMPLE_RATE).unwrap(),
    );
    let mut worker = Worker::new(dir.path(), None, Some(audio.clone()));
    worker
        .apply(Command::Load {
            path,
            title: "Tone".into(),
        })
        .unwrap();
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
    tone::assert_audible(&audio);
    worker.apply(Command::Pause).unwrap();
    worker.apply(Command::Frame).unwrap();
    assert_eq!(audio.stats().queued_frames, 0);
}

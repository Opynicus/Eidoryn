use eidoryn_psx::{
    bus::Bus,
    cdrom::{COMMAND_CLOCKS, SECTOR_CLOCKS},
    demo,
    disc::{DiscImage, DiscRegion},
    machine::Machine,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ps1-own-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn image(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dual(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    bytes[offset + 4..offset + 8].copy_from_slice(&value.to_be_bytes());
}
fn record(name: &[u8], lba: u32, length: u32, directory: bool) -> Vec<u8> {
    let mut bytes = vec![0; (33 + name.len() + 1) & !1];
    bytes[0] = bytes.len() as u8;
    dual(&mut bytes, 2, lba);
    dual(&mut bytes, 10, length);
    bytes[25] = if directory { 2 } else { 0 };
    bytes[28] = 1;
    bytes[31] = 1;
    bytes[32] = name.len() as u8;
    bytes[33..33 + name.len()].copy_from_slice(name);
    bytes
}
fn iso(exe: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 300 * 2048];
    let pvd = &mut bytes[16 * 2048..17 * 2048];
    pvd[..7].copy_from_slice(b"\x01CD001\x01");
    dual(pvd, 80, 300);
    pvd[128..132].copy_from_slice(&[0, 8, 8, 0]);
    pvd[156..190].copy_from_slice(&record(&[0], 20, 2048, true));
    bytes[17 * 2048..17 * 2048 + 7].copy_from_slice(b"\xffCD001\x01");
    let config = b"BOOT = cdrom:\\TEST.EXE;1\r\n";
    let directory = [
        record(&[0], 20, 2048, true),
        record(&[1], 20, 2048, true),
        record(b"SYSTEM.CNF;1", 21, config.len() as u32, false),
        record(b"TEST.EXE;1", 22, exe.len() as u32, false),
    ]
    .concat();
    bytes[20 * 2048..20 * 2048 + directory.len()].copy_from_slice(&directory);
    bytes[21 * 2048..21 * 2048 + config.len()].copy_from_slice(config);
    bytes[22 * 2048..22 * 2048 + exe.len()].copy_from_slice(exe);
    bytes
}
fn bus(path: &Path) -> Bus {
    let mut bus = Bus::default();
    bus.cdrom.insert(DiscImage::open(path).unwrap());
    bus.write8(0x1f80_1800, 1).unwrap();
    bus.write8(0x1f80_1802, 31).unwrap();
    bus
}
fn issue(bus: &mut Bus, opcode: u8, parameters: &[u8]) {
    bus.write8(0x1f80_1800, 0).unwrap();
    for parameter in parameters {
        bus.write8(0x1f80_1802, *parameter).unwrap();
    }
    bus.write8(0x1f80_1801, opcode).unwrap();
}
fn response(bus: &mut Bus, interrupt: u8) -> Vec<u8> {
    bus.write8(0x1f80_1800, 1).unwrap();
    assert_eq!(bus.read8(0x1f80_1803).unwrap() & 7, interrupt);
    let mut bytes = Vec::new();
    while bus.read8(0x1f80_1800).unwrap() & 0x20 != 0 {
        bytes.push(bus.read8(0x1f80_1801).unwrap());
    }
    bytes
}
fn ack(bus: &mut Bus) {
    bus.write8(0x1f80_1800, 1).unwrap();
    bus.write8(0x1f80_1803, 0x1f).unwrap();
}
fn command(bus: &mut Bus, opcode: u8, parameters: &[u8]) -> Vec<u8> {
    issue(bus, opcode, parameters);
    bus.tick(COMMAND_CLOCKS).unwrap();
    let bytes = response(bus, 3);
    ack(bus);
    bytes
}

fn raw_mode2(cooked: &[u8]) -> Vec<u8> {
    cooked
        .as_chunks::<2048>()
        .0
        .iter()
        .flat_map(|data| {
            let mut sector = [0; 2352];
            sector[1..11].fill(255);
            sector[15] = 2;
            sector[18] = 8;
            sector[22] = 8;
            sector[24..2072].copy_from_slice(data);
            sector
        })
        .collect()
}

#[test]
fn cdda_play_returns_pcm_and_defers_autopause_irq_until_ack() {
    let fixture = Fixture::new();
    fixture.image("data.bin", &raw_mode2(&vec![0; 5 * 2048]));
    let frame = [1000_i16, -2000].map(i16::to_le_bytes).concat();
    fixture.image("audio.bin", &frame.repeat(588 * 2));
    fixture.image("next.bin", &[0; 2352]);
    let path = fixture.image("audio.cue", b"FILE \"data.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 01 00:00:00\nFILE \"next.bin\" BINARY\n TRACK 03 AUDIO\n INDEX 01 00:00:00\n");
    let mut bus = bus(&path);
    command(&mut bus, 0x0e, &[2]); // Auto pause at the next track.
    issue(&mut bus, 3, &[2]);
    bus.cdrom.tick(COMMAND_CLOCKS);
    assert_eq!(response(&mut bus, 3), [0x82]);
    // Keep INT3 pending while the audio transport continues at sector cadence.
    for _ in 0..2 {
        bus.cdrom.tick(SECTOR_CLOCKS);
        for _ in 0..588 {
            assert_eq!(bus.cdrom.audio_sample(), [1000, -2000]);
        }
    }
    bus.cdrom.tick(SECTOR_CLOCKS);
    bus.write8(0x1f80_1800, 1).unwrap();
    assert_eq!(bus.read8(0x1f80_1803).unwrap() & 7, 3);
    ack(&mut bus);
    bus.cdrom.tick(1);
    assert_eq!(response(&mut bus, 4), [2]);
    ack(&mut bus);
    assert_eq!(bus.cdrom.cdda_sectors, 2);
    assert_eq!(bus.cdrom.audio_sample(), [0; 2]);

    command(&mut bus, 3, &[2]);
    bus.cdrom.tick(SECTOR_CLOCKS);
    command(&mut bus, 9, &[]); // Pause discards pending audio.
    assert_eq!(bus.cdrom.audio_sample(), [0; 2]);
    bus.cdrom.tick(COMMAND_CLOCKS * 2);
    assert_eq!(response(&mut bus, 2), [2]);
}

#[test]
fn read_s_routes_xa_audio_without_data_irq_and_delivers_interleaved_form2_video() {
    let fixture = Fixture::new();
    let mut raw = raw_mode2(&vec![0x66; 4 * 2048]);
    for (sector, channel, submode) in [(0, 2, 0x64), (1, 3, 0x64), (2, 2, 0x62)] {
        let header = [1, channel, submode, 0];
        raw[sector * 2352 + 16..sector * 2352 + 20].copy_from_slice(&header);
        raw[sector * 2352 + 20..sector * 2352 + 24].copy_from_slice(&header);
    }
    let path = fixture.image("xa-video.bin", &raw);
    assert!(DiscImage::open(&path).unwrap().read_data(2).is_err());
    let mut bus = bus(&path);
    command(&mut bus, 0x0b, &[]);
    assert!(bus.cdrom.muted());
    command(&mut bus, 0x0c, &[]);
    assert!(!bus.cdrom.muted());
    command(&mut bus, 0x0d, &[1, 2]);
    command(&mut bus, 0x0e, &[0xc8]);
    command(&mut bus, 2, &[0, 2, 0]);
    command(&mut bus, 0x1b, &[]);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    assert!(!bus.cdrom.irq_pending());
    assert_eq!(bus.cdrom.xa_sectors, 1);
    assert_ne!(bus.cdrom.read(0) & 4, 0);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    assert!(!bus.cdrom.irq_pending());
    assert_eq!(bus.cdrom.filtered_sectors, 1);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    assert_eq!(response(&mut bus, 1), [0x22]);
    ack(&mut bus);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2048);
    for _ in 0..2048 {
        assert_eq!(bus.cdrom.read_data(), 0x66);
    }
    assert_eq!(bus.cdrom.sectors_read, 3);
    assert_eq!(bus.cdrom.command_errors, 0);
    for _ in 0..4704 {
        bus.cdrom.audio_sample();
    }
    assert_eq!(bus.cdrom.read(0) & 4, 0); // The playing flag must clear when XA drains.
    command(&mut bus, 2, &[0, 2, 0]);
    command(&mut bus, 0x1b, &[]);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    assert_ne!(bus.cdrom.read(0) & 4, 0);
    bus.cdrom.write(0, 1);
    bus.cdrom.write(3, 0x20); // Decoder buffer clear, independently of drive commands.
    assert_eq!(bus.cdrom.read(0) & 4, 0);
    assert_eq!(bus.cdrom.audio_sample(), [0; 2]);
}

#[test]
fn getlocl_tracks_xa_headers_through_menu_pause_resume_and_next_data_read() {
    let fixture = Fixture::new();
    let mut raw = raw_mode2(&vec![0x55; 7 * 2048]);
    for (index, sector) in raw.as_chunks_mut::<2352>().0.iter_mut().enumerate() {
        sector[12..15].copy_from_slice(&[0, 2, index as u8]);
        if (1..=3).contains(&index) {
            let header = [7, if index == 2 { 2 } else { 1 }, 0x64, 0];
            sector[16..20].copy_from_slice(&header);
            sector[20..24].copy_from_slice(&header);
            for group in sector[24..24 + 18 * 128].as_chunks_mut::<128>().0 {
                group[..16].fill(0);
                group[16..].fill(0x21); // Original, audible XA test signal.
            }
        }
    }
    let path = fixture.image("menu-stream.bin", &raw);
    let mut bus = bus(&path);
    command(&mut bus, 0x0d, &[7, 1]);
    command(&mut bus, 0x0e, &[0xc8]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    response(&mut bus, 1);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    for _ in 0..2048 {
        bus.cdrom.read_data();
    }
    ack(&mut bus);
    assert_eq!(&command(&mut bus, 0x10, &[])[..3], &[0, 2, 0]);

    // XA sectors advance the physical header even without a host data IRQ,
    // including channels rejected by the audio filter.
    for (sector, channel) in [(1, 1), (2, 2)] {
        bus.tick(SECTOR_CLOCKS / 2).unwrap();
        assert!(!bus.cdrom.irq_pending());
        assert_eq!(bus.cdrom.data_available(), 0);
        assert_eq!(
            command(&mut bus, 0x10, &[]),
            [0, 2, sector, 2, 7, channel, 0x64, 0]
        );
    }
    assert_eq!(bus.cdrom.xa_sectors, 1);
    assert_eq!(bus.cdrom.filtered_sectors, 1);
    let resume = command(&mut bus, 0x10, &[]);
    command(&mut bus, 9, &[]);
    bus.tick(COMMAND_CLOCKS * 2).unwrap();
    assert_eq!(response(&mut bus, 2), [2]);
    ack(&mut bus);
    assert_eq!(bus.cdrom.read(0) & 4, 0);
    command(&mut bus, 2, &resume[..3]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS / 2).unwrap(); // Re-read filtered channel 2.
    bus.tick(SECTOR_CLOCKS / 2).unwrap(); // Resume audible channel 1.
    assert_eq!(bus.cdrom.filtered_sectors, 2);
    assert_eq!(bus.cdrom.xa_sectors, 2);
    assert!((0..200).any(|_| bus.cdrom.audio_sample() != [0; 2]));

    // A subsequent resource load must still produce INT1 and complete DMA3.
    command(&mut bus, 9, &[]);
    bus.tick(COMMAND_CLOCKS * 2).unwrap();
    response(&mut bus, 2);
    ack(&mut bus);
    command(&mut bus, 0x0e, &[0x80]);
    command(&mut bus, 2, &[0, 2, 4]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS / 2).unwrap();
    assert_eq!(response(&mut bus, 1), [0x22]);
    bus.write32(0x1f80_10f0, 0x8000).unwrap();
    bus.write32(0x1f80_10b0, 0x20000).unwrap();
    bus.write32(0x1f80_10b4, 512).unwrap();
    bus.write32(0x1f80_10b8, 0x1100_0000).unwrap();
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert!(bus.ram[0x20000..0x20800].iter().all(|&v| v == 0x55));
    assert_eq!(bus.read32(0x1f80_10b8).unwrap() & (1 << 24), 0);
    assert_eq!(bus.cdrom.command_errors, 0);
}

#[test]
fn get_id_uses_sector_four_region_and_waits_for_initial_response_ack() {
    let fixture = Fixture::new();
    for (text, region, letter) in [
        (
            "Sony Computer Entertainment Amer  ica ",
            DiscRegion::America,
            b'A',
        ),
        (
            "Sony Computer Entertainment Euro pe   ",
            DiscRegion::Europe,
            b'E',
        ),
        (
            "Sony Computer Entertainment Inc.\n",
            DiscRegion::Japan,
            b'I',
        ),
    ] {
        let mut bytes = iso(&demo::executable());
        let license = format!("          Licensed  by          {text}");
        bytes[4 * 2048..4 * 2048 + license.len()].copy_from_slice(license.as_bytes());
        for (name, image) in [
            ("licensed.iso", bytes.clone()),
            ("licensed.bin", raw_mode2(&bytes)),
        ] {
            let path = fixture.image(name, &image);
            assert_eq!(
                DiscImage::open(&path).unwrap().license_region().unwrap(),
                Some(region)
            );
            let mut bus = bus(&path);
            issue(&mut bus, 0x1a, &[]);
            bus.tick(COMMAND_CLOCKS).unwrap();
            assert_eq!(response(&mut bus, 3), [2]);
            bus.tick(COMMAND_CLOCKS * 2).unwrap();
            assert_eq!(bus.read8(0x1f80_1803).unwrap() & 7, 3);
            ack(&mut bus);
            bus.tick(1).unwrap();
            assert_eq!(
                response(&mut bus, 2),
                [2, 0, 0x20, 0, b'S', b'C', b'E', letter]
            );
        }
    }
}

#[test]
fn get_id_keeps_unknown_images_unlicensed_and_reports_missing_disc() {
    let fixture = Fixture::new();
    let path = fixture.image("homebrew.iso", &iso(&demo::executable()));
    assert_eq!(
        DiscImage::open(&path).unwrap().license_region().unwrap(),
        None
    );
    let mut drive = bus(&path);
    assert_eq!(command(&mut drive, 0x1a, &[]), [2]);
    drive.tick(COMMAND_CLOCKS * 2).unwrap();
    assert_eq!(response(&mut drive, 5), [0x0a, 0x80, 0x20, 0, 0, 0, 0, 0]);
    let mut empty = Bus::default();
    assert_eq!(command(&mut empty, 0x1a, &[]), [0x10]);
    empty.tick(COMMAND_CLOCKS * 2).unwrap();
    assert_eq!(response(&mut empty, 5), [8, 0x40, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn cue_separate_bins_keep_stored_pregap_and_guest_toc() {
    let fixture = Fixture::new();
    let exe = demo::executable();
    fixture.image("Track 1.bin", &raw_mode2(&iso(&exe)));
    let audio = [vec![0xaa; 150 * 2352], vec![0xbb; 225 * 2352]].concat();
    fixture.image("Track 2.bin", &audio);
    let path = fixture.image("disc.cue", b"FILE \"Track 1.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\nFILE \"Track 2.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 00 00:00:00\n INDEX 01 00:02:00\n");
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.sectors, 675);
    assert_eq!(disc.boot_executable().unwrap(), exe);
    let track = disc.tracks().nth(1).unwrap();
    assert_eq!(
        (track.index0_lba, track.start_lba, track.end_lba),
        (300, 450, 675)
    );
    assert_eq!(disc.read_sector(449).unwrap(), [0xaa; 2352]);
    assert_eq!(disc.read_sector(450).unwrap(), [0xbb; 2352]);
    assert_eq!(disc.read_sector(674).unwrap(), [0xbb; 2352]);
    assert!(disc.read_sector(675).is_err());
    assert!(disc.read_data(450).is_err());
    let mut bus = bus(&path);
    assert_eq!(command(&mut bus, 0x13, &[]), [2, 1, 2]);
    assert_eq!(command(&mut bus, 0x14, &[1]), [2, 0, 2]);
    assert_eq!(command(&mut bus, 0x14, &[2]), [2, 0, 8]);
    assert_eq!(command(&mut bus, 0x14, &[0]), [2, 0, 0x11]);
    issue(&mut bus, 0x14, &[3]);
    bus.tick(COMMAND_CLOCKS).unwrap();
    assert_eq!(response(&mut bus, 5)[1], 0x10);
    ack(&mut bus);
    command(&mut bus, 2, &[0, 7, 0]); // absolute 00:07:00 = LBA 375, audio pregap.
    command(&mut bus, 0x16, &[]);
    bus.tick(COMMAND_CLOCKS * 2).unwrap();
    response(&mut bus, 2);
    ack(&mut bus);
    assert_eq!(command(&mut bus, 0x11, &[]), [2, 0, 0, 1, 0, 0, 7, 0]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    assert_eq!(response(&mut bus, 5)[1], 0x80);
    assert_eq!(bus.cdrom.sectors_read, 0);
    assert!(bus.cdrom.last_error.as_ref().unwrap().contains("audio"));
    let output = Command::new(env!("CARGO_BIN_EXE_eidoryn"))
        .arg("--no-memory-cards")
        .arg("--inspect-disc")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.contains("tracks=2 sectors=675"));
    assert_eq!(report.matches("boundary reads OK").count(), 2);
    assert!(report.contains("Valid PS-X EXE"));
}

#[test]
fn cue_shared_bin_offsets_exclude_first_leadin_and_bound_track_extents() {
    let fixture = Fixture::new();
    let exe = demo::executable();
    let bytes = [
        vec![0xdd; 2 * 2352],
        raw_mode2(&iso(&exe)),
        vec![0xaa; 2 * 2352],
        vec![0xbb; 2 * 2352],
    ]
    .concat();
    fixture.image("disc.bin", &bytes);
    let path = fixture.image("disc.cue", b"FILE disc.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 00 00:00:00\nINDEX 01 00:00:02\nTRACK 02 AUDIO\nINDEX 00 00:04:02\nINDEX 01 00:04:04\n");
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.boot_executable().unwrap(), exe);
    assert_eq!(disc.sectors, 304);
    assert_eq!(disc.track_at(299).unwrap().number, 1);
    assert_eq!(disc.track_at(300).unwrap().number, 2);
    assert_eq!(disc.read_sector(301).unwrap(), [0xaa; 2352]);
    assert_eq!(disc.read_sector(302).unwrap(), [0xbb; 2352]);
    assert!(disc.read_sector(304).is_err());
}

#[test]
fn cue_synthetic_audio_gap_adds_silence_without_consuming_file() {
    let fixture = Fixture::new();
    fixture.image("data.bin", &raw_mode2(&vec![0; 2048]));
    fixture.image("audio.bin", &[0x55; 2352]);
    let path = fixture.image("disc.cue", b"FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\nFILE audio.bin BINARY\nTRACK 02 AUDIO\nPREGAP 00:02:00\nINDEX 01 00:00:00\n");
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.sectors, 152);
    assert_eq!(disc.read_sector(1).unwrap(), [0; 2352]);
    assert_eq!(disc.read_sector(150).unwrap(), [0; 2352]);
    assert_eq!(disc.read_sector(151).unwrap(), [0x55; 2352]);
}

#[test]
fn cue_rejects_invalid_paths_indices_and_unbounded_extents() {
    let fixture = Fixture::new();
    fixture.image("data.bin", &raw_mode2(&vec![0; 2048]));
    for cue in [
        "FILE ../data.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00",
        "FILE /data.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00",
        "FILE data.bin WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352",
        "FILE data.bin BINARY\nTRACK 02 MODE2/2352\nINDEX 01 00:00:00",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:01",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:75",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 00 00:00:01\nINDEX 01 00:00:00",
        "FILE data.bin BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:00:00",
        "FILE data.bin BINARY\nTRACK 01 AUDIO\nPREGAP 00:02:00\nINDEX 00 00:00:00\nINDEX 01 00:00:00",
    ] {
        let path = fixture.image("bad.cue", cue.as_bytes());
        assert!(DiscImage::open(&path).is_err(), "{cue}");
    }
    let path = fixture.image("too-big.cue", &vec![b' '; 65_537]);
    assert!(DiscImage::open(&path).is_err());
}

#[test]
fn delayed_commands_wait_for_interrupt_ack_and_irq_latches() {
    let fixture = Fixture::new();
    let path = fixture.image("data.iso", &vec![0; 2048]);
    let mut bus = bus(&path);
    bus.write32(0x1f80_1074, 4).unwrap();
    issue(&mut bus, 1, &[]);
    assert_ne!(bus.read8(0x1f80_1800).unwrap() & 0x80, 0);
    bus.tick(COMMAND_CLOCKS - 1).unwrap();
    assert!(!bus.irq_pending());
    bus.tick(1).unwrap();
    assert_eq!(response(&mut bus, 3), [2]);
    assert!(bus.irq_pending());
    issue(&mut bus, 0x0e, &[0x80]);
    bus.tick(COMMAND_CLOCKS).unwrap();
    assert_ne!(bus.read8(0x1f80_1800).unwrap() & 0x80, 0); // next command blocked by INT3
    bus.write32(0x1f80_1070, !4).unwrap();
    assert!(bus.irq_pending());
    ack(&mut bus);
    bus.write32(0x1f80_1070, !4).unwrap();
    assert!(!bus.irq_pending());
    bus.tick(1).unwrap();
    assert_eq!(response(&mut bus, 3), [2]);
    ack(&mut bus);
    assert_eq!(command(&mut bus, 0x0f, &[]), [2, 0x80, 0, 0, 0]);
}

#[test]
fn interrupt_mask_uses_encoded_flags_and_ack_discards_unread_response() {
    let fixture = Fixture::new();
    let path = fixture.image("data.iso", &vec![0; 2048]);
    let mut bus = bus(&path);
    issue(&mut bus, 0x0f, &[]); // Getparam produces five response bytes with INT3.
    bus.tick(COMMAND_CLOCKS).unwrap();
    bus.write8(0x1f80_1800, 1).unwrap();
    bus.write8(0x1f80_1802, 4).unwrap(); // Enable the INTSTS bit, not interrupt number 3.
    assert!(!bus.cdrom.irq_pending());
    bus.write8(0x1f80_1802, 2).unwrap();
    assert!(bus.cdrom.irq_pending());
    assert_eq!(bus.read8(0x1f80_1801).unwrap(), 2);
    assert_ne!(bus.read8(0x1f80_1800).unwrap() & 0x20, 0);
    bus.write8(0x1f80_1803, 1).unwrap(); // Partial flag acknowledgement changes INT3 to INT2.
    assert_eq!(bus.read8(0x1f80_1803).unwrap() & 7, 2);
    assert!(bus.cdrom.irq_pending());
    ack(&mut bus);
    assert!(!bus.cdrom.irq_pending());
    assert_eq!(bus.read8(0x1f80_1800).unwrap() & 0x20, 0);
    assert_eq!(command(&mut bus, 1, &[]), [2]);
}

#[test]
fn readn_awaits_bfrd_then_dma3_moves_sector_and_signals_completion() {
    let fixture = Fixture::new();
    let image = iso(&demo::executable());
    let path = fixture.image("disc.iso", &image);
    let mut bus = bus(&path);
    command(&mut bus, 2, &[0, 2, 0x16]);
    command(&mut bus, 0x0e, &[0x80]);
    command(&mut bus, 6, &[]);
    bus.write32(0x1f80_10f0, 0x8000).unwrap();
    bus.write32(0x1f80_10f4, 0x0088_0000).unwrap();
    bus.write32(0x1f80_10b0, 0x20000).unwrap();
    bus.write32(0x1f80_10b4, 512).unwrap();
    bus.write32(0x1f80_10b8, 0x1100_0000).unwrap();
    bus.tick(SECTOR_CLOCKS / 2 - 1).unwrap();
    assert_eq!(bus.cdrom.sectors_read, 0);
    bus.tick(1).unwrap();
    assert_eq!(response(&mut bus, 1), [0x22]);
    assert_ne!(bus.read32(0x1f80_10b8).unwrap() & (1 << 24), 0);
    assert_eq!(bus.cdrom.data_available(), 0);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(&bus.ram[0x20000..0x20800], &image[16 * 2048..17 * 2048]);
    assert_eq!(bus.read32(0x1f80_10b8).unwrap() & 0x1100_0000, 0);
    assert_eq!(bus.read32(0x1f80_10b0).unwrap(), 0x20000); // burst MADR stays at start
    assert_eq!(bus.read32(0x1f80_10f4).unwrap() & 0x8800_0000, 0x8800_0000);
    assert_eq!(bus.read32(0x1f80_1070).unwrap() & 0x0c, 0x0c);
    ack(&mut bus);
    command(&mut bus, 9, &[]);
    bus.tick(COMMAND_CLOCKS * 2).unwrap();
    assert_eq!(response(&mut bus, 2), [2]);
    ack(&mut bus);
    bus.tick(SECTOR_CLOCKS * 2).unwrap();
    assert_eq!(bus.cdrom.sectors_read, 1);
}

#[test]
fn raw_data_mode_and_halfword_fifo_reads_preserve_sector_header() {
    let fixture = Fixture::new();
    let path = fixture.image("disc.iso", &vec![0x5a; 2048]);
    let mut bus = bus(&path);
    command(&mut bus, 0x0e, &[0x20]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    response(&mut bus, 1);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2340);
    assert_eq!(bus.read16(0x1f80_1802).unwrap(), 0x0200);
    assert_eq!(bus.read16(0x1f80_1802).unwrap(), 0x0200);
    bus.write8(0x1f80_1803, 0).unwrap();
    assert_eq!(bus.cdrom.data_available(), 0);
}

#[test]
fn drive_errors_are_guest_interrupts_and_do_not_panic() {
    let mut empty = Bus::default();
    issue(&mut empty, 6, &[]);
    empty.tick(COMMAND_CLOCKS).unwrap();
    assert_eq!(response(&mut empty, 5)[1], 0x80);
    let fixture = Fixture::new();
    let path = fixture.image("disc.iso", &vec![0; 2048]);
    let mut bus = bus(&path);
    for (opcode, parameters, error) in [
        (2, vec![0, 0x6a, 0], 0x10),
        (2, vec![0], 0x20),
        (0xff, vec![], 0x40),
    ] {
        issue(&mut bus, opcode, &parameters);
        bus.tick(COMMAND_CLOCKS).unwrap();
        assert_eq!(response(&mut bus, 5)[1], error);
        ack(&mut bus);
    }
    command(&mut bus, 2, &[0, 3, 0]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    assert_eq!(response(&mut bus, 5)[1], 0x80);
    assert!(
        bus.cdrom
            .last_error
            .as_ref()
            .unwrap()
            .contains("beyond end")
    );
}

#[test]
fn iso_boot_reader_checks_directory_extents_and_read_bounds() {
    let fixture = Fixture::new();
    let exe = demo::executable();
    let mut image = iso(&exe);
    let path = fixture.image("good.iso", &image);
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.boot_executable().unwrap(), exe);
    assert_eq!(disc.read_file("test.exe", 65536).unwrap(), exe);
    assert!(disc.read_file("../TEST.EXE", 65536).is_err());
    assert!(disc.read_file("TEST.EXE", 4).is_err());
    assert!(disc.read_data(300).is_err());
    dual(&mut image, 16 * 2048 + 156 + 2, 3000);
    assert!(
        DiscImage::open(&fixture.image("bad.iso", &image))
            .unwrap()
            .boot_executable()
            .is_err()
    );
    let short = fixture.image("short.iso", &[0; 2049]);
    assert!(DiscImage::open(&short).is_err());
    std::fs::write(&path, [0; 16]).unwrap();
    assert!(disc.read_data(0).is_err());
}

#[test]
fn raw_bin_mode1_and_mode2_payload_offsets_are_distinct() {
    let fixture = Fixture::new();
    let cooked = fixture.image("cooked.iso", &vec![0x4b; 2048]);
    let raw2 = DiscImage::open(&cooked).unwrap().read_sector(0).unwrap();
    let mut raw1 = [0; 2352];
    raw1[..16].copy_from_slice(&raw2[..16]);
    raw1[15] = 1;
    raw1[16..2064].fill(0x6c);
    for (name, raw, expected) in [("mode1.bin", raw1, 0x6c), ("mode2.bin", raw2, 0x4b)] {
        let disc = DiscImage::open(&fixture.image(name, &raw)).unwrap();
        assert_eq!(disc.read_data(0).unwrap(), [expected; 2048]);
    }
}

#[test]
fn mips_guest_reads_disc_via_cdrom_and_dma_before_drawing() {
    let fixture = Fixture::new();
    let bytes = iso(&demo::disc_probe_executable());
    let path = fixture.image("probe.iso", &bytes);
    let mut machine = Machine::from_disc(&path, None, true).unwrap();
    assert!(machine.bus.cdrom.mounted());
    for _ in 0..20 {
        machine.run_slice().unwrap();
    }
    assert_eq!(
        machine.bus.peek32(0x300),
        Some(0x4344_524d),
        "guest did not validate ISO data; pc={:08x}",
        machine.cpu.pc
    );
    assert_eq!(
        &machine.bus.ram[0x20000..0x20800],
        &bytes[16 * 2048..17 * 2048]
    );
    assert_eq!(machine.bus.cdrom.sectors_read, 1);
    assert!(machine.bus.gpu.display_rgb().iter().any(|v| *v > 100));
}

#[test]
fn cli_disc_fast_boot_uses_self_built_core() {
    let fixture = Fixture::new();
    let path = fixture.image("probe.iso", &iso(&demo::disc_probe_executable()));
    let output = Command::new(env!("CARGO_BIN_EXE_eidoryn"))
        .arg("--no-memory-cards")
        .args(["--headless", "--fast-boot", "--frames", "20", "--disc"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("self-built"), "{text}");
    assert!(text.contains("CD sectors=1"), "{text}");
}

#[test]
fn worker_disc_reset_and_failed_loads_preserve_valid_session_and_bios() {
    use eidoryn_psx::runner::{Command as WorkerCommand, Runner, Snapshot};
    use std::time::{Duration, Instant};
    fn wait(runner: &Runner, condition: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(snapshot) = runner.take_snapshot()
                && condition(&snapshot)
            {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not publish expected state"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let fixture = Fixture::new();
    let bios = fixture.image("test-firmware.bin", &vec![0; 512 * 1024]);
    let disc = fixture.image("guest.iso", &iso(&demo::disc_probe_executable()));
    let runner = Runner::new(Machine::demo(), false).unwrap();
    runner.take_snapshot();
    runner.send(WorkerCommand::SetButtons(0xffff)).unwrap();
    let input = wait(&runner, |s| s.pad_buttons != 0);
    assert_eq!(input.pad_buttons, 0xfff9);
    assert!(!input.running);
    runner.send(WorkerCommand::SetButtons(0)).unwrap();
    wait(&runner, |s| s.pad_buttons == 0);
    runner
        .send(WorkerCommand::Load {
            path: bios.clone(),
            bios: true,
        })
        .unwrap();
    assert_eq!(
        wait(&runner, |s| s.bios_path.is_some()).bios_path,
        Some(bios.clone())
    );
    runner
        .send(WorkerCommand::Load {
            path: disc.clone(),
            bios: true,
        })
        .unwrap();
    let failed = wait(&runner, |s| s.message.is_some());
    assert_eq!(failed.bios_path, Some(bios.clone()));
    assert_eq!(failed.cpu.pc, 0xbfc0_0000);
    runner
        .send(WorkerCommand::LoadDisc {
            path: disc,
            bios: Some(bios.clone()),
            fast_boot: true,
        })
        .unwrap();
    let active = wait(&runner, |s| s.cdrom_reads == 1);
    assert!(active.message.is_none());
    assert!(active.disc_mounted);
    runner.send(WorkerCommand::SetButtons(1 << 14)).unwrap();
    assert!(wait(&runner, |s| s.pad_buttons == 1 << 14).running);
    runner.send(WorkerCommand::Reset).unwrap();
    let reset = wait(&runner, |s| !s.running && s.cpu.instructions == 0);
    assert!(reset.disc_mounted);
    assert_eq!(reset.cdrom_reads, 0);
    assert_eq!(reset.pad_buttons, 0);
    assert_eq!(reset.bios_path, Some(bios.clone()));
    runner
        .send(WorkerCommand::LoadDisc {
            path: fixture.0.join("missing.iso"),
            bios: None,
            fast_boot: true,
        })
        .unwrap();
    let failed = wait(&runner, |s| s.message.is_some());
    assert!(failed.disc_mounted);
    assert_eq!(failed.title, reset.title);
    assert_eq!(failed.bios_path, Some(bios));
}

#[test]
fn cd_staging_reuse_preserves_fifo_backpressure_mode_changes_and_init() {
    let fixture = Fixture::new();
    let image = raw_mode2(&[vec![0x31; 2048], vec![0x62; 2048], vec![0x93; 2048]].concat());
    let path = fixture.image("staging.bin", &image);
    let mut bus = bus(&path);
    command(&mut bus, 0x0e, &[0x20]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    assert_eq!(response(&mut bus, 1), [0x22]);
    ack(&mut bus);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2340);
    for &byte in &image[12..29] {
        assert_eq!(bus.cdrom.read_data(), byte);
    }

    // Prefetch a short payload while the previous raw sector is partly unread.
    command(&mut bus, 0x0e, &[0]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    assert_eq!(response(&mut bus, 1), [0x22]);
    ack(&mut bus);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2340 - 17);
    for &byte in &image[29..2352] {
        assert_eq!(bus.cdrom.read_data(), byte);
    }
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2048);
    for _ in 0..2048 {
        assert_eq!(bus.cdrom.read_data(), 0x62);
    }
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 0); // Staging must not be replayed.

    command(&mut bus, 0x0e, &[0x20]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    response(&mut bus, 1);
    ack(&mut bus);
    command(&mut bus, 0x0a, &[]); // Init must discard the prefetched raw sector.
    bus.tick(COMMAND_CLOCKS * 2).unwrap();
    assert_eq!(response(&mut bus, 2), [2]);
    ack(&mut bus);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 0);

    command(&mut bus, 2, &[0, 2, 0]);
    command(&mut bus, 6, &[]);
    bus.tick(SECTOR_CLOCKS).unwrap();
    response(&mut bus, 1);
    ack(&mut bus);
    bus.write8(0x1f80_1800, 0).unwrap();
    bus.write8(0x1f80_1803, 0x80).unwrap();
    assert_eq!(bus.cdrom.data_available(), 2340);
    for &byte in &image[12..2352] {
        assert_eq!(bus.cdrom.read_data(), byte);
    }
    assert_eq!(bus.cdrom.read_data(), 0);
    assert_eq!(bus.cdrom.command_errors, 0);
}

#[test]
fn batched_bus_clocks_keep_cdda_and_spu_in_cycle_order() {
    let fixture = Fixture::new();
    let frame = [1000_i16, -2000].map(i16::to_le_bytes).concat();
    fixture.image("original-audio.bin", &frame.repeat(588 * 3));
    let path = fixture.image(
        "original-audio.cue",
        b"FILE original-audio.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
    );
    let mut batched = bus(&path);
    assert_eq!(command(&mut batched, 3, &[]), [0x82]);
    let mut single = batched.clone();
    let clocks = SECTOR_CLOCKS * 2;
    batched.tick(clocks).unwrap();
    for _ in 0..clocks {
        single.tick(1).unwrap();
    }
    assert_eq!(batched.elapsed_cycles(), u64::from(COMMAND_CLOCKS + clocks));
    assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
    assert_eq!(batched.cdrom.cdda_sectors, 2);
    assert_eq!(batched.cdrom.cdda_sectors, single.cdrom.cdda_sectors);
    assert_eq!(batched.cdrom.debug_state(), single.cdrom.debug_state());
    assert_eq!(batched.spu.generated_samples, single.spu.generated_samples);
    assert_eq!(
        batched.spu.drain_audio().collect::<Vec<_>>(),
        single.spu.drain_audio().collect::<Vec<_>>()
    );
    for _ in 0..588 {
        assert_eq!(batched.cdrom.audio_sample(), single.cdrom.audio_sample());
    }
    for timer in 0..3 {
        for register in [0, 4, 8] {
            let address = 0x1100 + timer * 16 + register;
            assert_eq!(batched.timers.read(address), single.timers.read(address));
        }
    }
    assert_eq!(batched.irq_pending(), single.irq_pending());
}

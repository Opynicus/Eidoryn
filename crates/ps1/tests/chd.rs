use eidoryn_ps1::disc::{DiscImage, TrackKind};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

const FRAME: usize = 2448;
const HUNK: usize = FRAME * 2;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        Self::at_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
    }

    fn at_timestamp(timestamp: u128) -> Self {
        // Concurrent tests can observe the same system-clock timestamp.
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "eidoryn-chd-{}-{timestamp}-{id}",
            std::process::id(),
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

#[test]
fn fixtures_with_the_same_timestamp_keep_files_and_cleanup_isolated() {
    let first = Fixture::at_timestamp(0);
    let second = Fixture::at_timestamp(0);
    assert_ne!(first.0, second.0);
    let first_image = first.image("same.chd", &[1]);
    let second_image = second.image("same.chd", &[2]);
    assert_eq!(std::fs::read(&first_image).unwrap(), [1]);
    assert_eq!(std::fs::read(&second_image).unwrap(), [2]);
    drop(first);
    assert!(!first_image.exists());
    assert_eq!(std::fs::read(second_image).unwrap(), [2]);
}

// Original test data in the documented uncompressed v5 layout; no chdman or
// external game assets are required. The frame buffer includes track padding;
// metadata FRAMES counts exclude it.
// https://github.com/mamedev/mame/blob/master/src/lib/util/chd.h
fn chd(metadata: &[(&[u8; 4], &str)], frames: &[[u8; FRAME]]) -> Vec<u8> {
    assert!(frames.len().is_multiple_of(4));
    let hunks = frames.len() / 2;
    let metadata_offset = 124 + hunks * 4;
    let mut bytes = vec![0_u8; HUNK];
    bytes[..8].copy_from_slice(b"MComprHD");
    bytes[8..12].copy_from_slice(&124_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(&5_u32.to_be_bytes());
    bytes[32..40].copy_from_slice(&((frames.len() * FRAME) as u64).to_be_bytes());
    bytes[40..48].copy_from_slice(&124_u64.to_be_bytes());
    bytes[48..56].copy_from_slice(&(metadata_offset as u64).to_be_bytes());
    bytes[56..60].copy_from_slice(&(HUNK as u32).to_be_bytes());
    bytes[60..64].copy_from_slice(&(FRAME as u32).to_be_bytes());
    for hunk in 0..hunks {
        bytes[124 + hunk * 4..128 + hunk * 4].copy_from_slice(&((hunk + 1) as u32).to_be_bytes());
    }
    let mut offset = metadata_offset;
    for (index, (tag, text)) in metadata.iter().enumerate() {
        let end = offset + 16 + text.len() + 1;
        bytes[offset..offset + 4].copy_from_slice(*tag);
        bytes[offset + 4..offset + 8].copy_from_slice(&((text.len() + 1) as u32).to_be_bytes());
        let next = if index + 1 == metadata.len() { 0 } else { end };
        bytes[offset + 8..offset + 16].copy_from_slice(&(next as u64).to_be_bytes());
        bytes[offset + 16..end - 1].copy_from_slice(text.as_bytes());
        offset = end;
    }
    for frame in frames {
        bytes.extend_from_slice(frame);
    }
    bytes
}

fn data_frame(value: u8) -> [u8; FRAME] {
    let mut frame = [0; FRAME];
    frame[1..11].fill(0xff);
    frame[15] = 2;
    frame[18] = 8;
    frame[22] = 8;
    frame[24..2072].fill(value);
    frame
}

fn audio_frame(sample: [u8; 2]) -> [u8; FRAME] {
    let mut frame = [0; FRAME];
    for bytes in frame[..2352].as_chunks_mut::<2>().0 {
        bytes.copy_from_slice(&sample);
    }
    frame[2352..].fill(0x99);
    frame
}

fn assert_audio(disc: &DiscImage, lba: u32, sample: [u8; 2]) {
    let expected: Vec<_> = sample.into_iter().cycle().take(2352).collect();
    assert_eq!(disc.read_sector(lba).unwrap().as_slice(), expected);
}

#[test]
fn mixed_tracks_skip_padding_and_preserve_pregaps_and_audio() {
    let fixture = Fixture::new();
    let mut frames = vec![[0xee; FRAME]; 16];
    for (index, frame) in frames[..5].iter_mut().enumerate() {
        *frame = data_frame(0x10 + index as u8);
    }
    frames[8] = audio_frame([0x12, 0x34]);
    frames[9] = audio_frame([0x56, 0x78]);
    frames[10] = audio_frame([0x9a, 0xbc]);
    frames[12] = audio_frame([0xde, 0xf0]);
    frames[13] = audio_frame([0x23, 0x45]);
    let image = chd(
        &[
            (
                b"CHT2",
                "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:5 PREGAP:0 PGTYPE:MODE2_RAW PGSUB:NONE POSTGAP:0",
            ),
            (
                b"CHT2",
                "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:3 PREGAP:1 PGTYPE:VAUDIO PGSUB:NONE POSTGAP:0",
            ),
            (
                b"CHT2",
                "TRACK:3 TYPE:AUDIO SUBTYPE:NONE FRAMES:2 PREGAP:2 PGTYPE:AUDIO PGSUB:NONE POSTGAP:0",
            ),
        ],
        &frames,
    );
    let path = fixture.image("mixed.CHD", &image);
    let probe = DiscImage::probe(&path).unwrap();
    assert_eq!(probe.dependencies, [] as [PathBuf; 0]);
    assert!(probe.serial.is_none());
    let disc = DiscImage::open(&path).unwrap();
    let tracks: Vec<_> = disc.tracks().collect();
    assert_eq!(disc.sectors, 12);
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[0].kind, TrackKind::Mode2);
    assert_eq!(tracks[1].kind, TrackKind::Audio);
    assert_eq!(
        (tracks[1].index0_lba, tracks[1].start_lba, tracks[1].end_lba),
        (5, 6, 8)
    );
    assert_eq!(
        (tracks[2].index0_lba, tracks[2].start_lba, tracks[2].end_lba),
        (8, 10, 12)
    );
    assert_eq!(disc.read_data(4).unwrap(), [0x14; 2048]);
    assert_audio(&disc, 5, [0x34, 0x12]);
    assert_audio(&disc, 6, [0x78, 0x56]);
    assert_audio(&disc, 7, [0xbc, 0x9a]);
    assert_eq!(disc.read_sector(8).unwrap(), [0; 2352]);
    assert_eq!(disc.read_sector(9).unwrap(), [0; 2352]);
    assert_audio(&disc, 10, [0xf0, 0xde]);
    assert_audio(&disc, 11, [0x45, 0x23]);
    assert!(disc.read_data(6).is_err());
    assert!(disc.read_sector(12).is_err());
    let cloned = disc.clone();
    assert_eq!(cloned.read_data(0).unwrap(), [0x10; 2048]);
    assert_audio(&disc, 11, [0x45, 0x23]);
    assert_eq!(cloned.read_data(3).unwrap(), [0x13; 2048]);
}

#[test]
fn cooked_tracks_reconstruct_raw_headers_and_data() {
    let fixture = Fixture::new();
    for kind in ["MODE1", "MODE2_FORM1", "MODE2"] {
        let mut frame = [0; FRAME];
        if kind == "MODE2" {
            frame[2] = 8;
            frame[6] = 8;
            frame[8..2056].fill(0x5a);
        } else {
            frame[..2048].fill(0x5a);
        }
        let metadata = format!("TRACK:1 TYPE:{kind} SUBTYPE:NONE FRAMES:1");
        let image = chd(&[(b"CHTR", &metadata)], &[frame; 4]);
        let path = fixture.image("cooked.chd", &image);
        let disc = DiscImage::open(&path).unwrap();
        assert_eq!(disc.read_data(0).unwrap(), [0x5a; 2048], "{kind}");
        let raw = disc.read_sector(0).unwrap();
        assert_eq!(
            &raw[..12],
            &[0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0]
        );
        assert_eq!(&raw[12..15], &[0, 2, 0]);
        assert_eq!(raw[15], if kind == "MODE1" { 1 } else { 2 });
    }
}

#[test]
fn first_track_leadin_and_postgap_preserve_offsets() {
    let fixture = Fixture::new();
    let frames = vec![
        [0xee; FRAME],
        data_frame(1),
        data_frame(2),
        data_frame(3),
        audio_frame([0x12, 0x34]),
        [0xee; FRAME],
        [0xee; FRAME],
        [0xee; FRAME],
    ];
    let image = chd(
        &[
            (
                b"CHT2",
                "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:4 PREGAP:1 PGTYPE:VMODE2_RAW PGSUB:NONE POSTGAP:2",
            ),
            (
                b"CHT2",
                "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:1 PREGAP:0 PGTYPE:AUDIO PGSUB:NONE POSTGAP:0",
            ),
        ],
        &frames,
    );
    let path = fixture.image("leadin.chd", &image);
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.sectors, 6);
    assert_eq!(disc.tracks().next().unwrap().start_lba, 0);
    assert_eq!(disc.tracks().nth(1).unwrap().start_lba, 5);
    assert_eq!(disc.read_data(0).unwrap(), [1; 2048]);
    assert_eq!(disc.read_data(2).unwrap(), [3; 2048]);
    assert_eq!(disc.read_data(3).unwrap(), [0; 2048]);
    assert_eq!(disc.read_data(4).unwrap(), [0; 2048]);
    assert_audio(&disc, 5, [0x34, 0x12]);
    assert!(disc.read_sector(6).is_err());
}

#[test]
fn legacy_metadata_reopens_and_rejects_changed_image() {
    let fixture = Fixture::new();
    let mut image = chd(
        &[(b"CHTR", "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:3")],
        &[data_frame(1), data_frame(2), data_frame(3), [0xee; FRAME]],
    );
    let path = fixture.image("legacy.chd", &image);
    let disc = DiscImage::open(&path).unwrap();
    assert_eq!(disc.sectors, 3);
    let state = bincode::serialize(&disc).unwrap();
    let reopened: DiscImage = bincode::deserialize(&state).unwrap();
    assert_eq!(reopened.identity(), disc.identity());
    assert_eq!(reopened.read_data(2).unwrap(), [3; 2048]);
    drop(reopened);
    drop(disc);
    image[HUNK + 2 * FRAME + 24] ^= 1;
    std::fs::write(&path, &image).unwrap();
    assert!(bincode::deserialize::<DiscImage>(&state).is_err());
}

#[test]
fn invalid_metadata_and_truncated_hunks_return_errors() {
    let fixture = Fixture::new();
    let frames = [data_frame(1); 4];
    let multisession = chd(
        &[
            (b"CHTR", "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:4"),
            (b"CHSE", "SESSION:2"),
        ],
        &frames,
    );
    assert!(DiscImage::open(&fixture.image("multisession.chd", &multisession)).is_err());
    for metadata in [
        "TRACK:2 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:4",
        "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:0",
        "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:5",
        "TRACK:1 TYPE:UNKNOWN SUBTYPE:NONE FRAMES:4",
    ] {
        let path = fixture.image("invalid.chd", &chd(&[(b"CHTR", metadata)], &frames));
        assert!(DiscImage::open(&path).is_err(), "{metadata}");
    }
    let mut image = chd(
        &[(b"CHTR", "TRACK:1 TYPE:MODE2_RAW SUBTYPE:NONE FRAMES:4")],
        &frames,
    );
    image.truncate(image.len() - 1);
    let path = fixture.image("truncated.chd", &image);
    if let Ok(disc) = DiscImage::open(&path) {
        assert!(disc.read_sector(3).is_err());
    }
    image.truncate(24);
    let path = fixture.image("header.chd", &image);
    assert!(DiscImage::open(&path).is_err());
}

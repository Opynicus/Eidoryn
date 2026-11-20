//! CD-ROM CHDs use 2448-byte frames and pad each stored track to four frames.
//! Track metadata and audio byte order follow MAME's src/lib/util/cdrom.cpp.
use crate::{
    EmuError,
    disc::{DiscLayout, DiscReader, SYNC, Track, TrackInfo, TrackKind, edc, invalid, msf},
};
use chd::{Chd, header::Header, metadata::MetadataTag};
use std::{
    fmt,
    fs::File,
    path::Path,
    sync::{Arc, Mutex},
};

const FRAME_SIZE: u32 = 2448;
const TRACK_TAG: u32 = u32::from_be_bytes(*b"CHTR");
const TRACK2_TAG: u32 = u32::from_be_bytes(*b"CHT2");
const MAX_SECTORS: u32 = 359_849;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Mode1,
    Mode1Raw,
    Mode2Form1,
    Mode2Form2,
    Mode2Mixed,
    Mode2Raw,
    Audio,
}

impl Format {
    fn parse(text: &str) -> Result<Self, EmuError> {
        match text {
            "MODE1" => Ok(Self::Mode1),
            "MODE1_RAW" => Ok(Self::Mode1Raw),
            "MODE2_FORM1" => Ok(Self::Mode2Form1),
            "MODE2_FORM2" => Ok(Self::Mode2Form2),
            "MODE2" | "MODE2_FORM_MIX" => Ok(Self::Mode2Mixed),
            "MODE2_RAW" => Ok(Self::Mode2Raw),
            "AUDIO" => Ok(Self::Audio),
            _ => Err(invalid(format!("Unsupported CHD track type: {text}"))),
        }
    }

    fn kind(self) -> TrackKind {
        match self {
            Self::Mode1 | Self::Mode1Raw => TrackKind::Mode1,
            Self::Audio => TrackKind::Audio,
            _ => TrackKind::Mode2,
        }
    }
}

#[derive(Debug)]
struct ChdTrack {
    format: Format,
    stored_end: u32,
}

pub(crate) struct Reader {
    image: Chd<File>,
    tracks: Vec<ChdTrack>,
    compressed: Vec<u8>,
    hunk: Vec<u8>,
    cached_hunk: Option<u32>,
}

impl fmt::Debug for Reader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChdReader")
            .field("tracks", &self.tracks)
            .field("cached_hunk", &self.cached_hunk)
            .finish_non_exhaustive()
    }
}

impl Reader {
    pub(crate) fn file(&mut self) -> &mut File {
        self.image.inner()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        size_of::<Self>()
            + self.tracks.capacity() * size_of::<ChdTrack>()
            + self.compressed.capacity()
            + self.hunk.capacity()
            // Include the decoder's hunk map in the conservative rewind estimate.
            + self.image.header().hunk_count() as usize * 16
    }

    pub(crate) fn read_sector(&mut self, track: &Track, lba: u32) -> Result<[u8; 2352], EmuError> {
        let metadata = &self.tracks[usize::from(track.info.number - 1)];
        let format = metadata.format;
        let relative = lba - track.info.index0_lba;
        let frame = track.file_offset + relative.saturating_sub(track.gap);
        let synthetic = relative < track.gap || frame >= metadata.stored_end;
        let mut sector = [0; 2352];
        if !synthetic {
            let frames_per_hunk = self.image.header().hunk_size() / FRAME_SIZE;
            let hunk_number = frame / frames_per_hunk;
            if self.cached_hunk != Some(hunk_number) {
                // A failed decode must not leave the old cache marker on partially written data.
                self.cached_hunk = None;
                let count = self
                    .image
                    .hunk(hunk_number)
                    .and_then(|mut hunk| hunk.read_hunk_in(&mut self.compressed, &mut self.hunk))
                    .map_err(|error| invalid(format!("CHD hunk {hunk_number}: {error}")))?;
                if count != self.hunk.len() {
                    return Err(invalid("Truncated CHD hunk"));
                }
                self.cached_hunk = Some(hunk_number);
            }
            let offset = (frame % frames_per_hunk * FRAME_SIZE) as usize;
            sector.copy_from_slice(&self.hunk[offset..offset + 2352]);
        }
        if format == Format::Audio {
            // CHD stores CDDA samples big endian; BIN and the SPU consume little endian.
            for sample in sector.as_chunks_mut::<2>().0 {
                sample.swap(0, 1);
            }
            return Ok(sector);
        }
        if synthetic || !matches!(format, Format::Mode1Raw | Format::Mode2Raw) {
            match format {
                Format::Mode1 => {
                    sector.copy_within(..2048, 16);
                    sector[2064..].fill(0);
                }
                Format::Mode2Form1 | Format::Mode2Form2 => {
                    let length = if format == Format::Mode2Form1 {
                        2048
                    } else {
                        2324
                    };
                    sector.copy_within(..length, 24);
                    sector[16..24].fill(0);
                    sector[18] = if format == Format::Mode2Form1 {
                        8
                    } else {
                        0x28
                    };
                    sector[22] = sector[18];
                    sector[24 + length..].fill(0);
                }
                Format::Mode2Mixed => sector.copy_within(..2336, 16),
                _ => {}
            }
            sector[..12].copy_from_slice(&SYNC);
            sector[12..15].copy_from_slice(&msf(lba + 150));
            sector[15] = if format.kind() == TrackKind::Mode1 {
                1
            } else {
                2
            };
            // As for cooked ISOs, synthesize EDC but not mastering-only ECC parity.
            match format {
                Format::Mode1 => {
                    let checksum = edc(&sector[..2064]);
                    sector[2064..2068].copy_from_slice(&checksum.to_le_bytes());
                }
                Format::Mode2Form1 | Format::Mode2Form2 => {
                    let end = if format == Format::Mode2Form1 {
                        2072
                    } else {
                        2348
                    };
                    let checksum = edc(&sector[16..end]);
                    sector[end..end + 4].copy_from_slice(&checksum.to_le_bytes());
                }
                _ => {}
            }
        }
        let mode = if format.kind() == TrackKind::Mode1 {
            1
        } else {
            2
        };
        if sector[..12] != SYNC || sector[15] != mode {
            return Err(invalid(format!(
                "Invalid CHD data-sector header at LBA {lba}"
            )));
        }
        Ok(sector)
    }
}

#[allow(clippy::too_many_lines)] // Validate metadata and track extents before publishing the image.
pub(crate) fn open(path: &Path) -> Result<DiscLayout, EmuError> {
    let mut file =
        File::open(path).map_err(|error| invalid(format!("{}: {error}", path.display())))?;
    if !file
        .metadata()
        .map_err(|error| invalid(error.to_string()))?
        .is_file()
    {
        return Err(invalid("CHD must be a regular file"));
    }
    // Bound allocations before the decoder constructs its hunk map and codecs.
    let header = Header::try_read_header(&mut file)
        .map_err(|error| invalid(format!("CHD header: {error}")))?;
    if header.has_parent() {
        return Err(invalid(
            "CHD requires a parent image; use a standalone CD-ROM CHD",
        ));
    }
    if header.unit_bytes() != FRAME_SIZE
        || header.hunk_size() == 0
        || header.hunk_size() % FRAME_SIZE != 0
        || header.hunk_size() > 16 * 1024 * 1024
        || header.logical_bytes() == 0
        || header.logical_bytes() % u64::from(FRAME_SIZE) != 0
        || header.unit_count() > u64::from(MAX_SECTORS + 99 * 3)
    {
        return Err(invalid(
            "Invalid CHD CD-ROM frame size, hunk size, or CD address range",
        ));
    }
    let mut image = Chd::open(file, None).map_err(|error| invalid(format!("CHD: {error}")))?;
    let refs = image
        .metadata_refs()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(format!("CHD metadata: {error}")))?;
    let mut layouts = Vec::new();
    let mut formats = Vec::new();
    let mut cursor = 0_u32;
    let mut stored = 0_u32;
    for reference in refs {
        let tag = reference.metatag();
        if tag == u32::from_be_bytes(*b"CHSE") {
            return Err(invalid("Multi-session CHD images are not supported"));
        }
        if !matches!(tag, TRACK_TAG | TRACK2_TAG) {
            continue;
        }
        let metadata = reference
            .read(image.inner())
            .map_err(|error| invalid(error.to_string()))?;
        let text = std::str::from_utf8(&metadata.value)
            .map_err(|_| invalid("CHD track metadata is not UTF-8"))?
            .trim_end_matches('\0');
        let fields = text
            .split_ascii_whitespace()
            .map(|field| {
                field
                    .split_once(':')
                    .ok_or_else(|| invalid("Invalid CHD track metadata"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let field = |key| -> Result<&str, EmuError> {
            let mut values = fields.iter().filter(|(name, _)| *name == key);
            let value = values
                .next()
                .ok_or_else(|| invalid(format!("CHD metadata lacks {key}")))?
                .1;
            if values.next().is_some() {
                return Err(invalid(format!("Duplicate CHD metadata field: {key}")));
            }
            Ok(value)
        };
        let number = |key| -> Result<u32, EmuError> {
            field(key)?
                .parse::<u32>()
                .map_err(|_| invalid(format!("Invalid CHD metadata number: {key}")))
        };
        let track_number = number("TRACK")?;
        if layouts.len() >= 99 || track_number != layouts.len() as u32 + 1 {
            return Err(invalid("CHD tracks must be consecutive, starting at 1"));
        }
        let format = Format::parse(field("TYPE")?)?;
        if !matches!(field("SUBTYPE")?, "NONE" | "RW" | "RW_RAW") {
            return Err(invalid("Unsupported CHD subchannel type"));
        }
        let frames = number("FRAMES")?;
        let (pregap, stored_pregap, postgap) = if tag == TRACK2_TAG {
            let pregap = number("PREGAP")?;
            let pgtype = field("PGTYPE")?;
            let stored_pregap = pgtype.starts_with('V');
            if stored_pregap && pregap > 0 && Format::parse(&pgtype[1..])? != format {
                return Err(invalid("CHD stored pregap must match its track's format"));
            }
            if !matches!(field("PGSUB")?, "NONE" | "RW" | "RW_RAW") {
                return Err(invalid("Unsupported CHD pregap subchannel type"));
            }
            (pregap, stored_pregap, number("POSTGAP")?)
        } else {
            (0, false, 0)
        };
        if frames == 0
            || frames > MAX_SECTORS
            || pregap > MAX_SECTORS
            || postgap > MAX_SECTORS
            || (stored_pregap && pregap >= frames)
        {
            return Err(invalid("Invalid CHD track extent or pregap"));
        }
        // First-track index 0 is the lead-in, outside nonnegative disc LBAs.
        let skip = if layouts.is_empty() && stored_pregap {
            pregap
        } else {
            0
        };
        let gap = if !layouts.is_empty() && !stored_pregap {
            pregap
        } else {
            0
        };
        let length = frames - skip + gap + postgap;
        if cursor + length > MAX_SECTORS {
            return Err(invalid("CHD exceeds supported CD address range"));
        }
        layouts.push((
            TrackInfo {
                number: track_number as u8,
                kind: format.kind(),
                index0_lba: cursor,
                start_lba: cursor + if layouts.is_empty() { 0 } else { pregap },
                end_lba: cursor + length,
            },
            stored + skip,
            gap,
        ));
        formats.push(ChdTrack {
            format,
            stored_end: stored + frames,
        });
        cursor += length;
        stored += frames.div_ceil(4) * 4;
    }
    if layouts.is_empty() || u64::from(stored) != image.header().unit_count() {
        return Err(invalid(
            "CHD CD-ROM track metadata is missing or does not match its data length",
        ));
    }
    let hunk = image.get_hunksized_buffer();
    let reader = Arc::new(Mutex::new(DiscReader::Chd(Box::new(Reader {
        image,
        tracks: formats,
        compressed: Vec::new(),
        hunk,
        cached_hunk: None,
    }))));
    let tracks = layouts
        .into_iter()
        .map(|(info, file_offset, gap)| Track {
            info,
            file: Arc::clone(&reader),
            file_offset,
            gap,
            cooked: false,
        })
        .collect();
    Ok(DiscLayout::new(tracks, cursor, vec![]))
}

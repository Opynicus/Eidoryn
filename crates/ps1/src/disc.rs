//! ISO/BIN/CUE/CHD image and bounded ISO-9660 reader. No emulator dependencies.
//! Layout reference: <https://psx-spx.consoledev.net/cdromformat/>
use crate::{EmuError, bus::RAM_SIZE};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) const SYNC: [u8; 12] = [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0];

#[derive(Debug)]
pub(crate) enum DiscReader {
    File(File),
    Chd(Box<crate::chd::Reader>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackKind {
    #[default]
    Mode1,
    Mode2,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscRegion {
    America,
    Europe,
    Japan,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "DiscRegion")]
pub(crate) enum DiscRegionV1 {
    America,
    Europe,
    Japan,
}

impl From<&DiscRegion> for DiscRegionV1 {
    fn from(value: &DiscRegion) -> Self {
        match value {
            DiscRegion::America => Self::America,
            DiscRegion::Europe => Self::Europe,
            DiscRegion::Japan => Self::Japan,
        }
    }
}

impl From<DiscRegionV1> for DiscRegion {
    fn from(value: DiscRegionV1) -> Self {
        match value {
            DiscRegionV1::America => Self::America,
            DiscRegionV1::Europe => Self::Europe,
            DiscRegionV1::Japan => Self::Japan,
        }
    }
}

crate::state::v1::impl_serde_v1!(DiscRegion, DiscRegionV1);

impl DiscRegion {
    pub fn sce_letter(self) -> u8 {
        match self {
            Self::America => b'A',
            Self::Europe => b'E',
            Self::Japan => b'I',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub number: u8,
    pub kind: TrackKind,
    pub index0_lba: u32,
    pub start_lba: u32,
    pub end_lba: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Track {
    pub info: TrackInfo,
    pub file: Arc<Mutex<DiscReader>>,
    pub file_offset: u32,
    pub gap: u32,
    pub cooked: bool,
}

#[derive(Clone, Debug)]
pub struct DiscImage {
    layout: DiscLayout,
    pub sectors: u32,
    source: PathBuf,
    identity: String,
}

/// Bounded evidence for a desktop index. Only `DiscImage::open` produces a
/// verified save-state identity.
#[derive(Debug)]
pub struct DiscProbe {
    pub dependencies: Vec<PathBuf>,
    pub serial: Option<String>,
    pub boot_value: Option<String>,
    pub region: Option<DiscRegion>,
    /// Layout and a few sample sectors; a candidate key, never proof of equality.
    pub sample: String,
}

#[derive(Clone, Debug)]
pub(crate) struct DiscLayout {
    tracks: Vec<Track>,
    sectors: u32,
    dependencies: Vec<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "DiscReference")]
pub(crate) struct DiscReferenceV1 {
    source: PathBuf,
    identity: String,
}

impl From<&DiscImage> for DiscReferenceV1 {
    fn from(disc: &DiscImage) -> Self {
        Self {
            source: disc.source.clone(),
            identity: disc.identity.clone(),
        }
    }
}

impl TryFrom<DiscReferenceV1> for DiscImage {
    type Error = String;

    fn try_from(reference: DiscReferenceV1) -> Result<Self, Self::Error> {
        let disc = Self::open(&reference.source).map_err(|error| error.to_string())?;
        if disc.identity != reference.identity {
            return Err("Disc image has changed since the save state was created".into());
        }
        Ok(disc)
    }
}

crate::state::v1::impl_serde_v1!(DiscImage, DiscReferenceV1);

pub(crate) fn invalid(message: impl Into<String>) -> EmuError {
    EmuError::InvalidImage(message.into())
}

impl DiscImage {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.layout.tracks.capacity() * size_of::<Track>()
            + self.source.capacity()
            + self.identity.capacity()
            + self
                .layout
                .dependencies
                .iter()
                .map(PathBuf::capacity)
                .sum::<usize>()
            + self
                .layout
                .tracks
                .iter()
                .map(|track| {
                    crate::state::shared_bytes(track.file.as_ref())
                        + track.file.lock().map_or(0, |reader| match &*reader {
                            DiscReader::File(_) => 0,
                            DiscReader::Chd(reader) => reader.heap_bytes(),
                        })
                })
                .sum::<usize>()
    }

    pub fn open(path: &Path) -> Result<Self, EmuError> {
        let layout = DiscLayout::open(path)?;
        let source = path
            .canonicalize()
            .map_err(|error| invalid(error.to_string()))?;
        let identity = layout.identity()?;
        Ok(Self {
            sectors: layout.sectors,
            layout,
            source,
            identity,
        })
    }

    /// Read the layout, license sector and bounded ISO boot configuration without
    /// reading the executable or hashing the complete image.
    pub fn probe(path: &Path) -> Result<DiscProbe, EmuError> {
        let layout = DiscLayout::open(path)?;
        // A naked BIN has no descriptor. Sector-aligned firmware and arbitrary
        // PCM are common in game folders. Accept cooked BIN only when its ISO
        // volume is recognizable; raw data BIN already passed header checks.
        if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("bin"))
            && layout.tracks.first().is_some_and(|t| t.cooked)
        {
            layout.root()?;
        }
        let region = layout.license_region()?;
        let boot_value = layout.boot_value().ok().flatten();
        let serial = boot_value.as_deref().and_then(normalize_boot_serial);
        let mut sample = Sha256::new();
        sample.update(layout.sectors.to_le_bytes());
        for t in &layout.tracks {
            sample.update([t.info.number, t.info.kind as u8]);
            for lba in [t.info.index0_lba, t.info.start_lba, t.info.end_lba] {
                sample.update(lba.to_le_bytes());
            }
        }
        // Sampling is bounded independently of the disc size and track count.
        for lba in [0, 4, 16, layout.sectors / 2, layout.sectors - 1] {
            if lba < layout.sectors {
                sample.update(layout.read_sector(lba)?);
            }
        }
        Ok(DiscProbe {
            dependencies: layout.dependencies,
            boot_value,
            serial,
            region,
            sample: format!("{:x}", sample.finalize()),
        })
    }

    /// List the dependencies of a validated CUE layout without reading track sectors.
    pub fn cue_dependencies(path: &Path) -> Result<Vec<PathBuf>, EmuError> {
        Ok(crate::cue::open(path)?.dependencies)
    }

    pub fn supports_path(path: &Path) -> bool {
        path.extension().is_some_and(|ext| {
            ["cue", "chd", "iso", "bin"]
                .iter()
                .any(|v| ext.eq_ignore_ascii_case(v))
        })
    }

    /// Content and track-layout identity, independent of the image's filename.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn tracks(&self) -> impl Iterator<Item = &TrackInfo> {
        self.layout.tracks()
    }
    pub fn track_at(&self, lba: u32) -> Option<&TrackInfo> {
        self.layout.track_at(lba)
    }
    pub fn license_region(&self) -> Result<Option<DiscRegion>, EmuError> {
        self.layout.license_region()
    }
    pub fn read_sector(&self, lba: u32) -> Result<[u8; 2352], EmuError> {
        self.layout.read_sector(lba)
    }
    pub fn read_data(&self, lba: u32) -> Result<[u8; 2048], EmuError> {
        self.layout.read_data(lba)
    }
    pub fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>, EmuError> {
        self.layout.read_file(path, limit)
    }
    /// Host-assisted homebrew boot, explicitly bypassing BIOS initialization.
    pub fn boot_executable(&self) -> Result<Vec<u8>, EmuError> {
        let boot = self.layout.boot_value()?;
        let path = match boot.as_deref() {
            Some(boot) => boot_file(boot)?,
            None => "PSX.EXE;1",
        };
        self.read_file(path, RAM_SIZE + 0x800)
    }
}

impl DiscLayout {
    fn open(path: &Path) -> Result<Self, EmuError> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("cue") {
            return crate::cue::open(path);
        }
        if ext.eq_ignore_ascii_case("chd") {
            return crate::chd::open(path);
        }
        if !["iso", "bin"].iter().any(|s| ext.eq_ignore_ascii_case(s)) {
            return Err(invalid(
                "Supported disc formats: ISO, raw data BIN, BINARY CUE/BIN, and CD-ROM CHD",
            ));
        }
        let metadata =
            std::fs::metadata(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        if !metadata.is_file() || metadata.len() < 2048 {
            return Err(invalid("Disc must be a nonempty regular ISO/BIN file"));
        }
        let mut file = File::open(path).map_err(|e| invalid(e.to_string()))?;
        let mut header = [0; 16];
        file.read_exact(&mut header)
            .map_err(|e| invalid(e.to_string()))?;
        let raw = header[..12] == SYNC;
        let size = if raw { 2352 } else { 2048 };
        if metadata.len() % size != 0
            || metadata.len() / size > 359_849
            || (raw && !matches!(header[15], 1 | 2))
        {
            return Err(invalid(
                "Invalid disc sector size, mode, or CD address range",
            ));
        }
        let sectors = (metadata.len() / size) as u32;
        Ok(Self::new(
            vec![Track {
                info: TrackInfo {
                    number: 1,
                    kind: if raw && header[15] == 1 {
                        TrackKind::Mode1
                    } else {
                        TrackKind::Mode2
                    },
                    index0_lba: 0,
                    start_lba: 0,
                    end_lba: sectors,
                },
                file: Arc::new(Mutex::new(DiscReader::File(file))),
                file_offset: 0,
                gap: 0,
                cooked: !raw,
            }],
            sectors,
            vec![],
        ))
    }

    pub(crate) fn new(tracks: Vec<Track>, sectors: u32, dependencies: Vec<PathBuf>) -> Self {
        Self {
            tracks,
            sectors,
            dependencies,
        }
    }

    fn identity(&self) -> Result<String, EmuError> {
        let mut digest = Sha256::new();
        digest.update(b"Eidoryn disc identity v1");
        digest.update(self.sectors.to_le_bytes());
        let mut files = Vec::new();
        for track in &self.tracks {
            digest.update([
                track.info.number,
                track.info.kind as u8,
                u8::from(track.cooked),
            ]);
            for value in [
                track.info.index0_lba,
                track.info.start_lba,
                track.info.end_lba,
                track.file_offset,
                track.gap,
            ] {
                digest.update(value.to_le_bytes());
            }
            let file_index =
                if let Some(index) = files.iter().position(|file| Arc::ptr_eq(file, &track.file)) {
                    index
                } else {
                    files.push(Arc::clone(&track.file));
                    files.len() - 1
                };
            digest.update((file_index as u32).to_le_bytes());
        }
        for file in files {
            let mut reader = file
                .lock()
                .map_err(|_| invalid("Disc image lock poisoned"))?;
            let file = match &mut *reader {
                DiscReader::File(file) => file,
                DiscReader::Chd(reader) => reader.file(),
            };
            let length = file
                .metadata()
                .map_err(|error| invalid(error.to_string()))?
                .len();
            digest.update(length.to_le_bytes());
            file.seek(SeekFrom::Start(0))
                .map_err(|error| invalid(error.to_string()))?;
            let mut buffer = vec![0_u8; 65_536];
            let mut remaining = length;
            while remaining > 0 {
                let count = usize::try_from(remaining.min(buffer.len() as u64))
                    .expect("bounded disc hash read");
                file.read_exact(&mut buffer[..count])
                    .map_err(|error| invalid(error.to_string()))?;
                digest.update(&buffer[..count]);
                remaining -= count as u64;
            }
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    pub fn tracks(&self) -> impl Iterator<Item = &TrackInfo> {
        self.tracks.iter().map(|t| &t.info)
    }

    pub fn track_at(&self, lba: u32) -> Option<&TrackInfo> {
        self.tracks()
            .find(|t| lba >= t.index0_lba && lba < t.end_lba)
    }

    /// Infer the emulated SCEx region from the documented sector-4 license text.
    /// ISO/BIN images do not preserve the physical disc's authentication wobble.
    /// Unknown, audio and Mode 1 media stay unlicensed; no BIOS code is patched.
    pub fn license_region(&self) -> Result<Option<DiscRegion>, EmuError> {
        if self.sectors <= 4 || self.track_at(4).is_none_or(|t| t.kind != TrackKind::Mode2) {
            return Ok(None);
        }
        let sector = self.read_data(4)?;
        if &sector[..32] != b"          Licensed  by          " {
            return Ok(None);
        }
        let name: Vec<u8> = sector[32..70]
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        Ok(if name.starts_with(b"SonyComputerEntertainmentAmerica") {
            Some(DiscRegion::America)
        } else if name.starts_with(b"SonyComputerEntertainmentEurope") {
            Some(DiscRegion::Europe)
        } else if name.starts_with(b"SonyComputerEntertainmentInc.") {
            Some(DiscRegion::Japan)
        } else {
            None
        })
    }

    pub fn read_sector(&self, lba: u32) -> Result<[u8; 2352], EmuError> {
        let track = self
            .tracks
            .iter()
            .find(|t| lba >= t.info.index0_lba && lba < t.info.end_lba)
            .ok_or_else(|| invalid(format!("CD sector {lba} is beyond end of image")))?;
        let mut bytes = [0; 2352];
        let relative = lba - track.info.index0_lba;
        let mut reader = track
            .file
            .lock()
            .map_err(|_| invalid("Disc reader lock poisoned"))?;
        let file = match &mut *reader {
            DiscReader::File(file) => file,
            DiscReader::Chd(reader) => return reader.read_sector(track, lba),
        };
        if relative < track.gap {
            return Ok(bytes);
        } // Synthetic audio pregap is silence.
        let size = if track.cooked { 2048 } else { 2352 };
        file.seek(SeekFrom::Start(
            u64::from(track.file_offset + relative - track.gap) * size,
        ))
        .map_err(|e| invalid(e.to_string()))?;
        if track.cooked {
            bytes[..12].copy_from_slice(&SYNC);
            bytes[12..15].copy_from_slice(&msf(lba + 150));
            bytes[15] = 2;
            bytes[18] = 8;
            bytes[22] = 8;
            file.read_exact(&mut bytes[24..2072])
                .map_err(|e| invalid(e.to_string()))?;
            let edc = edc(&bytes[16..2072]);
            bytes[2072..2076].copy_from_slice(&edc.to_le_bytes());
            // ECC parity is not synthesized. This image is for emulation, not CD mastering.
        } else {
            file.read_exact(&mut bytes)
                .map_err(|e| invalid(e.to_string()))?;
            if track.info.kind != TrackKind::Audio
                && (bytes[..12] != SYNC
                    || bytes[15]
                        != if track.info.kind == TrackKind::Mode1 {
                            1
                        } else {
                            2
                        })
            {
                return Err(invalid(format!("Invalid data-sector header at LBA {lba}")));
            }
        }
        Ok(bytes)
    }

    pub fn read_data(&self, lba: u32) -> Result<[u8; 2048], EmuError> {
        if self
            .track_at(lba)
            .is_some_and(|t| t.kind == TrackKind::Audio)
        {
            return Err(invalid("Cannot read an audio track as ISO data"));
        }
        let sector = self.read_sector(lba)?;
        let offset = data_offset(&sector)?;
        Ok(sector[offset..offset + 2048]
            .try_into()
            .expect("fixed sector bounds"))
    }

    fn extent(&self, entry: &Entry, limit: usize) -> Result<Vec<u8>, EmuError> {
        if entry.length as usize > limit
            || u64::from(entry.lba) + u64::from(entry.length).div_ceil(2048)
                > u64::from(self.sectors)
        {
            return Err(invalid("ISO extent exceeds image or allocation limit"));
        }
        let mut result = Vec::with_capacity(entry.length as usize);
        for lba in entry.lba..entry.lba + entry.length.div_ceil(2048) {
            let sector = self.read_data(lba)?;
            let n = (entry.length as usize - result.len()).min(2048);
            result.extend_from_slice(&sector[..n]);
        }
        Ok(result)
    }

    fn root(&self) -> Result<Entry, EmuError> {
        for lba in 16..self.sectors.min(48) {
            let descriptor = self.read_data(lba)?;
            if &descriptor[1..6] != b"CD001" || descriptor[6] != 1 {
                return Err(invalid("Invalid ISO-9660 volume descriptor"));
            }
            if descriptor[0] == 255 {
                break;
            }
            if descriptor[0] == 1 {
                if descriptor[128..132] != [0, 8, 8, 0] {
                    return Err(invalid("ISO logical block size must be 2048"));
                }
                let root = Entry::parse(&descriptor[156..])?;
                if !root.directory {
                    return Err(invalid("ISO root is not a directory"));
                }
                return Ok(root);
            }
        }
        Err(invalid("ISO primary volume descriptor not found"))
    }

    fn find(&self, path: &str) -> Result<Option<Entry>, EmuError> {
        let path = path.replace('\\', "/");
        let components: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
        if components.is_empty()
            || components.len() > 16
            || components.iter().any(|s| matches!(*s, "." | ".."))
        {
            return Err(invalid("Invalid ISO file path"));
        }
        let mut current = self.root()?;
        for name in components {
            if !current.directory {
                return Err(invalid("ISO path component is not a directory"));
            }
            let bytes = self.extent(&current, 1024 * 1024)?;
            let mut offset = 0;
            let mut found = None;
            while offset < bytes.len() {
                let length = usize::from(bytes[offset]);
                if length == 0 {
                    offset = (offset / 2048 + 1) * 2048;
                    continue;
                }
                if offset % 2048 + length > 2048 || offset + length > bytes.len() {
                    return Err(invalid("Truncated ISO directory record"));
                }
                let record = &bytes[offset..offset + length];
                if length < 34 || usize::from(record[32]) + 33 > length {
                    return Err(invalid("Invalid ISO filename record"));
                }
                let id = &record[33..33 + usize::from(record[32])];
                let plain = id.split(|b| *b == b';').next().unwrap_or(id);
                if id.eq_ignore_ascii_case(name.as_bytes())
                    || (!name.contains(';') && plain.eq_ignore_ascii_case(name.as_bytes()))
                {
                    found = Some(Entry::parse(record)?);
                    break;
                }
                offset += length;
            }
            let Some(entry) = found else {
                return Ok(None);
            };
            current = entry;
        }
        Ok(Some(current))
    }

    pub fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>, EmuError> {
        let entry = self
            .find(path)?
            .ok_or_else(|| invalid(format!("ISO file not found: {path}")))?;
        if entry.directory {
            return Err(invalid("Expected an ISO file, found a directory"));
        }
        self.extent(&entry, limit)
    }

    fn boot_value(&self) -> Result<Option<String>, EmuError> {
        let Some(config) = self.find("SYSTEM.CNF;1")? else {
            return Ok(None);
        };
        let bytes = self.extent(&config, 4096)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid("Invalid SYSTEM.CNF text"))?;
        let value = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case("BOOT"))
            .map(|(_, value)| value.trim().trim_matches('"'))
            .ok_or_else(|| invalid("SYSTEM.CNF has no BOOT entry"))?;
        Ok(Some(value.to_owned()))
    }
}

fn boot_file(boot: &str) -> Result<&str, EmuError> {
    let (device, file) = boot
        .split_once(':')
        .ok_or_else(|| invalid("BOOT must use cdrom:"))?;
    if !device.eq_ignore_ascii_case("cdrom") {
        return Err(invalid("Unsupported BOOT device"));
    }
    Ok(file)
}

fn normalize_boot_serial(boot: &str) -> Option<String> {
    let filename = boot_file(boot)
        .ok()?
        .rsplit(['/', '\\'])
        .next()?
        .split(';')
        .next()?;
    let bytes = filename.as_bytes();
    if bytes.len() != 11
        || !bytes[..4].iter().all(u8::is_ascii_alphabetic)
        || !matches!(bytes[4], b'_' | b'-')
        || bytes[8] != b'.'
        || !bytes[5..8]
            .iter()
            .chain(&bytes[9..])
            .all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(format!(
        "{}-{}{}",
        filename[..4].to_ascii_uppercase(),
        &filename[5..8],
        &filename[9..]
    ))
}

struct Entry {
    lba: u32,
    length: u32,
    directory: bool,
}
impl Entry {
    fn parse(bytes: &[u8]) -> Result<Self, EmuError> {
        if bytes.len() < 34
            || bytes[0] < 34
            || usize::from(bytes[0]) > bytes.len()
            || bytes[1] != 0
            || bytes[25] & 0x80 != 0
            || bytes[26] != 0
            || bytes[27] != 0
        {
            return Err(invalid("Unsupported or truncated ISO directory extent"));
        }
        let dual = |offset| -> Result<u32, EmuError> {
            let little =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("record bounds"));
            let big = u32::from_be_bytes(
                bytes[offset + 4..offset + 8]
                    .try_into()
                    .expect("record bounds"),
            );
            if little != big {
                return Err(invalid("ISO extent byte orders disagree"));
            }
            Ok(little)
        };
        Ok(Self {
            lba: dual(2)?,
            length: dual(10)?,
            directory: bytes[25] & 2 != 0,
        })
    }
}

pub(crate) fn data_offset(sector: &[u8; 2352]) -> Result<usize, EmuError> {
    match sector[15] {
        1 => Ok(16),
        2 if sector[16..20] == sector[20..24] && sector[18] & 0x20 == 0 => Ok(24),
        _ => Err(invalid("Expected Mode 1 or Mode 2 Form 1 data sector")),
    }
}

pub(crate) fn msf(sector: u32) -> [u8; 3] {
    let bcd = |n: u32| ((n / 10) << 4 | (n % 10)) as u8;
    [bcd(sector / 4500), bcd(sector / 75 % 60), bcd(sector % 75)]
}

// Precompute the same eight polynomial steps for each possible input byte.
// Cooked ISO reads synthesize this checksum for every sector; the table is
// immutable (1 KiB), built at compile time, and needs no runtime allocation.
const EDC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (0xd801_8001 & 0_u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

pub(crate) fn edc(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |crc, &byte| {
        (crc >> 8) ^ EDC_TABLE[usize::from(crc as u8 ^ byte)]
    })
}

#[cfg(test)]
fn bitwise_edc(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xd801_8001 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    #[test]
    fn save_state_reopens_cue_and_rejects_changed_audio() {
        let directory = std::env::temp_dir().join(format!(
            "eidoryn-disc-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let cue = directory.join("game.cue");
        std::fs::write(&cue, "FILE \"data.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 01 00:00:00\n").unwrap();
        std::fs::write(directory.join("data.bin"), [0_u8; 2352 * 2]).unwrap();
        std::fs::write(directory.join("audio.bin"), [42_u8; 2352 * 2]).unwrap();
        let disc = super::DiscImage::open(&cue).unwrap();
        let bytes = bincode::serialize(&disc).unwrap();
        let restored: super::DiscImage = bincode::deserialize(&bytes).unwrap();
        assert_eq!(disc.identity(), restored.identity());
        assert_eq!(
            disc.read_sector(3).unwrap(),
            restored.read_sector(3).unwrap()
        );
        drop(restored);
        drop(disc);
        std::fs::write(directory.join("audio.bin"), [43_u8; 2352 * 2]).unwrap();
        assert!(bincode::deserialize::<super::DiscImage>(&bytes).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cd_edc_check_vector() {
        assert_eq!(super::edc(b"123456789"), 0x6ec2_edc4);
    }

    #[test]
    fn edc_matches_bitwise_reference_for_varied_sectors() {
        for byte in 0..=255 {
            assert_eq!(super::edc(&[byte]), super::bitwise_edc(&[byte]));
        }
        let mut seed = 0x9e37_79b9_u32;
        let mut sector = [0; 2352];
        for case in 0..18 {
            for byte in &mut sector {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *byte = match case {
                    0 => 0,
                    1 => 255,
                    _ => (seed >> 24) as u8,
                };
            }
            for length in [0, 1, 31, 2048, 2064, 2352] {
                let bytes = &sector[..length];
                assert_eq!(super::edc(bytes), super::bitwise_edc(bytes));
            }
        }
    }
}

#[cfg(test)]
mod library_probe_tests {
    use super::normalize_boot_serial;

    #[test]
    fn serial_is_derived_only_from_a_standard_iso_boot_filename() {
        assert_eq!(
            normalize_boot_serial(r"cdrom:\SLUS_000.67;1").as_deref(),
            Some("SLUS-00067")
        );
        assert_eq!(
            normalize_boot_serial("CDROM:/boot/slps_999.99;1").as_deref(),
            Some("SLPS-99999")
        );
        assert_eq!(normalize_boot_serial(r"cdrom:\HOMEBREW.EXE;1"), None);
        assert_eq!(normalize_boot_serial(r"host:\SLUS_000.67;1"), None);
        assert_eq!(normalize_boot_serial(r"cdrom:\SLUS_000.670;1"), None);
    }
}

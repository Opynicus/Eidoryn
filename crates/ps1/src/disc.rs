//! Original ISO/BIN/CUE image and bounded ISO-9660 reader. No emulator dependencies.
//! Layout reference: https://psx-spx.consoledev.net/cdromformat/
use crate::{EmuError, bus::RAM_SIZE};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::{Arc, Mutex},
};

const SYNC: [u8; 12] = [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0];

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
    pub file: Arc<Mutex<File>>,
    pub file_offset: u32,
    pub gap: u32,
    pub cooked: bool,
}

#[derive(Clone, Debug)]
pub struct DiscImage {
    pub(crate) tracks: Vec<Track>,
    pub sectors: u32,
}

pub(crate) fn invalid(message: impl Into<String>) -> EmuError {
    EmuError::InvalidImage(message.into())
}

impl DiscImage {
    pub fn open(path: &Path) -> Result<Self, EmuError> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("cue") {
            return crate::cue::open(path);
        }
        if !["iso", "bin"].iter().any(|s| ext.eq_ignore_ascii_case(s)) {
            return Err(invalid(
                "Supported disc formats: ISO, raw data BIN, and BINARY CUE/BIN; CHD is not implemented",
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
        Ok(Self {
            tracks: vec![Track {
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
                file: Arc::new(Mutex::new(file)),
                file_offset: 0,
                gap: 0,
                cooked: !raw,
            }],
            sectors,
        })
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
        if relative < track.gap {
            return Ok(bytes);
        } // Synthetic audio pregap is silence.
        let mut file = track
            .file
            .lock()
            .map_err(|_| invalid("Disc reader lock poisoned"))?;
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
        let components = path.split(['/', '\\']).filter(|part| !part.is_empty());
        let count = components.clone().count();
        // Validate the complete path before any directory reads, preserving error priority.
        if !(1..=16).contains(&count) || components.clone().any(|part| matches!(part, "." | "..")) {
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

    /// Host-assisted homebrew boot, explicitly bypassing BIOS initialization.
    pub fn boot_executable(&self) -> Result<Vec<u8>, EmuError> {
        if let Some(config) = self.find("SYSTEM.CNF;1")? {
            let bytes = self.extent(&config, 4096)?;
            self.read_file(boot_path(&bytes)?, RAM_SIZE + 0x800)
        } else {
            self.read_file("PSX.EXE;1", RAM_SIZE + 0x800)
        }
    }
}

/// Borrow the boot filename from the bounded configuration buffer.
fn boot_path(config: &[u8]) -> Result<&str, EmuError> {
    let text = std::str::from_utf8(config).map_err(|_| invalid("Invalid SYSTEM.CNF text"))?;
    let boot = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("BOOT"))
        .map(|(_, value)| value.trim().trim_matches('"'))
        .ok_or_else(|| invalid("SYSTEM.CNF has no BOOT entry"))?;
    let (device, file) = boot
        .split_once(':')
        .ok_or_else(|| invalid("BOOT must use cdrom:"))?;
    if !device.eq_ignore_ascii_case("cdrom") {
        return Err(invalid("Unsupported BOOT device"));
    }
    Ok(file)
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

fn edc(bytes: &[u8]) -> u32 {
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
    fn boot_config_preserves_first_entry_quoting_and_errors() {
        for (config, expected) in [
            ("BOOT = cdrom:\\TEST.EXE;1\r\n", "\\TEST.EXE;1"),
            (
                " boot = \"CDROM:/dir/test.exe;1\" \nBOOT=cdrom:second.exe",
                "/dir/test.exe;1",
            ),
            ("BOOT=cdrom:", ""),
        ] {
            assert_eq!(super::boot_path(config.as_bytes()).unwrap(), expected);
        }
        for (config, expected) in [
            (&b"\xff"[..], "Invalid SYSTEM.CNF text"),
            (&b"TCB=4"[..], "SYSTEM.CNF has no BOOT entry"),
            (&b"BOOT=TEST.EXE"[..], "BOOT must use cdrom:"),
            (&b"BOOT=host:TEST.EXE"[..], "Unsupported BOOT device"),
            (
                &b"BOOT=bad\nBOOT=cdrom:valid.exe"[..],
                "BOOT must use cdrom:",
            ),
        ] {
            assert_eq!(super::boot_path(config).unwrap_err().to_string(), expected);
        }
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

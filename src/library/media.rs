//! File format adapters for indexing media without starting an emulation core.
use super::IndexResult;
use eidoryn_gba::{HEADER_LEN, MAX_ROM_LEN};
use eidoryn_ps1::disc::{DiscImage, DiscRegion};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// Stable platform identifiers used in the library database and the desktop core picker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GamePlatform {
    #[default]
    Ps1,
    Gba,
    Nds,
}

impl GamePlatform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ps1 => "PS1",
            Self::Gba => "GBA",
            Self::Nds => "NDS",
        }
    }

    pub(super) fn from_id(name: &str) -> Option<Self> {
        [Self::Ps1, Self::Gba, Self::Nds]
            .into_iter()
            .find(|platform| platform.as_str() == name)
    }

    pub(super) const fn cartridge(self) -> bool {
        matches!(self, Self::Gba | Self::Nds)
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gba"))
        {
            Some(Self::Gba)
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("nds"))
        {
            Some(Self::Nds)
        } else if DiscImage::supports_path(path) {
            Some(Self::Ps1)
        } else {
            None
        }
    }

    pub(super) fn for_file(path: &Path) -> IndexResult<Self> {
        Self::from_path(path)
            .ok_or_else(|| format!("Unsupported game format: {}", path.display()).into())
    }

    pub(super) fn probe(self, path: &Path) -> IndexResult<MediaProbe> {
        match self {
            Self::Ps1 => {
                let disc = DiscImage::probe(path)?;
                Ok(MediaProbe {
                    platform: self,
                    title: None,
                    serial: disc.serial,
                    boot_value: disc.boot_value,
                    region: disc.region.map(|r| match r {
                        DiscRegion::America => "USA",
                        DiscRegion::Europe => "Europe",
                        DiscRegion::Japan => "Japan",
                    }),
                    sample: disc.sample,
                    dependencies: disc.dependencies,
                })
            }
            Self::Gba => probe_gba(path),
            Self::Nds => probe_nds(path),
        }
    }

    pub(super) fn identity(self, path: &Path) -> IndexResult<String> {
        match self {
            Self::Ps1 => Ok(DiscImage::open(path)?.identity().to_owned()),
            Self::Gba | Self::Nds => {
                let mut rom = File::open(path)?;
                let mut hash = Sha256::new();
                std::io::copy(&mut rom, &mut hash)?;
                Ok(format!("{:x}", hash.finalize()))
            }
        }
    }
}

pub(super) struct MediaProbe {
    pub platform: GamePlatform,
    pub title: Option<String>,
    pub serial: Option<String>,
    pub region: Option<&'static str>,
    pub boot_value: Option<String>,
    pub sample: String,
    pub dependencies: Vec<PathBuf>,
}

fn probe_gba(path: &Path) -> IndexResult<MediaProbe> {
    let mut rom = File::open(path)?;
    let len = rom.metadata()?.len();
    if !(HEADER_LEN as u64..=MAX_ROM_LEN as u64).contains(&len) {
        return Err("GBA cartridge size must be between 192 bytes and 32 MiB".into());
    }
    let mut header = [0; HEADER_LEN];
    rom.read_exact(&mut header)?;
    // Homebrew may omit the logo and product code. Accept it, but only use
    // printable header fields as display metadata.
    let title = header[0xa0..0xac]
        .split(|&b| b == 0)
        .next()
        .filter(|bytes| bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' '))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let code = &header[0xac..0xb0];
    let serial = code
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        .then(|| String::from_utf8_lossy(code).into_owned());
    let region = serial.as_ref().and_then(|_| match code[3] {
        b'E' => Some("USA"),
        b'J' => Some("Japan"),
        b'P' | b'X' | b'Y' | b'Z' => Some("Europe"),
        b'K' => Some("Korea"),
        b'C' => Some("China"),
        b'U' => Some("Australia"),
        b'H' => Some("Netherlands"),
        b'Q' => Some("Denmark"),
        b'D' => Some("Germany"),
        b'F' => Some("France"),
        b'I' => Some("Italy"),
        b'S' => Some("Spain"),
        _ => None,
    });
    let sample = sample_cartridge(&mut rom, len)?;
    Ok(MediaProbe {
        platform: GamePlatform::Gba,
        title,
        serial,
        region,
        boot_value: None,
        dependencies: Vec::new(),
        sample,
    })
}

fn probe_nds(path: &Path) -> IndexResult<MediaProbe> {
    let mut rom = File::open(path)?;
    let len = rom.metadata()?.len();
    let mut prefix = [0; eidoryn_nds::HEADER_LEN];
    rom.read_exact(&mut prefix)?;
    let header = eidoryn_nds::Header::parse(&prefix, len)?;
    let serial = header.game_code().map(str::to_owned);
    // DS codes X, Y and Z occur in several continents. Keep ambiguous suffixes
    // unset until the catalog or ROM fingerprint identifies a specific edition.
    let region = serial
        .as_ref()
        .and_then(|code| match code.as_bytes().last()? {
            b'E' => Some("USA"),
            b'J' => Some("Japan"),
            b'K' => Some("Korea"),
            b'C' => Some("China"),
            b'F' => Some("France"),
            b'D' => Some("Germany"),
            b'I' => Some("Italy"),
            b'S' => Some("Spain"),
            b'L' => Some("Canada"),
            b'H' => Some("Netherlands"),
            b'Q' => Some("Denmark"),
            b'G' => Some("Greece"),
            b'U' => Some("Australia"),
            _ => None,
        });
    Ok(MediaProbe {
        platform: GamePlatform::Nds,
        title: Some(header.title())
            .filter(|title| !title.is_empty())
            .map(str::to_owned),
        serial,
        region,
        boot_value: None,
        dependencies: Vec::new(),
        sample: sample_cartridge(&mut rom, len)?,
    })
}

fn sample_cartridge(rom: &mut File, len: u64) -> std::io::Result<String> {
    let mut sample = Sha256::new();
    sample.update(len.to_le_bytes());
    let mut page = [0u8; 4096];
    let count = len.min(page.len() as u64) as usize;
    let last = len - count as u64;
    for offset in [0, last / 2, last] {
        rom.seek(SeekFrom::Start(offset))?;
        rom.read_exact(&mut page[..count])?;
        sample.update(&page[..count]);
    }
    Ok(format!("{:x}", sample.finalize()))
}

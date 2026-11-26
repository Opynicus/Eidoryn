//! Validate both executable descriptors before allocating or mapping guest memory.
use crate::{Error, HEADER_LEN, MAX_ROM_LEN};
use std::ops::Range;

#[derive(Clone, Debug)]
pub(crate) struct Segment {
    pub(crate) source: Range<usize>,
    pub(crate) load: u32,
    pub(crate) entry: u32,
}

/// The fields needed for offline indexing and direct boot of a DS cartridge.
#[derive(Clone, Debug)]
pub struct Header {
    title: String,
    code: Option<String>,
    pub(crate) arm9: Segment,
    pub(crate) arm7: Segment,
}

impl Header {
    /// Parse just the header bytes, validating source ranges against the actual file length.
    pub fn parse(bytes: &[u8], file_len: u64) -> Result<Self, Error> {
        if bytes.len() < HEADER_LEN || !(HEADER_LEN as u64..=MAX_ROM_LEN as u64).contains(&file_len)
        {
            return Err(Error::InvalidRom("invalid DS cartridge length"));
        }
        if !matches!(bytes[0x12], 0 | 2) {
            return Err(Error::InvalidRom(
                "unsupported DS cartridge unit code (DSi-only images require a DSi)",
            ));
        }
        let title = &bytes[..bytes[..12].iter().position(|&b| b == 0).unwrap_or(12)];
        let title = if title.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            std::str::from_utf8(title)
                .unwrap_or_default()
                .trim()
                .to_owned()
        } else {
            String::new()
        };
        let code = bytes[12..16]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .then(|| String::from_utf8_lossy(&bytes[12..16]).into_owned());
        Ok(Self {
            title,
            code,
            arm9: segment(bytes, 0x20, file_len, false)?,
            arm7: segment(bytes, 0x30, file_len, true)?,
        })
    }
    /// Cartridge-provided internal title, possibly empty for homebrew.
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Four-character game identifier when present.
    pub fn game_code(&self) -> Option<&str> {
        self.code.as_deref()
    }
}

fn segment(bytes: &[u8], at: usize, file_len: u64, arm7: bool) -> Result<Segment, Error> {
    let word = |off| {
        let i = at + off;
        u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
    };
    let (offset, entry, load, size) = (word(0usize), word(4), word(8), word(12));
    let source_end = u64::from(offset) + u64::from(size);
    if offset < HEADER_LEN as u32 || size < 2 || source_end > file_len {
        return Err(Error::InvalidRom(
            "DS executable lies outside the cartridge",
        ));
    }
    let end = load
        .checked_add(size)
        .ok_or(Error::InvalidRom("wrapped DS executable address"))?;
    let addr = entry & !1;
    if load & 3 != 0 || entry & 3 == 2 || !(load..end).contains(&addr) {
        return Err(Error::InvalidRom(
            "DS executable entry lies outside its loaded segment",
        ));
    }
    let main =
        (0x0200_0000..0x0300_0000).contains(&load) && end <= 0x0300_0000 && size <= 4 * 1024 * 1024;
    let private = arm7
        && (0x0300_0000..0x0400_0000).contains(&load)
        && end <= 0x0400_0000
        && size <= 96 * 1024;
    if !main && !private {
        return Err(Error::InvalidRom("unsupported DS executable RAM range"));
    }
    Ok(Segment {
        source: offset as usize..source_end as usize,
        load,
        entry,
    })
}

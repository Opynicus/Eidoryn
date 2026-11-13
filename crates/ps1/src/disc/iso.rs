//! Bounded ISO-9660 filesystem and boot metadata, built only on sector reads.
use super::{DiscImage, invalid};
use crate::{EmuError, bus::RAM_SIZE};

impl DiscImage {
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
        let components = path.split(['/', '\\']).filter(|s| !s.is_empty());
        // Reject the whole path before any I/O, even when an earlier component is absent.
        if !(1..=16).contains(&components.clone().count())
            || components.clone().any(|s| matches!(s, "." | ".."))
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

fn boot_path(bytes: &[u8]) -> Result<&str, EmuError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Invalid SYSTEM.CNF text"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_path_accepts_existing_whitespace_quotes_and_case_rules() {
        assert_eq!(
            boot_path(b"TCB = 4\r\n  bOoT = \"CdRoM:\\GAME\\MAIN.EXE;1\"  \r\n").unwrap(),
            "\\GAME\\MAIN.EXE;1"
        );
        // The first BOOT entry wins; an empty path is left to ISO path validation.
        assert_eq!(
            boot_path(b"BOOT=cdrom:\nBOOT=cdrom:SECOND.EXE").unwrap(),
            ""
        );
    }

    #[test]
    fn boot_path_retains_specific_parse_errors() {
        for (bytes, expected) in [
            (&b"\xff"[..], "Invalid SYSTEM.CNF text"),
            (&b"STACK=0"[..], "SYSTEM.CNF has no BOOT entry"),
            (&b"BOOT=MAIN.EXE"[..], "BOOT must use cdrom:"),
            (&b"BOOT=host:MAIN.EXE"[..], "Unsupported BOOT device"),
            (
                &b"BOOT=host:MAIN.EXE\nBOOT=cdrom:MAIN.EXE"[..],
                "Unsupported BOOT device",
            ),
        ] {
            assert_eq!(boot_path(bytes).unwrap_err(), invalid(expected));
        }
    }
}

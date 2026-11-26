//! The cartridge AUX SPI bus, with the two-byte address protocol of 64/512 Kbit EEPROMs.
use serde::{Deserialize, Serialize};

pub(crate) const SMALL: usize = 8 * 1024;
pub(crate) const LARGE: usize = 64 * 1024;

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Backup {
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
    command: Option<u8>,
    address_bytes: u8,
    address: u16,
    // Page data becomes nonvolatile when chip select rises. Preserve a partial
    // transfer in quick states without publishing it to the battery file.
    #[serde(with = "serde_bytes")]
    page: Vec<u8>,
    page_cursor: u8,
    reply: u8,
    status: u8,
    #[serde(skip)]
    revision: u64,
}

impl Backup {
    pub(crate) fn valid(&self) -> bool {
        matches!(self.bytes.len(), 0 | SMALL | LARGE)
            && self.address_bytes <= 2
            && self.page.len() <= self.page_size()
            && usize::from(self.page_cursor) < self.page_size()
            && usize::from(self.page_cursor) <= self.page.len()
    }

    pub(crate) fn data(&self) -> Option<&[u8]> {
        (!self.bytes.is_empty()).then_some(&self.bytes)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn import(&mut self, bytes: &[u8]) -> bool {
        if !matches!(bytes.len(), SMALL | LARGE) {
            return false;
        }
        self.bytes = bytes.to_vec();
        self.revision = self.revision.wrapping_add(1);
        self.reset_bus();
        true
    }

    pub(crate) fn restored(&mut self, previous: &Self) {
        if self.bytes.is_empty() && !previous.bytes.is_empty() {
            // A snapshot can precede the first EEPROM write. Flush erased bytes to
            // the existing battery file as part of restoring that earlier timeline.
            self.bytes.resize(previous.bytes.len(), 0xff);
        }
        self.revision = previous
            .revision
            .wrapping_add(u64::from(self.bytes != previous.bytes));
    }

    pub(crate) fn reset_bus(&mut self) {
        self.command = None;
        self.page.clear();
        self.page_cursor = 0;
        self.address_bytes = 0;
        self.status &= 0x0c;
        self.reply = 0;
    }

    pub(crate) fn deselect(&mut self) {
        if !self.page.is_empty() {
            self.commit_page();
            self.page.clear();
            self.page_cursor = 0;
        }
        if matches!(self.command, Some(0x01 | 0x02)) {
            self.status &= !2;
        }
        self.command = None;
    }

    pub(crate) fn reply(&self) -> u8 {
        self.reply
    }

    pub(crate) fn shift(&mut self, byte: u8, hold: bool) {
        self.reply = 0xff;
        match self.command {
            None => {
                self.command = Some(byte);
                self.address_bytes = 0;
                self.address = 0;
                match byte {
                    0x06 => self.status |= 2,  // WREN
                    0x04 => self.status &= !2, // WRDI
                    _ => {}
                }
            }
            Some(0x05) => self.reply = self.status,
            Some(0x01) if self.status & 2 != 0 => self.status = (self.status & 2) | (byte & 0x0c),
            Some(0x02 | 0x03) if self.address_bytes < 2 => {
                self.address = (self.address << 8) | u16::from(byte);
                self.address_bytes += 1;
            }
            Some(0x03) => {
                self.reply = self
                    .bytes
                    .get(usize::from(self.address))
                    .copied()
                    .unwrap_or(0xff);
                self.address = self.address.wrapping_add(1);
            }
            Some(0x02) if self.status & 2 != 0 => {
                let cursor = usize::from(self.page_cursor);
                if cursor == self.page.len() {
                    self.page.push(byte);
                } else {
                    self.page[cursor] = byte;
                }
                self.page_cursor = ((cursor + 1) & (self.page_size() - 1)) as u8;
            }
            _ => {}
        }
        if !hold {
            self.deselect();
        }
    }

    fn capacity(&self) -> usize {
        // Both chip sizes share a two-byte address. An access above 8 KiB provides
        // positive evidence for the larger capacity without a game-code database.
        if usize::from(self.address) >= SMALL || self.bytes.len() == LARGE {
            LARGE
        } else {
            SMALL
        }
    }

    fn page_size(&self) -> usize {
        if self.capacity() == SMALL { 32 } else { 128 }
    }

    fn commit_page(&mut self) {
        let size = self.capacity();
        let limit = match self.status >> 2 & 3 {
            1 => size * 3 / 4,
            2 => size / 2,
            3 => 0,
            _ => size,
        };
        let first = usize::from(self.address);
        if first >= limit {
            return;
        }
        let mask = self.page_size() - 1;
        let mut changed = self.bytes.len() != size;
        self.bytes.resize(size, 0xff);
        for (i, byte) in self.page.iter().copied().enumerate() {
            let dst = &mut self.bytes[(first & !mask) | ((first + i) & mask)];
            changed |= *dst != byte;
            *dst = byte;
        }
        self.revision = self.revision.wrapping_add(u64::from(changed));
    }
}

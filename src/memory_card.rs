//! Independently implemented 128 KiB PS1 card and byte-serial protocol.
//! Register/protocol reference: https://psx-spx.consoledev.net/controllersandmemorycards/
pub const CARD_SIZE: usize = 128 * 1024;
pub const SECTOR_SIZE: usize = 128;
pub const SECTORS: usize = CARD_SIZE / SECTOR_SIZE;
pub const CARD_ACK_DELAY: u32 = 300;
pub const READ_ACK_DELAY: u32 = CARD_ACK_DELAY + 31_000;
pub type Cards = [Option<MemoryCard>; 2];

#[derive(Clone)]
pub struct MemoryCard {
    bytes: Vec<u8>,
    flag: u8,
    position: usize,
    command: u8,
    address: u16,
    checksum: u8,
    previous: u8,
    buffer: [u8; SECTOR_SIZE],
    result: u8,
    active: bool,
    revision: u64,
    pub reads: u64,
    pub writes: u64,
}

impl MemoryCard {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() != CARD_SIZE {
            return Err(format!(
                "Memory card must be exactly {CARD_SIZE} bytes; got {}",
                bytes.len()
            ));
        }
        Ok(Self {
            bytes,
            flag: 8,
            position: 0,
            command: 0,
            address: 0,
            checksum: 0,
            previous: 0,
            buffer: [0; SECTOR_SIZE],
            result: 0xff,
            active: false,
            revision: 0,
            reads: 0,
            writes: 0,
        })
    }

    pub fn formatted() -> Self {
        let mut bytes = vec![0xff; CARD_SIZE];
        let mut header = [0; SECTOR_SIZE];
        header[..2].copy_from_slice(b"MC");
        header[127] = b'M' ^ b'C';
        bytes[..128].copy_from_slice(&header);
        bytes[63 * 128..64 * 128].copy_from_slice(&header);
        for sector in 1..36 {
            let frame = &mut bytes[sector * 128..(sector + 1) * 128];
            frame.fill(0);
            if sector < 16 {
                frame[0] = 0xa0;
                frame[8..10].fill(0xff);
            } else {
                frame[..4].fill(0xff);
            }
            frame[127] = frame[..127].iter().fold(0, |sum, byte| sum ^ byte);
        }
        Self::from_bytes(bytes).expect("formatted card size")
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Deselecting or resetting SIO aborts only the serial transaction, not storage.
    pub fn deselect(&mut self) {
        self.active = false;
    }

    pub fn select(&mut self) {
        self.position = 0;
        self.command = 0;
        self.address = 0;
        self.checksum = 0;
        self.previous = 0;
        self.active = true;
    }

    /// Returns the received byte and optional delay before the device's ACK edge.
    pub fn exchange(&mut self, byte: u8) -> (u8, Option<u32>) {
        if !self.active {
            return (0xff, None);
        }
        let position = self.position;
        let (reply, more) = match position {
            0 => {
                self.command = byte;
                (self.flag, matches!(byte, 0x52 | 0x53 | 0x57))
            }
            1 => (0x5a, true),
            2 => (0x5d, true),
            _ if self.command == 0x53 => {
                let id = [0x5c, 0x5d, 4, 0, 0, 0x80];
                (id[position - 3], position < 8)
            }
            3 => {
                self.address = u16::from(byte) << 8;
                self.checksum = byte;
                (0, true)
            }
            4 => {
                self.address |= u16::from(byte);
                self.checksum ^= byte;
                (self.previous, true)
            }
            _ if self.command == 0x52 => self.read_byte(position),
            _ => self.write_byte(position, byte),
        };
        self.position += 1;
        self.previous = byte;
        self.active = more;
        let delay = if self.command == 0x52 && position == 5 {
            READ_ACK_DELAY
        } else {
            CARD_ACK_DELAY
        };
        (reply, more.then_some(delay))
    }

    fn read_byte(&mut self, position: usize) -> (u8, bool) {
        let valid = usize::from(self.address) < SECTORS;
        match position {
            5 => (0x5c, true),
            6 => (0x5d, true),
            7 => (
                if valid {
                    (self.address >> 8) as u8
                } else {
                    0xff
                },
                true,
            ),
            8 => (if valid { self.address as u8 } else { 0xff }, valid),
            9..=136 => {
                let byte = self.bytes[usize::from(self.address) * SECTOR_SIZE + position - 9];
                self.checksum ^= byte;
                (byte, true)
            }
            137 => (self.checksum, true),
            _ => {
                self.reads += 1;
                (0x47, false)
            }
        }
    }

    fn write_byte(&mut self, position: usize, byte: u8) -> (u8, bool) {
        match position {
            5..=132 => {
                self.buffer[position - 5] = byte;
                self.checksum ^= byte;
                (self.previous, true)
            }
            133 => {
                self.result = if usize::from(self.address) >= SECTORS {
                    0xff
                } else if byte != self.checksum {
                    0x4e
                } else {
                    0x47
                };
                if self.result == 0x47 {
                    let start = usize::from(self.address) * SECTOR_SIZE;
                    let target = &mut self.bytes[start..start + SECTOR_SIZE];
                    if target != self.buffer {
                        target.copy_from_slice(&self.buffer);
                        self.revision += 1;
                    }
                    self.writes += 1;
                    self.flag = 0;
                } else {
                    self.flag |= 4;
                }
                (self.previous, true)
            }
            134 => (0x5c, true),
            135 => (0x5d, true),
            _ => (self.result, false),
        }
    }
}

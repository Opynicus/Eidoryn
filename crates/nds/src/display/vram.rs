//! ARM9 and ARM7 see different windows into the same nine banks of video memory.
use serde::{Deserialize, Serialize};
use std::cell::OnceCell;

const START: [usize; 10] = [
    0, 0x20000, 0x40000, 0x60000, 0x80000, 0x90000, 0x94000, 0x98000, 0xa0000, 0xa4000,
];
const PAGE: usize = 0x4000;
pub(super) const BG_A: usize = 0;
pub(super) const BG_B: usize = 1;
pub(super) const OBJ_A: usize = 2;
pub(super) const OBJ_B: usize = 3;
pub(super) const ARM7: usize = 4;

#[derive(Default)]
struct TextureSlots {
    image: [u16; 4],
    palette: [u16; 6],
}

#[derive(Serialize, Deserialize)]
pub(super) struct Vram {
    bytes: Vec<u8>,
    control: [u8; 9],
    // A 16 KiB window can address several banks at once. Reads OR their values.
    pages: [[u16; 32]; 5],
    // Derived from VRAMCNT; reconstruct lazily after a bank switch or snapshot load.
    #[serde(skip)]
    texture_slots: OnceCell<TextureSlots>,
}

impl Default for Vram {
    fn default() -> Self {
        Self {
            bytes: vec![0; START[9]],
            control: [0; 9],
            pages: [[0; 32]; 5],
            texture_slots: OnceCell::new(),
        }
    }
}

impl Vram {
    pub(super) fn valid(&self) -> bool {
        self.bytes.len() == START[9] && self.pages == Self::mappings(&self.control)
    }

    pub(super) fn control(&self, bank: usize) -> u8 {
        self.control[bank]
    }

    pub(super) fn set_control(&mut self, bank: usize, value: u8) {
        if self.control[bank] == value {
            return;
        }
        self.control[bank] = value;
        self.pages = Self::mappings(&self.control);
        self.texture_slots.take();
    }

    fn mappings(control: &[u8; 9]) -> [[u16; 32]; 5] {
        let mut pages = [[0_u16; 32]; 5];
        for (bank, &reg) in control.iter().enumerate() {
            if reg & 0x80 == 0 {
                continue;
            }
            let offset = usize::from((reg >> 3) & 3);
            let small = (offset & 1) + (offset & 2) * 2;
            let (target, page) = match (bank, reg & 7) {
                (0..=3, 1) => (BG_A, 8 * offset),
                (0..=1, 2) => (OBJ_A, 8 * (offset & 1)),
                (2..=3, 2) => (ARM7, 8 * (offset & 1)),
                (2, 4) | (7, 1) => (BG_B, 0),
                (3, 4) | (8, 2) => (OBJ_B, 0),
                (4, 1) => (BG_A, 0),
                (4, 2) => (OBJ_A, 0),
                (5..=6, 1) => (BG_A, small),
                (5..=6, 2) => (OBJ_A, small),
                (8, 1) => (BG_B, 2),
                _ => continue,
            };
            let count = (START[bank + 1] - START[bank]) / PAGE;
            for mapping in &mut pages[target][page..page + count] {
                *mapping |= 1 << bank;
            }
        }
        pages
    }

    pub(super) fn read(&self, target: usize, offset: usize) -> u8 {
        self.read_banks(self.pages[target][(offset / PAGE) & 31], offset)
    }

    fn texture_slots(&self) -> &TextureSlots {
        self.texture_slots.get_or_init(|| {
            let mut slots = TextureSlots::default();
            for (bank, &control) in self.control[..7].iter().enumerate() {
                if control & 0x87 != 0x83 {
                    continue;
                }
                let offset = usize::from(control >> 3 & 3);
                match bank {
                    0..=3 => slots.image[offset] |= 1 << bank,
                    4 => slots.palette[..4]
                        .iter_mut()
                        .for_each(|mask| *mask |= 1 << bank),
                    _ => slots.palette[(offset & 1) + (offset & 2) * 2] |= 1 << bank,
                }
            }
            slots
        })
    }

    pub(super) fn texture_byte(&self, addr: usize) -> u8 {
        self.read_banks(self.texture_slots().image[(addr >> 17) & 3], addr)
    }

    pub(super) fn texture_palette(&self, addr: usize) -> u16 {
        let slot = (addr >> 14) & 7;
        let Some(&banks) = self.texture_slots().palette.get(slot) else {
            return 0;
        };
        u16::from_le_bytes([
            self.read_banks(banks, addr),
            self.read_banks(banks, addr + 1),
        ]) & 0x7fff
    }

    fn read_banks(&self, mut mask: u16, offset: usize) -> u8 {
        let mut value = 0;
        while mask != 0 {
            let bank = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            value |= self.bytes[Self::physical(bank, offset)];
        }
        value
    }

    pub(super) fn write(&mut self, target: usize, offset: usize, value: u8) {
        let mut mask = self.pages[target][(offset / PAGE) & 31];
        while mask != 0 {
            let bank = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            self.bytes[Self::physical(bank, offset)] = value;
        }
    }

    fn physical(bank: usize, offset: usize) -> usize {
        START[bank] + (offset & (START[bank + 1] - START[bank] - 1))
    }

    fn lcdc_bank(&self, offset: usize) -> Option<usize> {
        if offset >= START[9] {
            return None;
        }
        let bank = START.partition_point(|&start| start <= offset) - 1;
        (self.control[bank] & 0x87 == 0x80).then_some(bank)
    }

    pub(super) fn lcdc_read(&self, offset: usize) -> u8 {
        self.lcdc_bank(offset).map_or(0, |_| self.bytes[offset])
    }

    pub(super) fn lcdc_write(&mut self, offset: usize, value: u8) {
        if self.lcdc_bank(offset).is_some() {
            self.bytes[offset] = value;
        }
    }
}

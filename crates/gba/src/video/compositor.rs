//! Window masking and color effects are applied to visible layers, before RGB expansion.
use super::{WIDTH, half};

pub(super) const OBJ: u8 = 1 << 4;
const BACKDROP: u8 = 1 << 5;
const EFFECTS: u8 = 1 << 5;

#[derive(Clone, Copy)]
pub(super) struct Pixel {
    pub color: u16,
    pub rank: u8,
    pub layer: u8,
    pub semi: bool,
}

impl Pixel {
    pub const EMPTY: Self = Self {
        color: 0,
        rank: u8::MAX,
        layer: 0,
        semi: false,
    };
}

#[derive(Clone, Copy)]
pub(super) struct Stack {
    top: Pixel,
    next: Pixel,
}

impl Stack {
    pub fn backdrop(color: u16) -> Self {
        Self {
            top: Pixel {
                color,
                rank: 32,
                layer: BACKDROP,
                semi: false,
            },
            next: Pixel::EMPTY,
        }
    }

    pub fn insert(&mut self, pixel: Pixel) {
        if pixel.rank < self.top.rank {
            self.next = self.top;
            self.top = pixel;
        } else if pixel.rank < self.next.rank {
            self.next = pixel;
        }
    }

    pub fn color(self, effects: &Effects, window: u8) -> u16 {
        let top = self.top;
        let selected = effects.targets & u16::from(top.layer) != 0 && window & EFFECTS != 0;
        let second = (effects.targets >> 8) & u16::from(self.next.layer) != 0;
        // Semi-transparent objects force alpha over an eligible immediate neighbor.
        if second && (top.semi || (selected && effects.mode == 1)) {
            return channels(top.color, self.next.color, |a, b| {
                ((a * effects.eva + b * effects.evb) >> 4).min(31)
            });
        }
        if selected {
            match effects.mode {
                2 => return channels(top.color, 0, |a, _| a + (((31 - a) * effects.evy) >> 4)),
                3 => return channels(top.color, 0, |a, _| a - ((a * effects.evy) >> 4)),
                _ => {}
            }
        }
        top.color
    }
}

fn channels(a: u16, b: u16, mix: impl Fn(u16, u16) -> u16) -> u16 {
    let channel = |shift: u32| mix((a >> shift) & 31, (b >> shift) & 31) << shift;
    channel(0) | channel(5) | channel(10)
}

pub(super) struct Effects {
    targets: u16,
    mode: u16,
    eva: u16,
    evb: u16,
    evy: u16,
}

impl Effects {
    pub fn read(io: &[u8]) -> Self {
        let targets = half(io, 0x50);
        let coefficient = |offset: usize| u16::from((io[offset] & 31).min(16));
        Self {
            targets,
            mode: (targets >> 6) & 3,
            eva: coefficient(0x52),
            evb: coefficient(0x53),
            evy: coefficient(0x54),
        }
    }
}

fn contains(range: u16, coordinate: u8) -> bool {
    // Both bounds are 8-bit LCD counter values. A wrapped interval spans the counter rollover.
    let [end, start] = range.to_le_bytes();
    coordinate.wrapping_sub(start) < end.wrapping_sub(start)
}

pub(super) fn window_masks(
    control: u16,
    y: usize,
    io: &[u8],
    obj_window: &[bool; WIDTH],
) -> [u8; WIDTH] {
    if control & 0xe000 == 0 {
        return [0x3f; WIDTH];
    }
    let mut masks = std::array::from_fn(|i| if obj_window[i] { io[0x4b] } else { io[0x4a] });
    for window in [1, 0] {
        if control & (0x2000 << window) == 0 || !contains(half(io, 0x44 + window * 2), y as u8) {
            continue;
        }
        let horizontal = half(io, 0x40 + window * 2);
        for (x, mask) in masks.iter_mut().enumerate() {
            if contains(horizontal, x as u8) {
                *mask = io[0x48 + window];
            }
        }
    }
    masks
}

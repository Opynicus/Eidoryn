use super::{Display, H, REG_SIZE, W, half, rgb, word};

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Affine,
    Extended,
    Large,
}

impl Kind {
    fn for_layer(mode: u32, layer: usize) -> Option<Self> {
        match (mode, layer) {
            (6, 2) => Some(Self::Large),
            (6.., _) => None,
            (1 | 2, 3) | (2 | 4, 2) => Some(Self::Affine),
            (3..=5, 3) | (5, 2) => Some(Self::Extended),
            _ => Some(Self::Text),
        }
    }
}

struct Background<'a> {
    screen: &'a Display,
    engine: usize,
    control: u16,
    map: usize,
    chars: usize,
}

impl Background<'_> {
    fn byte(&self, address: usize) -> u8 {
        self.screen.vram.read(self.engine, address)
    }
    fn half(&self, address: usize) -> u16 {
        u16::from_le_bytes([self.byte(address), self.byte(address + 1)])
    }
    fn color(&self, index: u8, bank: usize) -> Option<u16> {
        (index != 0).then(|| {
            half(
                &self.screen.palette,
                self.engine * 1024 + (bank + usize::from(index)) * 2,
            )
        })
    }

    fn tile(&self, entry: u16, x: usize, y: usize, full: bool) -> Option<u16> {
        let x = if entry & 0x400 != 0 {
            7 - (x & 7)
        } else {
            x & 7
        };
        let y = if entry & 0x800 != 0 {
            7 - (y & 7)
        } else {
            y & 7
        };
        let tile = usize::from(entry & 1023);
        if full {
            self.color(self.byte(self.chars + tile * 64 + y * 8 + x), 0)
        } else {
            let b = self.byte(self.chars + tile * 32 + y * 4 + x / 2);
            let index = (b >> ((x & 1) * 4)) & 15;
            self.color(index, usize::from(entry >> 12) * 16)
        }
    }

    fn text(&self, x: usize, y: usize) -> Option<u16> {
        let size = self.control >> 14;
        let blocks_x = 1 + usize::from(size & 1);
        let block = (x / 256 % blocks_x) + blocks_x * (y / 256 % (1 + usize::from(size >> 1)));
        let entry = self.half(self.map + 2048 * block + 2 * (((y / 8) & 31) * 32 + ((x / 8) & 31)));
        self.tile(entry, x, y, self.control & 128 != 0)
    }

    fn transformed(&self, kind: Kind, x: i32, y: i32) -> Option<u16> {
        let size = usize::from(self.control >> 14);
        let bitmap = matches!(kind, Kind::Extended) && self.control & 0x80 != 0;
        let (width, height) = if bitmap {
            [(128, 128), (256, 256), (512, 256), (512, 512)][size]
        } else if matches!(kind, Kind::Large) {
            [(512, 1024), (1024, 512), (512, 1024), (1024, 512)][size]
        } else {
            let dim = 128 << size;
            (dim, dim)
        };
        if self.control & 0x2000 == 0 && !(x >= 0 && y >= 0 && x < width && y < height) {
            return None;
        }
        let x = (x & (width - 1)) as usize;
        let y = (y & (height - 1)) as usize;
        let pos = y * width as usize + x;
        if matches!(kind, Kind::Large) {
            return self.color(self.byte(pos), 0);
        }
        if bitmap {
            let base = usize::from((self.control >> 8) & 31) * 0x4000;
            if self.control & 4 == 0 {
                return self.color(self.byte(base + pos), 0);
            }
            let pixel = self.half(base + 2 * pos);
            return (pixel & 0x8000 != 0).then_some(pixel);
        }
        let tile = (y / 8) * (width as usize / 8) + x / 8;
        let entry = if matches!(kind, Kind::Affine) {
            u16::from(self.byte(self.map + tile))
        } else {
            self.half(self.map + 2 * tile)
        };
        self.tile(entry, x, y, true)
    }
}

impl Display {
    pub(super) fn draw_bg(
        &self,
        engine: usize,
        layer: usize,
        cnt: u16,
        control: u32,
        out: &mut [u8],
    ) {
        let Some(kind) = Kind::for_layer(control & 7, layer) else {
            return;
        };
        if layer == 0 && engine == 0 && control & 8 != 0 {
            return;
        }
        let regs = &self.regs[engine * REG_SIZE..(engine + 1) * REG_SIZE];
        let global = if engine == 0 { control as usize } else { 0 };
        let bg = Background {
            screen: self,
            engine,
            control: cnt,
            map: ((global >> 27) & 7) * 0x10000 + usize::from((cnt >> 8) & 31) * 0x800,
            chars: ((global >> 24) & 7) * 0x10000 + usize::from((cnt >> 2) & 15) * 0x4000,
        };
        let offset = 0x10 + 4 * layer;
        let sx = usize::from(half(regs, offset) & 511);
        let sy = usize::from(half(regs, offset + 2) & 511);
        let affine = if layer == 3 { 0x30 } else { 0x20 };
        let [pa, pb, pc, pd] = [0, 2, 4, 6].map(|i| i32::from(half(regs, affine + i) as i16));
        let [rx, ry] = [8, 12].map(|i| ((word(regs, affine + i) << 4) as i32) >> 4);
        for y in 0..H {
            let mut ax = rx + pb * y as i32;
            let mut ay = ry + pd * y as i32;
            for x in 0..W {
                let color = match kind {
                    Kind::Text => bg.text(sx + x, sy + y),
                    _ => bg.transformed(kind, ax >> 8, ay >> 8),
                };
                if let Some(color) = color {
                    let pos = (y * W + x) * 3;
                    out[pos..pos + 3].copy_from_slice(&rgb(color));
                }
                ax += pa;
                ay += pc;
            }
        }
    }
}

use super::{Display, H, W, half, rgb};

impl Display {
    // Normal 16- and 256-color tiled objects, including flip and priority.
    pub(super) fn draw_sprites(&self, engine: usize, control: u32, priority: u16, out: &mut [u8]) {
        const SIZES: [[(usize, usize); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        let oam = &self.oam[engine * 1024..];
        for obj in (0..128).rev() {
            let [attr0, attr1, attr2] = [0, 2, 4].map(|i| half(oam, obj * 8 + i));
            if attr0 & 0x300 != 0 || attr0 & 0x0c00 >= 0x0800 || ((attr2 >> 10) & 3) != priority {
                continue;
            }
            let Some(sizes) = SIZES.get(usize::from(attr0 >> 14)) else {
                continue;
            };
            let (width, height) = sizes[usize::from(attr1 >> 14)];
            let full = attr0 & 0x2000 != 0;
            let tile_bytes = if full { 64 } else { 32 };
            let linear = control & (1 << 4) != 0;
            let boundary = if linear {
                32 << ((control >> 20) & 3)
            } else {
                32
            };
            let base = usize::from(attr2 & 1023) * boundary;
            let row_size = if linear { width / 8 * tile_bytes } else { 1024 };
            for dy in 0..height {
                let y = (usize::from(attr0 & 255) + dy) & 255;
                if y >= H {
                    continue;
                }
                let ty = if attr1 & 0x2000 != 0 {
                    height - 1 - dy
                } else {
                    dy
                };
                for dx in 0..width {
                    let x = (usize::from(attr1 & 511) + dx) & 511;
                    if x >= W {
                        continue;
                    }
                    let tx = if attr1 & 0x1000 != 0 {
                        width - 1 - dx
                    } else {
                        dx
                    };
                    let tile = base + ty / 8 * row_size + tx / 8 * tile_bytes;
                    let pixel_offset = (ty & 7) * 8 + (tx & 7);
                    let (index, bank) = if full {
                        (self.vram.read(engine + 2, tile + pixel_offset), 0)
                    } else {
                        let packed = self.vram.read(engine + 2, tile + pixel_offset / 2);
                        (
                            (packed >> (pixel_offset % 2 * 4)) & 15,
                            usize::from(attr2 >> 12) * 16,
                        )
                    };
                    if index != 0 {
                        let color = half(
                            &self.palette,
                            engine * 1024 + 512 + (usize::from(index) + bank) * 2,
                        );
                        let at = (y * W + x) * 3;
                        out[at..at + 3].copy_from_slice(&rgb(color));
                    }
                }
            }
        }
    }
}

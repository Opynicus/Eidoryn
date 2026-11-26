//! Texture image and palette decoding for the geometry engine.
use super::vram::Vram;

fn coordinate(coord: i32, size: i32, repeat: bool, mirror: bool) -> usize {
    if !repeat {
        return coord.clamp(0, size - 1) as usize;
    }
    let within = coord & (size - 1);
    if mirror && coord & size != 0 {
        (size - 1 - within) as usize
    } else {
        within as usize
    }
}

fn mix_colors(first: u16, second: u16, first_weight: u16) -> u16 {
    let mut result = 0;
    for shift in [0, 5, 10] {
        let a = first >> shift & 31;
        let b = second >> shift & 31;
        result |= ((a * first_weight + b * (8 - first_weight)) >> 3) << shift;
    }
    result
}

impl Vram {
    /// Sample `TEXIMAGE_PARAM` and `PLTT_BASE` at integer texel coordinates.
    pub(super) fn sample_texture(
        &self,
        params: u32,
        palette_base: u32,
        s: i32,
        t: i32,
    ) -> Option<(u16, u8)> {
        let format = (params >> 26) & 7;
        if format == 0 {
            return Some((0x7fff, 31));
        }
        let width = 8 << ((params >> 20) & 7);
        let height = 8 << ((params >> 23) & 7);
        let x = coordinate(s, width, params & (1 << 16) != 0, params & (1 << 18) != 0);
        let y = coordinate(t, height, params & (1 << 17) != 0, params & (1 << 19) != 0);
        let pixel = y * width as usize + x;
        let base = (params as usize & 0xffff) * 8;
        let palette = (palette_base as usize & 0x1fff) << if format == 2 { 3 } else { 4 };
        let (index, alpha) = match format {
            1 => {
                let packed = self.texture_byte(base + pixel);
                let alpha = packed >> 5;
                (packed & 31, (alpha << 2) | (alpha >> 1))
            }
            2 => (
                (self.texture_byte(base + pixel / 4) >> ((pixel & 3) * 2)) & 3,
                31,
            ),
            3 => (
                (self.texture_byte(base + pixel / 2) >> ((pixel & 1) * 4)) & 15,
                31,
            ),
            4 => (self.texture_byte(base + pixel), 31),
            5 => {
                let block = base + 4 * ((y / 4) * (width as usize / 4) + x / 4);
                return self
                    .compressed_texel(block, palette, x & 3, y & 3)
                    .map(|color| (color, 31));
            }
            6 => {
                let packed = self.texture_byte(base + pixel);
                (packed & 7, packed >> 3)
            }
            _ => {
                let addr = base + pixel * 2;
                let color =
                    u16::from_le_bytes([self.texture_byte(addr), self.texture_byte(addr + 1)]);
                return (color & 0x8000 != 0).then_some((color & 0x7fff, 31));
            }
        };
        let transparent_zero = (2..=4).contains(&format) && params & (1 << 29) != 0;
        (alpha != 0 && !(index == 0 && transparent_zero)).then(|| {
            (
                self.texture_palette(palette + 2 * usize::from(index)),
                alpha,
            )
        })
    }

    fn compressed_texel(&self, block: usize, palette: usize, x: usize, y: usize) -> Option<u16> {
        let index = (self.texture_byte(block + y) >> (2 * x)) & 3;
        // Slot 1 holds a descriptor for every 4-byte block in slots 0 and 2.
        let meta_addr = 0x20000 + ((block >> 2) & 0x10000) + ((block & 0x1ffff) >> 1);
        let meta = u16::from_le_bytes([
            self.texture_byte(meta_addr),
            self.texture_byte(meta_addr + 1),
        ]);
        let colors = palette + usize::from(meta & 0x3fff) * 4;
        match (meta >> 14, index) {
            (0 | 1, 3) => None,
            (1, 2) | (3, 2 | 3) => {
                let weight = if meta >> 14 == 1 {
                    4
                } else if index == 2 {
                    5
                } else {
                    3
                };
                Some(mix_colors(
                    self.texture_palette(colors),
                    self.texture_palette(colors + 2),
                    weight,
                ))
            }
            _ => Some(self.texture_palette(colors + 2 * usize::from(index))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture_vram(image: &[u8], palette: &[u16]) -> Vram {
        let mut vram = Vram::default();
        vram.set_control(0, 0x80); // Stage texture in A and palette in F through LCDC.
        vram.set_control(5, 0x80);
        for (i, &byte) in image.iter().enumerate() {
            vram.lcdc_write(i, byte);
        }
        for (i, byte) in palette.iter().flat_map(|v| v.to_le_bytes()).enumerate() {
            vram.lcdc_write(0x90000 + i, byte);
        }
        vram.set_control(0, 0x83);
        vram.set_control(5, 0x83);
        vram
    }

    #[test]
    fn banked_palettes_use_per_format_stride_and_color_zero_rule() {
        let palette = [0x19; 32];
        let mut vram = texture_vram(&[0xe4, 0xa1], &palette);
        vram.set_control(5, 0x80);
        for (i, c) in [0x11u16, 0x22, 0x33, 0x44].into_iter().enumerate() {
            for (j, b) in c.to_le_bytes().into_iter().enumerate() {
                vram.lcdc_write(0x90008 + 2 * i + j, b);
            }
        }
        vram.set_control(5, 0x83);
        let params = 2 << 26 | 1 << 29;
        assert_eq!(vram.sample_texture(params, 1, 0, 0), None);
        for (s, c) in [(1, 0x22), (2, 0x33), (3, 0x44)] {
            assert_eq!(vram.sample_texture(params, 1, s, 0), Some((c, 31)));
        }
        assert_eq!(vram.sample_texture(3 << 26, 0, 2, 0), Some((0x19, 31)));
        assert_eq!(vram.sample_texture(1 << 26, 0, 1, 0), Some((0x19, 22)));
        assert_eq!(vram.sample_texture(6 << 26, 0, 1, 0), Some((0x19, 20)));
        vram.set_control(5, 0x93); // Palette slot 4
        assert_eq!(vram.sample_texture(params, 1, 2, 0), Some((0, 31)));
        assert_eq!(vram.sample_texture(4 << 26, 0x1000, 2, 0), Some((0x19, 31)));
    }

    #[test]
    fn repeat_mirror_clamp_and_direct_alpha() {
        let pixels: Vec<u8> = (0..8)
            .flat_map(|x| (if x == 0 { 0u16 } else { 0x8000 | x }).to_le_bytes())
            .collect();
        let vram = texture_vram(&pixels, &[]);
        let p = 7 << 26;
        assert_eq!(vram.sample_texture(p, 0, -1, 0), None);
        assert_eq!(vram.sample_texture(p, 0, 20, 0), Some((7, 31)));
        assert_eq!(vram.sample_texture(p | 1 << 16, 0, -2, 0), Some((6, 31)));
        assert_eq!(
            vram.sample_texture(p | 1 << 16 | 1 << 18, 0, -2, 0),
            Some((1, 31))
        );
    }

    #[test]
    fn compressed_block_in_slot_two_reads_its_own_descriptor_in_slot_one() {
        let mut vram = texture_vram(&[0xe4], &[31, 31 << 10]);
        vram.set_control(0, 0x93); // Texture data slot 2
        vram.set_control(1, 0x80);
        vram.lcdc_write(0x30001, 0xc0); // Slot 1, upper half, interpolation mode 3
        vram.set_control(1, 0x8b);
        let p = 5 << 26 | 0x8000;
        for (s, color) in [
            (0, 31),
            (1, 31 << 10),
            (2, 0x13 | 0x0b << 10),
            (3, 0x0b | 0x13 << 10),
        ] {
            assert_eq!(vram.sample_texture(p, 0, s, 0), Some((color, 31)));
        }
    }
}

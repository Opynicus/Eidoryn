//! Ordered, original software rasterization. Used by both execution backends.
use super::{
    DisplaySettings, DrawStyle, Texture, Transfer, VRAM_HEIGHT, VRAM_WIDTH, Vertex, color_channels,
    color15, sign11,
};

const DITHER: [[i64; 4]; 4] = [
    [-4, 0, -3, 1],
    [2, -2, 3, -1],
    [-3, 1, -4, 0],
    [3, -1, 2, -2],
];

#[derive(Clone, Copy, Default)]
pub(super) struct DrawState {
    pub draw_min: (i32, i32),
    pub draw_max: (i32, i32),
    pub offset: (i32, i32),
    pub mask_set: bool,
    pub mask_check: bool,
    pub draw_mode: u32,
    pub texture_window: u32,
}

#[derive(Clone, Copy)]
pub(super) enum RasterCommand {
    Packet([u32; 12], DrawState),
    Line([Vertex; 2], DrawStyle, DrawState),
    Pixel(usize, u16, DrawState),
}

#[derive(Clone)]
pub(super) struct Rasterizer {
    pub vram: Vec<u16>,
    state: DrawState,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self {
            vram: vec![0; VRAM_WIDTH * VRAM_HEIGHT],
            state: DrawState::default(),
        }
    }
}

impl Rasterizer {
    #[cfg(feature = "vulkan")]
    pub(super) fn from_vram(vram: Vec<u16>) -> Self {
        Self {
            vram,
            state: DrawState::default(),
        }
    }
    pub(super) fn empty() -> Self {
        Self {
            vram: Vec::new(),
            state: DrawState::default(),
        }
    }
    pub(super) fn execute(&mut self, command: RasterCommand) {
        match command {
            RasterCommand::Packet(packet, state) => {
                self.state = state;
                self.packet(&packet);
            }
            RasterCommand::Line(vertices, style, state) => {
                self.state = state;
                self.line(vertices, style);
            }
            RasterCommand::Pixel(index, color, state) => {
                self.state = state;
                self.write_pixel(index, color);
            }
        }
    }
    pub(super) fn display_rgb(&self, display: DisplaySettings) -> Vec<u8> {
        let (width, height) = display.dimensions;
        let mut result = vec![0; width * height * 3];
        if display.disabled {
            return result;
        }
        for (y, output) in result.chunks_exact_mut(width * 3).enumerate() {
            let row = ((display.start.1 + y) & 511) * VRAM_WIDTH;
            let source = &self.vram[row..row + VRAM_WIDTH];
            if display.rgb24 {
                // RGB24 is a byte stream. Copy pairs from each VRAM halfword,
                // including horizontal wrap, instead of dividing for each channel.
                for (pair, pixel) in output.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let color = source[(display.start.0 + pair) & 1023];
                    pixel.copy_from_slice(&color.to_le_bytes());
                }
            } else {
                for (x, pixel) in output.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let color = source[(display.start.0 + x) & 1023];
                    let r = (color & 31) as u8;
                    let g = ((color >> 5) & 31) as u8;
                    let b = ((color >> 10) & 31) as u8;
                    pixel.copy_from_slice(&[
                        (r << 3) | (r >> 2),
                        (g << 3) | (g >> 2),
                        (b << 3) | (b >> 2),
                    ]);
                }
            }
        }
        result
    }

    #[allow(clippy::too_many_lines)] // Keep the small GP0 command dispatcher together.
    fn packet(&mut self, p: &[u32; 12]) {
        match p[0] >> 24 {
            0x02 => {
                let x = (p[1] & 0x3f0) as usize;
                let y = ((p[1] >> 16) & 511) as usize;
                let width = (((p[2] & 1023) + 15) & !15) as usize;
                let height = ((p[2] >> 16) & 511) as usize;
                for row in 0..height {
                    for col in 0..width {
                        self.vram[((y + row) & 511) * VRAM_WIDTH + ((x + col) & 1023)] =
                            color15(p[0]);
                    }
                }
            }
            0x20..=0x3f => {
                let shaded = p[0] & (1 << 28) != 0;
                let quad = p[0] & (1 << 27) != 0;
                let textured = p[0] & (1 << 26) != 0;
                let stride = 1 + usize::from(shaded) + usize::from(textured);
                let mut vertices = [Vertex::default(); 4];
                for (n, vertex) in vertices
                    .iter_mut()
                    .enumerate()
                    .take(if quad { 4 } else { 3 })
                {
                    let position_index = 1 + n * stride;
                    *vertex = Vertex {
                        position: self.position(p[position_index]),
                        color: color_channels(p[if shaded { position_index - 1 } else { 0 }]),
                        uv: if textured {
                            [
                                i64::from(p[position_index + 1] & 255),
                                i64::from((p[position_index + 1] >> 8) & 255),
                            ]
                        } else {
                            [0; 2]
                        },
                    };
                }
                if textured {
                    self.state.draw_mode =
                        (self.state.draw_mode & !0x1ff) | ((p[2 + stride] >> 16) & 0x1ff);
                }
                let style = DrawStyle {
                    texture: textured.then_some(Texture {
                        clut: (p[2] >> 16) as u16,
                        raw: p[0] & (1 << 24) != 0,
                    }),
                    transparent: p[0] & (1 << 25) != 0,
                    dither: self.state.draw_mode & 0x200 != 0 && (shaded || textured),
                };
                self.triangle([vertices[0], vertices[1], vertices[2]], style);
                if quad {
                    self.triangle([vertices[1], vertices[2], vertices[3]], style);
                }
            }
            0x40..=0x5f => {
                let shaded = p[0] & (1 << 28) != 0;
                let first = Vertex {
                    position: self.position(p[1]),
                    color: color_channels(p[0]),
                    uv: [0; 2],
                };
                let last = Vertex {
                    position: self.position(p[2 + usize::from(shaded)]),
                    color: color_channels(p[if shaded { 2 } else { 0 }]),
                    uv: [0; 2],
                };
                let style = DrawStyle {
                    texture: None,
                    transparent: p[0] & (1 << 25) != 0,
                    // Unlike flat polygons, even flat lines use dithering.
                    dither: self.state.draw_mode & 0x200 != 0,
                };
                self.line([first, last], style);
            }
            0x60..=0x7f => {
                let (x, y) = self.position(p[1]);
                let textured = p[0] & (1 << 26) != 0;
                let size = p[2 + usize::from(textured)];
                let (width, height) = match (p[0] >> 27) & 3 {
                    1 => (1, 1),
                    2 => (8, 8),
                    3 => (16, 16),
                    _ => ((size & 1023) as i32, ((size >> 16) & 511) as i32),
                };
                let style = DrawStyle {
                    texture: textured.then_some(Texture {
                        clut: (p[2] >> 16) as u16,
                        raw: p[0] & (1 << 24) != 0,
                    }),
                    transparent: p[0] & (1 << 25) != 0,
                    dither: false,
                };
                let (u_step, v_step) = (
                    if self.state.draw_mode & 0x1000 != 0 {
                        -1
                    } else {
                        1
                    },
                    if self.state.draw_mode & 0x2000 != 0 {
                        -1
                    } else {
                        1
                    },
                );
                for row in y.max(self.state.draw_min.1)..(y + height).min(self.state.draw_max.1 + 1)
                {
                    for col in
                        x.max(self.state.draw_min.0)..(x + width).min(self.state.draw_max.0 + 1)
                    {
                        let uv = [
                            i64::from(p[2] & 255) + i64::from((col - x) * u_step),
                            i64::from((p[2] >> 8) & 255) + i64::from((row - y) * v_step),
                        ];
                        self.shade_pixel(
                            row as usize * VRAM_WIDTH + col as usize,
                            color_channels(p[0]),
                            uv,
                            style,
                        );
                    }
                }
            }
            0x80 => {
                let mut source = Transfer::new(p[1], p[3]);
                let mut destination = Transfer::new(p[2], p[3]);
                // Sequential copy intentionally allows overlapping VRAM regions.
                while let (Some(src), Some(dst)) = (source.next(), destination.next()) {
                    self.write_pixel(dst, self.vram[src]);
                }
            }
            _ => {}
        }
    }

    fn position(&self, word: u32) -> (i32, i32) {
        (
            sign11(word) + self.state.offset.0,
            sign11(word >> 16) + self.state.offset.1,
        )
    }

    fn write_pixel(&mut self, index: usize, color: u16) {
        if !self.state.mask_check || self.vram[index] & 0x8000 == 0 {
            self.vram[index] = color | (u16::from(self.state.mask_set) << 15);
        }
    }

    fn sample_texture(&self, uv: [i64; 2], clut: u16) -> u16 {
        let mask_x = (self.state.texture_window & 31) * 8;
        let mask_y = ((self.state.texture_window >> 5) & 31) * 8;
        let u =
            ((uv[0] as u32 & 255) & !mask_x) | (((self.state.texture_window >> 10) * 8) & mask_x);
        let v =
            ((uv[1] as u32 & 255) & !mask_y) | (((self.state.texture_window >> 15) * 8) & mask_y);
        let page_x = (self.state.draw_mode & 15) * 64;
        let page_y = ((self.state.draw_mode >> 4) & 1) * 256;
        let depth = (self.state.draw_mode >> 7) & 3;
        let packed = self.vram[(((page_y + v) & 511) * 1024
            + ((page_x + (u >> (2 - depth.min(2)))) & 1023))
            as usize];
        let index = match depth {
            0 => (packed >> ((u & 3) * 4)) & 15,
            1 => (packed >> ((u & 1) * 8)) & 255,
            _ => return packed,
        };
        self.vram[(((usize::from(clut) >> 6) & 511) * 1024)
            + ((usize::from(clut & 63) * 16 + usize::from(index)) & 1023)]
    }

    fn shade_pixel(&mut self, index: usize, mut color: [i64; 3], uv: [i64; 2], style: DrawStyle) {
        let mut mask = 0;
        let mut transparent = style.transparent;
        let raw = if let Some(texture) = style.texture {
            let texel = self.sample_texture(uv, texture.clut);
            if texel == 0 {
                return;
            }
            mask = texel & 0x8000;
            transparent &= mask != 0;
            for (n, component) in color.iter_mut().enumerate() {
                let value = i64::from((texel >> (n * 5)) & 31);
                *component = if texture.raw {
                    value * 8
                } else {
                    value * *component / 16
                };
            }
            texture.raw
        } else {
            false
        };
        let dither = if style.dither && !raw {
            DITHER[(index / 1024) & 3][index & 3]
        } else {
            0
        };
        let mut result = mask;
        for (n, component) in color.iter().enumerate() {
            let foreground = ((*component + dither).clamp(0, 255) >> 3) as u16;
            let background = (self.vram[index] >> (n * 5)) & 31;
            let value = if transparent {
                match (self.state.draw_mode >> 5) & 3 {
                    0 => background / 2 + foreground / 2,
                    1 => (background + foreground).min(31),
                    2 => background.saturating_sub(foreground),
                    _ => (background + foreground / 4).min(31),
                }
            } else {
                foreground
            };
            result |= value << (n * 5);
        }
        self.write_pixel(index, result);
    }

    /// Inclusive endpoints, signed coordinates, draw-area clipping and shared
    /// color/mask/blend rules. DDA subpixel rounding is approximate, like triangles.
    fn line(&mut self, [first, last]: [Vertex; 2], style: DrawStyle) {
        let (dx, dy) = (
            last.position.0 - first.position.0,
            last.position.1 - first.position.1,
        );
        if dx.abs() >= 1024 || dy.abs() >= 512 {
            return;
        }
        let length = dx.abs().max(dy.abs());
        let divisor = length.max(1);
        for step in 0..=length {
            let x = (first.position.0 * divisor + dx * step + divisor / 2).div_euclid(divisor);
            let y = (first.position.1 * divisor + dy * step + divisor / 2).div_euclid(divisor);
            if x < self.state.draw_min.0
                || x > self.state.draw_max.0
                || y < self.state.draw_min.1
                || y > self.state.draw_max.1
            {
                continue;
            }
            let color = std::array::from_fn(|n| {
                (first.color[n] * i64::from(divisor - step) + last.color[n] * i64::from(step))
                    / i64::from(divisor)
            });
            self.shade_pixel(y as usize * VRAM_WIDTH + x as usize, color, [0; 2], style);
        }
    }

    fn triangle(&mut self, mut vertices: [Vertex; 3], style: DrawStyle) {
        let area = edge(
            vertices[0].position,
            vertices[1].position,
            vertices[2].position,
        );
        if area == 0 {
            return;
        }
        if area < 0 {
            vertices.swap(1, 2);
        }
        let [a, b, c] = vertices.map(|v| v.position);
        // Reject oversized primitives like the hardware, and bound raster work to VRAM.
        if a.0.max(b.0).max(c.0) - a.0.min(b.0).min(c.0) >= 1024
            || a.1.max(b.1).max(c.1) - a.1.min(b.1).min(c.1) >= 512
        {
            return;
        }
        let min_x = a.0.min(b.0).min(c.0).max(self.state.draw_min.0);
        let max_x = a.0.max(b.0).max(c.0).min(self.state.draw_max.0);
        let min_y = a.1.min(b.1).min(c.1).max(self.state.draw_min.1);
        let max_y = a.1.max(b.1).max(c.1).min(self.state.draw_max.1);
        // Pixel-center affine interpolation with a top-left rule; hardware subpixel precision is approximate.
        let (a2, b2, c2) = ((a.0 * 2, a.1 * 2), (b.0 * 2, b.1 * 2), (c.0 * 2, c.1 * 2));
        let total = area.abs() * 4;
        let pairs = [(b2, c2), (c2, a2), (a2, b2)];
        let inclusive = [(b, c), (c, a), (a, b)].map(|(p, q)| top_left(p, q));
        let step_x = pairs.map(|(p, q)| -2 * i64::from(q.1 - p.1));
        let step_y = pairs.map(|(p, q)| 2 * i64::from(q.0 - p.0));
        let mut row_weights = pairs.map(|(p, q)| edge(p, q, (min_x * 2 + 1, min_y * 2 + 1)));
        // Flat colors and raw textures do not need three divisions per covered pixel.
        let constant_color = (style.texture.is_some_and(|t| t.raw)
            || vertices.iter().all(|v| v.color == vertices[0].color))
        .then_some(vertices[0].color);
        for y in min_y..=max_y {
            let mut next_weights = row_weights;
            for x in min_x..=max_x {
                let weights = next_weights;
                for n in 0..3 {
                    next_weights[n] += step_x[n];
                }
                if !weights
                    .into_iter()
                    .zip(inclusive)
                    .all(|(w, include)| w > 0 || (w == 0 && include))
                {
                    continue;
                }
                let interpolate = |values: [i64; 3]| {
                    values.iter().zip(weights).map(|(v, w)| v * w).sum::<i64>() / total
                };
                let color = constant_color.unwrap_or_else(|| {
                    std::array::from_fn(|n| interpolate(vertices.map(|v| v.color[n])))
                });
                let uv = if style.texture.is_some() {
                    std::array::from_fn(|n| interpolate(vertices.map(|v| v.uv[n])))
                } else {
                    [0; 2]
                };
                self.shade_pixel(y as usize * VRAM_WIDTH + x as usize, color, uv, style);
            }
            for n in 0..3 {
                row_weights[n] += step_y[n];
            }
        }
    }

    #[cfg(test)]
    fn triangle_reference(&mut self, mut vertices: [Vertex; 3], style: DrawStyle) {
        let area = edge(
            vertices[0].position,
            vertices[1].position,
            vertices[2].position,
        );
        if area == 0 {
            return;
        }
        if area < 0 {
            vertices.swap(1, 2);
        }
        let [a, b, c] = vertices.map(|v| v.position);
        // Reject oversized primitives like the hardware, and bound raster work to VRAM.
        if a.0.max(b.0).max(c.0) - a.0.min(b.0).min(c.0) >= 1024
            || a.1.max(b.1).max(c.1) - a.1.min(b.1).min(c.1) >= 512
        {
            return;
        }
        let min_x = a.0.min(b.0).min(c.0).max(self.state.draw_min.0);
        let max_x = a.0.max(b.0).max(c.0).min(self.state.draw_max.0);
        let min_y = a.1.min(b.1).min(c.1).max(self.state.draw_min.1);
        let max_y = a.1.max(b.1).max(c.1).min(self.state.draw_max.1);
        // Pixel-center affine interpolation with a top-left rule; hardware subpixel precision is approximate.
        let (a2, b2, c2) = ((a.0 * 2, a.1 * 2), (b.0 * 2, b.1 * 2), (c.0 * 2, c.1 * 2));
        let total = area.abs() * 4;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let point = (x * 2 + 1, y * 2 + 1);
                let weights = [
                    edge(b2, c2, point),
                    edge(c2, a2, point),
                    edge(a2, b2, point),
                ];
                if ![(b, c), (c, a), (a, b)]
                    .iter()
                    .zip(weights)
                    .all(|(&(p, q), w)| w > 0 || (w == 0 && top_left(p, q)))
                {
                    continue;
                }
                let interpolate = |values: [i64; 3]| {
                    values.iter().zip(weights).map(|(v, w)| v * w).sum::<i64>() / total
                };
                let color = std::array::from_fn(|n| interpolate(vertices.map(|v| v.color[n])));
                let uv = std::array::from_fn(|n| interpolate(vertices.map(|v| v.uv[n])));
                self.shade_pixel(y as usize * VRAM_WIDTH + x as usize, color, uv, style);
            }
        }
    }
}

fn edge(a: (i32, i32), b: (i32, i32), p: (i32, i32)) -> i64 {
    i64::from(b.0 - a.0) * i64::from(p.1 - a.1) - i64::from(b.1 - a.1) * i64::from(p.0 - a.0)
}
fn top_left(a: (i32, i32), b: (i32, i32)) -> bool {
    b.1 < a.1 || (b.1 == a.1 && b.0 > a.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::Texture;
    #[test]
    fn incremental_triangles_match_original_pixels_for_all_shading_modes() {
        let mut random = 0x3141_5926_u32;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random
        };
        let mut base = Rasterizer::default();
        for texel in &mut base.vram {
            *texel = next() as u16;
        }
        for case in 0..400 {
            let mut fast = base.clone();
            fast.state.draw_min = (3, 5);
            fast.state.draw_max = (62, 58);
            fast.state.draw_mode = (next() & 0x1ff) | 0x200;
            fast.state.texture_window = next() & 0xfffff;
            fast.state.mask_set = case & 1 != 0;
            fast.state.mask_check = case & 2 != 0;
            let style = DrawStyle {
                texture: (case % 3 != 0).then_some(Texture {
                    raw: case & 4 != 0,
                    clut: next() as u16,
                }),
                transparent: case & 8 != 0,
                dither: case & 16 != 0,
            };
            let mut vertices = std::array::from_fn(|_| Vertex {
                position: ((next() % 80) as i32 - 8, (next() % 80) as i32 - 8),
                color: std::array::from_fn(|_| i64::from(next() & 255)),
                uv: std::array::from_fn(|_| i64::from(next() & 255)),
            });
            if case & 32 != 0 {
                vertices[1].color = vertices[0].color;
                vertices[2].color = vertices[0].color;
            }
            let mut reference = fast.clone();
            fast.triangle(vertices, style);
            reference.triangle_reference(vertices, style);
            assert_eq!(fast.vram, reference.vram, "raster case {case}");
        }
    }
}

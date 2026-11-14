//! Translate the original GP0 front end into integer compute primitives.
//! The wire record is 32 words, shared with raster.wgsl (no host ABI casts).
use super::super::rasterizer::{DrawState, RasterCommand};
use super::super::{DrawStyle, Texture, Vertex, color_channels, color15, sign11};

pub(super) const FILL: u32 = 0;
pub(super) const RECTANGLE: u32 = 1;
pub(super) const TRIANGLE: u32 = 2;
pub(super) const LINE: u32 = 3;
pub(super) const UPLOAD: u32 = 4;
pub(super) const COPY: u32 = 5;
pub(super) const TILE_COUNT: usize = 128 * 64;

#[derive(Clone, Copy)]
pub(super) struct Primitive(pub [u32; 32]);

impl Primitive {
    fn new(kind: u32, state: DrawState, style: DrawStyle) -> Self {
        let mut words = [0; 32];
        words[0] = kind;
        words[5] = state.draw_mode;
        words[6] = state.texture_window;
        words[7] = u32::from(state.mask_set)
            | (u32::from(state.mask_check) << 1)
            | (u32::from(style.texture.is_some()) << 2)
            | (u32::from(style.texture.is_some_and(|t| t.raw)) << 3)
            | (u32::from(style.transparent) << 4)
            | (u32::from(style.dither) << 5);
        words[8] = style.texture.map_or(0, |t| u32::from(t.clut));
        Self(words)
    }

    fn bounds(&mut self, bounds: [i32; 4], state: Option<DrawState>) -> bool {
        let [mut left, mut top, mut right, mut bottom] = bounds;
        if let Some(state) = state {
            left = left.max(state.draw_min.0);
            top = top.max(state.draw_min.1);
            right = right.min(state.draw_max.0);
            bottom = bottom.min(state.draw_max.1);
        }
        left = left.max(0);
        top = top.max(0);
        right = right.min(1023);
        bottom = bottom.min(511);
        self.0[1..5].copy_from_slice(&[left as u32, top as u32, right as u32, bottom as u32]);
        left <= right && top <= bottom
    }

    fn vertex(&mut self, index: usize, v: Vertex) {
        self.0[9 + index] =
            v.color[0] as u32 | ((v.color[1] as u32) << 8) | ((v.color[2] as u32) << 16);
        self.0[12 + index * 2] = v.position.0 as u32;
        self.0[13 + index * 2] = v.position.1 as u32;
        self.0[18 + index * 2] = v.uv[0] as u32;
        self.0[19 + index * 2] = v.uv[1] as u32;
    }

    pub(super) fn write_tiles(self, mut visit: impl FnMut(usize)) {
        // Wrapped copies use a broad bounding box, then test membership in the
        // shader. Extra tiles are harmless, including for dependency tracking.
        for y in self.0[2] / 8..=self.0[4] / 8 {
            for x in self.0[1] / 8..=self.0[3] / 8 {
                visit((y * 128 + x) as usize);
            }
        }
    }

    pub(super) fn read_tiles(self, mut visit: impl FnMut(usize)) {
        if self.0[0] == COPY {
            wrapped_tiles(self.0[26], self.0[27], self.0[30], self.0[31], visit);
        } else if self.0[7] & 4 != 0 {
            // Bound texture reads by the UV range, including window transforms,
            // wrap and sprite flips. Tracking a whole 256-row page needlessly
            // serialized sprites whose source and destination never overlap.
            let mode = self.0[5];
            let depth = ((mode >> 7) & 3).min(2);
            let (mut columns, mut rows) = (0_u128, 0_u64);
            for axis in 0..2 {
                let (low, high) = self.uv_range(axis);
                let (low, high) = if high - low >= 255 {
                    (0, 255)
                } else {
                    (low, high)
                };
                let mask = ((self.0[6] >> (axis * 5)) & 31) * 8;
                let offset = ((self.0[6] >> (10 + axis * 5)) * 8) & mask;
                // A texture-window transform only clears/replaces bits. All
                // UVs in one source tile therefore map to the same VRAM tile,
                // even for negative/flipped coordinates or a wrapped window.
                // Visit at most 32 groups, instead of up to 256 coordinates.
                let shift = if axis == 0 { 5 - depth } else { 3 };
                let span = 1 << shift;
                for group in low.div_euclid(span)..=high.div_euclid(span) {
                    let coordinate = group * span;
                    let uv = ((coordinate as u32 & 255) & !mask) | offset;
                    if axis == 0 {
                        let word = ((mode & 15) * 64 + (uv >> (2 - depth))) & 1023;
                        columns |= 1 << (word / 8);
                    } else {
                        let row = (((mode >> 4) & 1) * 256 + uv) & 511;
                        rows |= 1 << (row / 8);
                    }
                }
            }
            while rows != 0 {
                let y = rows.trailing_zeros() as usize;
                rows &= rows - 1;
                let mut xs = columns;
                while xs != 0 {
                    visit(y * 128 + xs.trailing_zeros() as usize);
                    xs &= xs - 1;
                }
            }
            if depth < 2 {
                wrapped_tiles(
                    (self.0[8] & 63) * 16,
                    (self.0[8] >> 6) & 511,
                    if depth == 0 { 16 } else { 256 },
                    1,
                    visit,
                );
            }
        }
    }

    fn uv_range(self, axis: usize) -> (i32, i32) {
        let first = self.0[18 + axis] as i32;
        if self.0[0] == RECTANGLE {
            let step = if self.0[5] & (0x1000 << axis) == 0 {
                1
            } else {
                -1
            };
            let start = self.0[12 + axis] as i32;
            let a = first + (self.0[1 + axis] as i32 - start) * step;
            let b = first + (self.0[3 + axis] as i32 - start) * step;
            (a.min(b), a.max(b))
        } else {
            let second = self.0[20 + axis] as i32;
            let third = self.0[22 + axis] as i32;
            (first.min(second).min(third), first.max(second).max(third))
        }
    }
}

fn wrapped_tiles(x: u32, y: u32, width: u32, height: u32, mut visit: impl FnMut(usize)) {
    for ty in y / 8..=(y + height - 1) / 8 {
        for tx in x / 8..=(x + width - 1) / 8 {
            visit(((ty & 63) * 128 + (tx & 127)) as usize);
        }
    }
}

fn style(packet: &[u32; 12], textured: bool, dither: bool) -> DrawStyle {
    DrawStyle {
        texture: textured.then_some(Texture {
            clut: (packet[2] >> 16) as u16,
            raw: packet[0] & (1 << 24) != 0,
        }),
        transparent: packet[0] & (1 << 25) != 0,
        dither,
    }
}

fn position(word: u32, state: DrawState) -> (i32, i32) {
    (
        sign11(word) + state.offset.0,
        sign11(word >> 16) + state.offset.1,
    )
}

const PLAIN: DrawStyle = DrawStyle {
    texture: None,
    transparent: false,
    dither: false,
};

pub(super) fn decode(command: RasterCommand, output: &mut Vec<Primitive>) {
    match command {
        RasterCommand::Pixel(index, color, state) => {
            let mut p = Primitive::new(UPLOAD, state, PLAIN);
            p.0[1] = (index & 1023) as u32;
            p.0[2] = (index / 1024) as u32;
            p.0[3] = p.0[1];
            p.0[4] = p.0[2];
            p.0[9] = u32::from(color);
            output.push(p);
        }
        RasterCommand::Line(vertices, style, state) => line(vertices, style, state, output),
        RasterCommand::Packet(p, mut state) => match p[0] >> 24 {
            0x02 => fill(&p, output),
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
                    let i = 1 + n * stride;
                    *vertex = Vertex {
                        position: position(p[i], state),
                        color: color_channels(p[if shaded { i - 1 } else { 0 }]),
                        uv: if textured {
                            [i64::from(p[i + 1] & 255), i64::from((p[i + 1] >> 8) & 255)]
                        } else {
                            [0; 2]
                        },
                    };
                }
                if textured {
                    state.draw_mode = (state.draw_mode & !0x1ff) | ((p[2 + stride] >> 16) & 0x1ff);
                }
                let style = style(
                    &p,
                    textured,
                    state.draw_mode & 0x200 != 0 && (shaded || textured),
                );
                triangle(
                    [vertices[0], vertices[1], vertices[2]],
                    style,
                    state,
                    output,
                );
                if quad {
                    triangle(
                        [vertices[1], vertices[2], vertices[3]],
                        style,
                        state,
                        output,
                    );
                }
            }
            0x60..=0x7f => {
                let textured = p[0] & (1 << 26) != 0;
                let (x, y) = position(p[1], state);
                let size = p[2 + usize::from(textured)];
                let (width, height) = match (p[0] >> 27) & 3 {
                    1 => (1, 1),
                    2 => (8, 8),
                    3 => (16, 16),
                    _ => ((size & 1023) as i32, ((size >> 16) & 511) as i32),
                };
                let mut prim = Primitive::new(RECTANGLE, state, style(&p, textured, false));
                prim.vertex(
                    0,
                    Vertex {
                        position: (x, y),
                        color: color_channels(p[0]),
                        uv: [i64::from(p[2] & 255), i64::from((p[2] >> 8) & 255)],
                    },
                );
                if prim.bounds([x, y, x + width - 1, y + height - 1], Some(state)) {
                    output.push(prim);
                }
            }
            0x80 => {
                copy(&p, state, output);
            }
            _ => {}
        },
    }
}

fn fill(p: &[u32; 12], output: &mut Vec<Primitive>) {
    let (x, y) = (p[1] & 0x3f0, (p[1] >> 16) & 511);
    let (width, height) = (((p[2] & 1023) + 15) & !15, (p[2] >> 16) & 511);
    if width == 0 || height == 0 {
        return;
    }
    for (left, w) in [
        (x, width.min(1024 - x)),
        (0, (x + width).saturating_sub(1024)),
    ] {
        for (top, h) in [
            (y, height.min(512 - y)),
            (0, (y + height).saturating_sub(512)),
        ] {
            if w == 0 || h == 0 {
                continue;
            }
            let mut prim = Primitive::new(FILL, DrawState::default(), PLAIN);
            prim.0[9] = u32::from(color15(p[0]));
            prim.bounds(
                [
                    left as i32,
                    top as i32,
                    (left + w - 1) as i32,
                    (top + h - 1) as i32,
                ],
                None,
            );
            output.push(prim);
        }
    }
}

fn triangle(
    mut vertices: [Vertex; 3],
    style: DrawStyle,
    state: DrawState,
    output: &mut Vec<Primitive>,
) {
    let [a, b, c] = vertices.map(|v| v.position);
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let bounds = [
        a.0.min(b.0).min(c.0),
        a.1.min(b.1).min(c.1),
        a.0.max(b.0).max(c.0),
        a.1.max(b.1).max(c.1),
    ];
    if area == 0 || bounds[2] - bounds[0] >= 1024 || bounds[3] - bounds[1] >= 512 {
        return;
    }
    if area < 0 {
        vertices.swap(1, 2);
    }
    let mut prim = Primitive::new(TRIANGLE, state, style);
    if !prim.bounds(bounds, Some(state)) {
        return;
    }
    // Inside a <=1023*511 box, doubled-coordinate barycentric weights sum to
    // <=2,091,012. Multiplying by any 8-bit attribute stays below i32::MAX.
    prim.0[24] = area.unsigned_abs() * 4;
    for (n, v) in vertices.into_iter().enumerate() {
        prim.vertex(n, v);
    }
    output.push(prim);
}

fn line(vertices: [Vertex; 2], style: DrawStyle, state: DrawState, output: &mut Vec<Primitive>) {
    let [a, b] = vertices.map(|v| v.position);
    if (b.0 - a.0).abs() >= 1024 || (b.1 - a.1).abs() >= 512 {
        return;
    }
    let mut prim = Primitive::new(LINE, state, style);
    if !prim.bounds(
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)],
        Some(state),
    ) {
        return;
    }
    for (n, v) in vertices.into_iter().enumerate() {
        prim.vertex(n, v);
    }
    output.push(prim);
}

fn copy(p: &[u32; 12], state: DrawState, output: &mut Vec<Primitive>) {
    let mut prim = Primitive::new(COPY, state, PLAIN);
    prim.0[26] = p[1] & 1023;
    prim.0[27] = (p[1] >> 16) & 511;
    prim.0[28] = p[2] & 1023;
    prim.0[29] = (p[2] >> 16) & 511;
    prim.0[30] = (p[3].wrapping_sub(1) & 1023) + 1;
    prim.0[31] = ((p[3] >> 16).wrapping_sub(1) & 511) + 1;
    let (x, y, width, height) = (prim.0[28], prim.0[29], prim.0[30], prim.0[31]);
    prim.bounds(
        [
            if x + width > 1024 { 0 } else { x as i32 },
            if y + height > 512 { 0 } else { y as i32 },
            (x + width - 1).min(1023) as i32,
            (y + height - 1).min(511) as i32,
        ],
        None,
    );
    output.push(prim);
}

//! Rasterize the completed geometry buffer as engine A background 0.
use super::{Geometry, H, Polygon, Vertex, Vram, W};
use crate::display::rgb;

#[derive(Clone, Copy, Default)]
struct ScreenVertex {
    xy: [f32; 2],
    z: f32,
    inv_w: f32,
    uv: [f32; 2],
    color: [f32; 3],
}
impl Vertex {
    fn screen(self, viewport: u32) -> ScreenVertex {
        let [x1, y1, x2, y2] = viewport.to_le_bytes();
        let width = u16::from(x2.wrapping_sub(x1)) + 1;
        let height = u16::from(y2.wrapping_sub(y1)) + 1;
        let [x, y, z, w] = self.clip;
        let inv_w = w.recip();
        ScreenVertex {
            xy: [
                f32::from(x1) + 1.0_f32.midpoint(x * inv_w) * f32::from(width),
                H as f32 - f32::from(y1) - 1.0_f32.midpoint(y * inv_w) * f32::from(height),
            ],
            z: 1.0_f32.midpoint(z * inv_w),
            inv_w,
            uv: self.uv,
            color: self.color,
        }
    }
    fn lerp(self, other: Self, t: f32) -> Self {
        let interpolate = |a: f32, b: f32| a + (b - a) * t;
        Self {
            clip: std::array::from_fn(|i| interpolate(self.clip[i], other.clip[i])),
            uv: std::array::from_fn(|i| interpolate(self.uv[i], other.uv[i])),
            color: std::array::from_fn(|i| interpolate(self.color[i], other.color[i])),
        }
    }
}
fn distance(v: Vertex, plane: usize) -> f32 {
    if plane & 1 == 0 {
        v.clip[3] + v.clip[plane / 2]
    } else {
        v.clip[3] - v.clip[plane / 2]
    }
}

// Clipping quad against six planes can increase its vertex count by at most six.
fn clipped(poly: &Polygon) -> ([Vertex; 10], usize) {
    let mut verts = [Vertex::default(); 10];
    let mut n = usize::from(poly.count);
    verts[..n].copy_from_slice(&poly.vertices[..n]);
    for plane in 0..6 {
        if verts[..n].iter().all(|&v| distance(v, plane) >= 0.) {
            continue;
        }
        let input = verts;
        let mut prev = input[n - 1];
        let mut dp = distance(prev, plane);
        let count = n;
        n = 0;
        for &v in &input[..count] {
            let d = distance(v, plane);
            if (dp > 0. && d < 0.) || (dp < 0. && d > 0.) {
                // Concave / self-intersecting guest quads are not hardware-defined.
                if n == verts.len() {
                    return (verts, 0);
                }
                verts[n] = prev.lerp(v, dp / (dp - d));
                n += 1;
            }
            if d >= 0. {
                if n == verts.len() {
                    return (verts, 0);
                }
                verts[n] = v;
                n += 1;
            }
            prev = v;
            dp = d;
        }
        if n < 3 {
            return (verts, 0);
        }
    }
    (verts, n)
}
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn top_left(a: [f32; 2], b: [f32; 2]) -> bool {
    b[1] < a[1] || b[1].total_cmp(&a[1]).is_eq() && b[0] > a[0]
}

struct Raster<'a> {
    vram: &'a Vram,
    output: &'a mut [u8],
    depths: Vec<f32>,
    control: u16,
    alpha_ref: u8,
    w_buffer: bool,
}
impl Geometry {
    pub(in crate::display) fn render(&self, vram: &Vram, control: u16, output: &mut [u8]) {
        let clear_alpha = ((self.clear_color >> 16) & 31) as u8;
        if clear_alpha != 0 {
            let color = rgb(self.clear_color as u16);
            for p in output.as_chunks_mut::<3>().0 {
                composite(p, color, clear_alpha);
            }
        }
        if self.front.is_empty() {
            return;
        }
        let depth = if self.swap & 2 != 0 {
            f32::INFINITY
        } else {
            f32::from(self.clear_depth) / 32767.
        };
        let mut raster = Raster {
            vram,
            output,
            depths: vec![depth; W * H],
            control,
            alpha_ref: self.alpha_ref,
            w_buffer: self.swap & 2 != 0,
        };
        // Opaque polygons are drawn before translucent geometry.
        for translucent in [false, true] {
            for p in &self.front {
                let alpha = (p.attributes >> 16) & 31;
                let alpha_texture =
                    matches!((p.texture >> 26) & 7, 1 | 6) && (p.attributes >> 4) & 3 != 1;
                if translucent == ((alpha != 31 && alpha != 0) || alpha_texture) {
                    raster.polygon(p);
                }
            }
        }
    }
}
impl Raster<'_> {
    fn polygon(&mut self, p: &Polygon) {
        if p.attributes & 0xc0 == 0 {
            return;
        }
        let (verts, n) = clipped(p);
        if n < 3 || verts[..n].iter().any(|v| v.clip[3] <= 0.) {
            return;
        }
        let first = verts[0].screen(p.viewport);
        let mut second = verts[1].screen(p.viewport);
        for v in &verts[2..n] {
            let third = v.screen(p.viewport);
            self.triangle(p, [first, second, third]);
            second = third;
        }
    }
    fn triangle(&mut self, poly: &Polygon, mut verts: [ScreenVertex; 3]) {
        let mut area = edge(verts[0].xy, verts[1].xy, verts[2].xy);
        if area == 0. || !area.is_finite() {
            return;
        }
        // CCW front faces in clip space have negative area after the LCD Y flip.
        let face = if area < 0. { 0x80 } else { 0x40 };
        if poly.attributes & face == 0 {
            return;
        }
        if area < 0. {
            verts.swap(1, 2);
            area = -area;
        }
        let [a, b, c] = verts;
        let min_x = a.xy[0].min(b.xy[0]).min(c.xy[0]).floor().max(0.) as usize;
        let max_x = a.xy[0].max(b.xy[0]).max(c.xy[0]).ceil().min(W as f32) as usize;
        let min_y = a.xy[1].min(b.xy[1]).min(c.xy[1]).floor().max(0.) as usize;
        let max_y = a.xy[1].max(b.xy[1]).max(c.xy[1]).ceil().min(H as f32) as usize;
        let edges = [(b.xy, c.xy), (c.xy, a.xy), (a.xy, b.xy)];
        let inclusive = edges.map(|(start, end)| top_left(start, end));
        let inv_area = area.recip();
        for y in min_y..max_y {
            for x in min_x..max_x {
                let sample = [x as f32 + 0.5, y as f32 + 0.5];
                let weights = edges.map(|(a, b)| edge(a, b, sample));
                if weights
                    .iter()
                    .zip(inclusive)
                    .any(|(&barycentric, inc)| barycentric < 0. || barycentric == 0. && !inc)
                {
                    continue;
                }
                let weights = weights.map(|s| s * inv_area);
                let inv_w = weights[0] * a.inv_w + weights[1] * b.inv_w + weights[2] * c.inv_w;
                let depth = if self.w_buffer {
                    inv_w.recip()
                } else {
                    weights[0] * a.z + weights[1] * b.z + weights[2] * c.z
                };
                let index = y * W + x;
                let old = self.depths[index];
                if poly.attributes & (1 << 14) != 0 {
                    if (depth - old).abs() > 1. / 32768. {
                        continue;
                    }
                } else if depth >= old {
                    continue;
                }
                let divisor = inv_w.recip();
                let barycentric = [
                    weights[0] * a.inv_w * divisor,
                    weights[1] * b.inv_w * divisor,
                    weights[2] * c.inv_w * divisor,
                ];
                let interp = |values: [f32; 3]| {
                    barycentric[0] * values[0]
                        + barycentric[1] * values[1]
                        + barycentric[2] * values[2]
                };
                let uv: [i32; 2] =
                    std::array::from_fn(|i| interp(verts.map(|v| v.uv[i])).floor() as i32);
                let color: [u16; 3] = std::array::from_fn(|i| {
                    interp(verts.map(|v| v.color[i])).round().clamp(0., 31.) as u16
                });
                let Some((shaded, alpha)) = self.shade(poly, uv, color) else {
                    continue;
                };
                let pixel = &mut self.output[3 * index..3 * index + 3];
                composite(
                    pixel,
                    rgb(shaded),
                    if self.control & 8 != 0 { alpha } else { 31 },
                );
                if alpha == 31 || poly.attributes & (1 << 11) != 0 {
                    self.depths[index] = depth;
                }
            }
        }
    }
    fn shade(&self, poly: &Polygon, uv: [i32; 2], color: [u16; 3]) -> Option<(u16, u8)> {
        let texture = if self.control & 1 != 0 {
            poly.texture
        } else {
            0
        };
        let (tex, ta) = self
            .vram
            .sample_texture(texture, poly.palette, uv[0], uv[1])?;
        let poly_alpha = ((poly.attributes >> 16) & 31) as u8;
        let poly_alpha = if poly_alpha == 0 { 31 } else { poly_alpha };
        let decal = (poly.attributes >> 4) & 3 == 1;
        let alpha = if decal {
            poly_alpha
        } else {
            (((u16::from(ta) + 1) * (u16::from(poly_alpha) + 1) - 1) >> 5) as u8
        };
        if alpha == 0 || self.control & 4 != 0 && alpha <= self.alpha_ref {
            return None;
        }
        let mut shaded = 0;
        for (i, vc) in color.into_iter().enumerate() {
            let tc = (tex >> (5 * i)) & 31;
            let channel = if decal && (texture >> 26) & 7 != 0 {
                (tc * u16::from(ta) + vc * (31 - u16::from(ta))) / 31
            } else {
                ((tc + 1) * (vc + 1) - 1) >> 5
            };
            shaded |= channel << (5 * i);
        }
        Some((shaded, alpha))
    }
}
fn composite(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    if alpha == 31 {
        pixel.copy_from_slice(&color);
        return;
    }
    let alpha = u16::from(alpha) + 1;
    for (dst, src) in pixel.iter_mut().zip(color) {
        *dst = ((u16::from(src) * alpha + u16::from(*dst) * (32 - alpha)) >> 5) as u8;
    }
}

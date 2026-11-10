// Original PS1 integer rasterizer. Every parallel invocation owns one VRAM
// pixel; the host separates batches whenever texture/copy reads alias writes.
struct Primitive { w: array<u32, 32> }
@group(0) @binding(0) var<storage, read_write> vram: array<u32>;
@group(0) @binding(1) var<storage, read> commands: array<Primitive>;
@group(0) @binding(2) var<storage, read> tiles: array<u32>;
@group(0) @binding(3) var<storage, read> uploads: array<u32>;
@group(0) @binding(4) var<storage, read_write> output: array<u32>;
@group(0) @binding(5) var<uniform> params: array<vec4<u32>, 2>;

const DITHER = array<i32, 16>(-4, 0, -3, 1, 2, -2, 3, -1, -3, 1, -4, 0, 3, -1, 2, -2);

fn color(word: u32) -> vec3<i32> {
    return vec3<i32>(i32(word & 255u), i32((word >> 8u) & 255u), i32((word >> 16u) & 255u));
}
fn edge(a: vec2<i32>, b: vec2<i32>, p: vec2<i32>) -> i32 {
    let ab = b - a;
    let ap = p - a;
    return ab.x * ap.y - ab.y * ap.x;
}
fn covered(weight: i32, a: vec2<i32>, b: vec2<i32>) -> bool {
    return weight > 0 || (weight == 0 && (b.y < a.y || (b.y == a.y && b.x > a.x)));
}
fn floor_div(n: vec2<i32>, d: i32) -> vec2<i32> {
    return n / d - select(vec2<i32>(0), vec2<i32>(1), (n < vec2<i32>(0)) & (n % d != vec2<i32>(0)));
}
fn sample_texture(uv: vec2<i32>, mode: u32, window: u32, clut: u32) -> u32 {
    let mask = vec2<u32>(window & 31u, (window >> 5u) & 31u) * 8u;
    let xy = ((vec2<u32>(uv) & vec2<u32>(255u)) & ~mask)
        | ((vec2<u32>(window >> 10u, window >> 15u) * 8u) & mask);
    let depth = (mode >> 7u) & 3u;
    let x = ((mode & 15u) * 64u + (xy.x >> (2u - min(depth, 2u)))) & 1023u;
    let y = (((mode >> 4u) & 1u) * 256u + xy.y) & 511u;
    let packed = vram[y * 1024u + x];
    if depth >= 2u { return packed; }
    var index = (packed >> ((xy.x & 3u) * 4u)) & 15u;
    if depth == 1u { index = (packed >> ((xy.x & 1u) * 8u)) & 255u; }
    return vram[((clut >> 6u) & 511u) * 1024u + (((clut & 63u) * 16u + index) & 1023u)];
}
fn write_mask(value: u32, old: u32, flags: u32) -> u32 {
    if (flags & 2u) != 0u && (old & 32768u) != 0u { return old; }
    return value | ((flags & 1u) << 15u);
}
fn shade(p: Primitive, xy: vec2<u32>, old: u32, input_color: vec3<i32>, uv: vec2<i32>) -> u32 {
    let flags = p.w[7];
    var rgb = input_color;
    var mask = 0u;
    var transparent = (flags & 16u) != 0u;
    let raw = (flags & 12u) == 12u;
    if (flags & 4u) != 0u {
        let texel = sample_texture(uv, p.w[5], p.w[6], p.w[8]);
        if texel == 0u { return old; }
        mask = texel & 32768u;
        transparent = transparent && mask != 0u;
        let t = vec3<i32>(i32(texel & 31u), i32((texel >> 5u) & 31u), i32((texel >> 10u) & 31u));
        if raw { rgb = t * 8; } else { rgb = t * rgb / 16; }
    }
    if (flags & 32u) != 0u && !raw { rgb += vec3<i32>(DITHER[(xy.y & 3u) * 4u + (xy.x & 3u)]); }
    var foreground = vec3<u32>(clamp(rgb, vec3<i32>(0), vec3<i32>(255))) >> vec3<u32>(3u);
    if transparent {
        let background = vec3<u32>(old & 31u, (old >> 5u) & 31u, (old >> 10u) & 31u);
        switch (p.w[5] >> 5u) & 3u {
            case 0u: { foreground = background / 2u + foreground / 2u; }
            case 1u: { foreground = min(background + foreground, vec3<u32>(31u)); }
            case 2u: { foreground = background - min(background, foreground); }
            default: { foreground = min(background + foreground / 4u, vec3<u32>(31u)); }
        }
    }
    return write_mask(mask | foreground.x | (foreground.y << 5u) | (foreground.z << 10u), old, flags);
}

fn draw(p: Primitive, xy: vec2<u32>, old: u32) -> u32 {
    if xy.x < p.w[1] || xy.y < p.w[2] || xy.x > p.w[3] || xy.y > p.w[4] { return old; }
    let kind = p.w[0];
    if kind == 0u { return p.w[9]; } // fill ignores mask and drawing area
    if kind == 4u { return write_mask(uploads[p.w[25] + xy.x - p.w[1]], old, p.w[7]); }
    if kind == 5u {
        let delta = (xy - vec2<u32>(p.w[28], p.w[29])) & vec2<u32>(1023u, 511u);
        if delta.x >= p.w[30] || delta.y >= p.w[31] { return old; }
        let source = (vec2<u32>(p.w[26], p.w[27]) + delta) & vec2<u32>(1023u, 511u);
        return write_mask(vram[source.y * 1024u + source.x], old, p.w[7]);
    }
    let point = vec2<i32>(xy);
    let a = vec2<i32>(i32(p.w[12]), i32(p.w[13]));
    var rgb = color(p.w[9]);
    var uv = vec2<i32>(i32(p.w[18]), i32(p.w[19]));
    if kind == 1u {
        let flip = vec2<bool>((p.w[5] & 4096u) != 0u, (p.w[5] & 8192u) != 0u);
        uv += (point - a) * select(vec2<i32>(1), vec2<i32>(-1), flip);
    } else if kind == 2u {
        let b = vec2<i32>(i32(p.w[14]), i32(p.w[15]));
        let c = vec2<i32>(i32(p.w[16]), i32(p.w[17]));
        let center = point * 2 + vec2<i32>(1);
        let weights = vec3<i32>(edge(b * 2, c * 2, center), edge(c * 2, a * 2, center), edge(a * 2, b * 2, center));
        if !covered(weights.x, b, c) || !covered(weights.y, c, a) || !covered(weights.z, a, b) { return old; }
        let total = i32(p.w[24]);
        rgb = (rgb * weights.x + color(p.w[10]) * weights.y + color(p.w[11]) * weights.z) / total;
        uv = (uv * weights.x + vec2<i32>(i32(p.w[20]), i32(p.w[21])) * weights.y
            + vec2<i32>(i32(p.w[22]), i32(p.w[23])) * weights.z) / total;
    } else if kind == 3u {
        let delta = vec2<i32>(i32(p.w[14]), i32(p.w[15])) - a;
        let length = max(abs(delta.x), abs(delta.y));
        let divisor = max(length, 1);
        var step = 0;
        if abs(delta.x) >= abs(delta.y) && delta.x != 0 { step = (point.x - a.x) * sign(delta.x); }
        else if delta.y != 0 { step = (point.y - a.y) * sign(delta.y); }
        if step < 0 || step > length { return old; }
        let at = floor_div(a * divisor + delta * step + vec2<i32>(divisor / 2), divisor);
        if any(at != point) { return old; }
        rgb = (rgb * (divisor - step) + color(p.w[10]) * step) / divisor;
    }
    return shade(p, xy, old, rgb, uv);
}

@compute @workgroup_size(8, 8)
fn raster(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let header = group.x * 4u;
    let xy = vec2<u32>(tiles[header], tiles[header + 1u]) * 8u + local.xy;
    let index = xy.y * 1024u + xy.x;
    var value = vram[index];
    let offset = tiles[header + 2u];
    let count = tiles[header + 3u];
    for (var i = 0u; i < count; i++) { value = draw(commands[tiles[offset + i]], xy, value); }
    vram[index] = value;
}

// Feedback/overlapping copy must observe preceding writes in row-major order.
// These rare commands remain on the physical GPU, using a single invocation.
@compute @workgroup_size(1)
fn ordered() {
    let p = commands[0];
    if p.w[0] == 5u {
        for (var y = 0u; y < p.w[31]; y++) {
            for (var x = 0u; x < p.w[30]; x++) {
                let source = (vec2<u32>(p.w[26], p.w[27]) + vec2<u32>(x, y)) & vec2<u32>(1023u, 511u);
                let destination = (vec2<u32>(p.w[28], p.w[29]) + vec2<u32>(x, y)) & vec2<u32>(1023u, 511u);
                let index = destination.y * 1024u + destination.x;
                vram[index] = write_mask(vram[source.y * 1024u + source.x], vram[index], p.w[7]);
            }
        }
    } else {
        for (var y = p.w[2]; y <= p.w[4]; y++) {
            for (var x = p.w[1]; x <= p.w[3]; x++) {
                let index = y * 1024u + x;
                vram[index] = draw(p, vec2<u32>(x, y), vram[index]);
            }
        }
    }
}

@compute @workgroup_size(64)
fn scanout(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params[0].x;
    if id.x >= width * params[0].y { return; }
    if params[1].x != 0u { output[id.x] = 0u; return; }
    let x = id.x % width;
    let row = ((id.x / width + params[0].w) & 511u) * 1024u;
    var rgb: vec3<u32>;
    if params[1].y != 0u {
        let byte = params[0].z * 2u + x * 3u;
        for (var channel = 0u; channel < 3u; channel++) {
            let address = byte + channel;
            rgb[channel] = (vram[row + ((address / 2u) & 1023u)] >> ((address & 1u) * 8u)) & 255u;
        }
    } else {
        let pixel = vram[row + ((params[0].z + x) & 1023u)];
        rgb = vec3<u32>(pixel & 31u, (pixel >> 5u) & 31u, (pixel >> 10u) & 31u);
        rgb = (rgb << vec3<u32>(3u)) | (rgb >> vec3<u32>(2u));
    }
    output[id.x] = rgb.x | (rgb.y << 8u) | (rgb.z << 16u);
}

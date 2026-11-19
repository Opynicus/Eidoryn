// Each tile invocation owns a native PS1 word and its host-side samples.
// Texture pages and CLUTs always address the native plane, including feedback.
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
fn edge(a: vec2<i32>, b: vec2<i32>, center: vec2<i32>, units: i32) -> i32 {
    let ab = b - a;
    let ap = center - a * units;
    return ab.x * ap.y - ab.y * ap.x;
}
fn covered(weight: i32, a: vec2<i32>, b: vec2<i32>) -> bool {
    return weight > 0 || (weight == 0 && (b.y < a.y || (b.y == a.y && b.x > a.x)));
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
fn masked_destination(flags: u32, old: u32) -> bool {
    return (flags & 2u) != 0u && (old & 32768u) != 0u;
}

fn write_mask(value: u32, old: u32, flags: u32) -> u32 {
    if masked_destination(flags, old) { return old; }
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

fn draw(p: Primitive, xy: vec2<u32>, sub: vec2<u32>, scale: u32, old: u32) -> u32 {
    let kind = p.w[0];
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
        // Only the sample displacement is in subpixel units. Vertex differences
        // stay native. Covered weights sum to area * 2*S, so even their weighted
        // 8-bit sum is bounded by 1023 * 511 * 8 * 255 < 2^31 at 4x.
        let units = i32(2u * scale);
        let center = point * units + vec2<i32>(2u * sub + 1u);
        let weights = vec3<i32>(edge(b, c, center, units), edge(c, a, center, units), edge(a, b, center, units));
        if !covered(weights.x, b, c) || !covered(weights.y, c, a) || !covered(weights.z, a, b) { return old; }
        let total = i32(p.w[24]) * units;
        rgb = (rgb * weights.x + color(p.w[10]) * weights.y + color(p.w[11]) * weights.z) / total;
        uv = (uv * weights.x + vec2<i32>(i32(p.w[20]), i32(p.w[21])) * weights.y
            + vec2<i32>(i32(p.w[22]), i32(p.w[23])) * weights.z) / total;
    } else if kind == 3u {
        let delta = vec2<i32>(i32(p.w[14]), i32(p.w[15])) - a;
        let length = max(abs(delta.x), abs(delta.y));
        if length > 0 {
            // The native DDA is a one-pixel-wide strip with inclusive endpoints.
            // Evaluate that same width at subpixel centers, clamping at each cap.
            let units = i32(2u * scale);
            let offset = (point - a) * units + vec2<i32>(2u * sub + 1u) - vec2<i32>(i32(scale));
            let steep = abs(delta.y) > abs(delta.x);
            let major = select(offset.x, offset.y, steep);
            let minor = select(offset.y, offset.x, steep);
            let extent = length * units;
            let step = clamp(major * sign(select(delta.x, delta.y, steep)), 0, extent);
            let distance = minor * length - select(delta.y, delta.x, steep) * step;
            let half_width = length * i32(scale);
            if distance <= -half_width || distance > half_width { return old; }
            rgb = (rgb * (extent - step) + color(p.w[10]) * step) / extent;
        }
    }
    return shade(p, xy, old, rgb, uv);
}

const NATIVE_WORDS = 1024u * 512u;
fn native_index(xy: vec2<u32>) -> u32 { return xy.y * 1024u + xy.x; }

// One contiguous block per native word makes a wrapped transfer a sequence of
// complete blocks, independent of the scanout origin and display dimensions.
fn sample_base(index: u32, scale: u32) -> u32 { return NATIVE_WORDS + index * scale * scale; }

// The native mask gates the whole transfer. A copied sample retains its source
// mask and detail even when the native source word has a different color/mask.
fn copy_word(p: Primitive, delta: vec2<u32>, index: u32, old: u32) -> u32 {
    if masked_destination(p.w[7], old) { return old; }
    let source = (vec2<u32>(p.w[26], p.w[27]) + delta) & vec2<u32>(1023u, 511u);
    let source_index = native_index(source);
    let scale = params[1].z;
    let mask = (p.w[7] & 1u) << 15u;
    if scale > 1u {
        let source_base = sample_base(source_index, scale);
        let destination_base = sample_base(index, scale);
        for (var i = 0u; i < scale * scale; i++) {
            vram[destination_base + i] = vram[source_base + i] | mask;
        }
    }
    return vram[source_index] | mask;
}

fn apply(p: Primitive, xy: vec2<u32>, old: u32) -> u32 {
    if xy.x < p.w[1] || xy.y < p.w[2] || xy.x > p.w[3] || xy.y > p.w[4] { return old; }
    let scale = params[1].z;
    let base = sample_base(native_index(xy), scale);
    let kind = p.w[0];
    if kind == 5u {
        let delta = (xy - vec2<u32>(p.w[28], p.w[29])) & vec2<u32>(1023u, 511u);
        if delta.x >= p.w[30] || delta.y >= p.w[31] { return old; }
        return copy_word(p, delta, native_index(xy), old);
    }
    if kind == 0u || kind == 4u {
        var value = p.w[9]; // Fills ignore mask settings.
        if kind == 4u {
            if masked_destination(p.w[7], old) { return old; }
            value = uploads[p.w[25] + xy.x - p.w[1]] | ((p.w[7] & 1u) << 15u);
        }
        if scale > 1u {
            for (var i = 0u; i < scale * scale; i++) { vram[base + i] = value; }
        }
        return value;
    }

    if scale > 1u {
        // In an ordered self-textured draw every subsample sees the native
        // destination as it was immediately before this pixel was drawn.
        for (var y = 0u; y < scale; y++) {
            for (var x = 0u; x < scale; x++) {
                let index = base + y * scale + x;
                vram[index] = draw(p, xy, vec2<u32>(x, y), scale, vram[index]);
            }
        }
    }
    return draw(p, xy, vec2<u32>(0u), 1u, old);
}

@compute @workgroup_size(8, 8)
fn raster(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let header = group.x * 4u;
    let xy = vec2<u32>(tiles[header], tiles[header + 1u]) * 8u + local.xy;
    let index = native_index(xy);
    var value = vram[index];
    let offset = tiles[header + 2u];
    let count = tiles[header + 3u];
    for (var i = 0u; i < count; i++) { value = apply(commands[tiles[offset + i]], xy, value); }
    vram[index] = value;
}

// Self-textured primitives must see all preceding native writes in row-major order.
@compute @workgroup_size(1)
fn ordered() {
    let p = commands[0];
    let origin = vec2<u32>(p.w[1], p.w[2]);
    let size = vec2<u32>(p.w[3], p.w[4]) - origin + 1u;
    // Each dispatch resumes the same command at its next row-major pixel.
    let first = p.w[25];
    let end = min(first + 1024u, size.x * size.y);
    for (var i = first; i < end; i++) {
        let xy = (origin + vec2<u32>(i % size.x, i / size.x)) & vec2<u32>(1023u, 511u);
        let index = native_index(xy);
        vram[index] = apply(p, xy, vram[index]);
    }
}

fn copy_at(p: Primitive, delta: vec2<u32>) {
    let xy = (vec2<u32>(p.w[28], p.w[29]) + delta) & vec2<u32>(1023u, 511u);
    let index = native_index(xy);
    vram[index] = copy_word(p, delta, index, vram[index]);
}

@compute @workgroup_size(64)
fn ordered_copy(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = commands[0];
    if p.w[27] == p.w[29] {
        // Source and destination share rows. Each row is independent; propagate
        // writes across horizontal overlaps (including wrap) in ascending X.
        if id.x < p.w[31] {
            for (var x = 0u; x < p.w[30]; x++) { copy_at(p, vec2<u32>(x, id.x)); }
        }
    } else {
        // Different source/destination rows never overlap within one scanline.
        // One workgroup finishes all columns before any lane reads the next row.
        for (var row = 0u; row < 32u; row++) {
            let y = p.w[25] + row;
            if y < p.w[31] {
                for (var x = id.x; x < p.w[30]; x += 64u) { copy_at(p, vec2<u32>(x, y)); }
            }
            storageBarrier();
        }
    }
}

@compute @workgroup_size(8, 8)
fn seed_enhanced(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = native_index(id.xy);
    let scale = params[1].z;
    let base = sample_base(index, scale);
    for (var i = 0u; i < scale * scale; i++) { vram[base + i] = vram[index]; }
}

@compute @workgroup_size(8, 8)
fn scanout(@builtin(global_invocation_id) id: vec3<u32>) {
    let scale = params[1].w;
    let size = params[0].xy * scale;
    if any(id.xy >= size) { return; }
    let out_index = id.y * size.x + id.x;
    if params[1].x != 0u { output[out_index] = 0u; return; }
    let xy = id.xy / scale;
    let row = ((xy.y + params[0].w) & 511u) * 1024u;
    var rgb: vec3<u32>;
    if params[1].y != 0u {
        let byte = params[0].z * 2u + xy.x * 3u;
        for (var channel = 0u; channel < 3u; channel++) {
            let address = byte + channel;
            rgb[channel] = (vram[row + ((address / 2u) & 1023u)] >> ((address & 1u) * 8u)) & 255u;
        }
    } else {
        var index = row + ((params[0].z + xy.x) & 1023u);
        if scale > 1u {
            let sub = id.xy % scale;
            index = sample_base(index, scale) + sub.y * scale + sub.x;
        }
        let pixel = vram[index];
        rgb = vec3<u32>(pixel & 31u, (pixel >> 5u) & 31u, (pixel >> 10u) & 31u);
        rgb = (rgb << vec3<u32>(3u)) | (rgb >> vec3<u32>(2u));
    }
    output[out_index] = rgb.x | (rgb.y << 8u) | (rgb.z << 16u);
}

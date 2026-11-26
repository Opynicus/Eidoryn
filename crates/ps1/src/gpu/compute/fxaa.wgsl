// FXAA 3.11 PC Quality, preset 12, after the PS1 display conversion.
// Algorithm by Timothy Lottes, NVIDIA. See LICENSE-THIRD-PARTY for the notice.
@group(0) @binding(0) var<storage, read> scanout: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@group(0) @binding(2) var<uniform> dimensions: vec4<u32>;

const RELATIVE_THRESHOLD = 0.125;
const ABSOLUTE_THRESHOLD = 0.0625;
const SUBPIXEL_STRENGTH = 0.50;

fn rgb_at(p: vec2<i32>) -> vec3<f32> {
    let xy = vec2<u32>(clamp(p, vec2<i32>(0), vec2<i32>(dimensions.xy) - 1));
    let word = scanout[xy.y * dimensions.x + xy.x];
    return vec3<f32>((vec3<u32>(word) >> vec3<u32>(0u, 8u, 16u)) & vec3<u32>(255u)) / 255.0;
}

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

// Integer coordinates denote pixel centers. Clamp each tap at the CRTC boundary.
fn bilinear(p: vec2<f32>) -> vec3<f32> {
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let top = mix(rgb_at(base), rgb_at(base + vec2<i32>(1, 0)), f.x);
    let bottom = mix(rgb_at(base + vec2<i32>(0, 1)), rgb_at(base + vec2<i32>(1, 1)), f.x);
    return mix(top, bottom, f.y);
}

struct Endpoint {
    distance: f32,
    delta: f32,
}

fn search(start: vec2<f32>, along: vec2<f32>, midpoint: f32, threshold: f32) -> Endpoint {
    // After the first four samples (at 1, 2.5, 4.5 and 8.5 pixels),
    // preset 12 extrapolates another 12 pixels if no endpoint was found.
    let advances = array<f32, 4>(1.5, 2.0, 4.0, 12.0);
    var endpoint = Endpoint(1.0, 0.0);
    for (var i = 0u; i < 4u; i++) {
        endpoint.delta = luma(bilinear(start + along * endpoint.distance)) - midpoint;
        if abs(endpoint.delta) >= threshold { break; }
        endpoint.distance += advances[i];
    }
    return endpoint;
}

@compute @workgroup_size(8, 8)
fn fxaa(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= dimensions.xy) { return; }
    let index = id.y * dimensions.x + id.x;
    let p = vec2<i32>(id.xy);
    let center = luma(rgb_at(p));
    let n = luma(rgb_at(p + vec2<i32>(0, -1)));
    let s = luma(rgb_at(p + vec2<i32>(0, 1)));
    let w = luma(rgb_at(p + vec2<i32>(-1, 0)));
    let e = luma(rgb_at(p + vec2<i32>(1, 0)));
    let high = max(center, max(max(n, s), max(e, w)));
    let low = min(center, min(min(n, s), min(e, w)));
    let contrast = high - low;
    if contrast < max(ABSOLUTE_THRESHOLD, high * RELATIVE_THRESHOLD) {
        output[index] = scanout[index];
        return;
    }

    let nw = luma(rgb_at(p + vec2<i32>(-1, -1)));
    let ne = luma(rgb_at(p + vec2<i32>(1, -1)));
    let sw = luma(rgb_at(p + vec2<i32>(-1, 1)));
    let se = luma(rgb_at(p + vec2<i32>(1, 1)));
    let horizontal = abs(nw + sw - 2.0*w) + 2.0*abs(n + s - 2.0*center) + abs(ne + se - 2.0*e);
    let vertical = abs(nw + ne - 2.0*n) + 2.0*abs(w + e - 2.0*center) + abs(sw + se - 2.0*s);
    let is_horizontal = horizontal >= vertical;
    let along = select(vec2<f32>(0, 1), vec2<f32>(1, 0), is_horizontal);
    let before = select(w, n, is_horizontal);
    let after = select(e, s, is_horizontal);
    let use_before = abs(before - center) >= abs(after - center);
    let adjacent = select(after, before, use_before);
    let normal = along.yx * select(1.0, -1.0, use_before);

    let midpoint = (adjacent + center) * 0.5;
    let threshold = abs(adjacent - center) * 0.25;
    let edge_center = vec2<f32>(p) + 0.5 * normal;
    let a = search(edge_center, -along, midpoint, threshold);
    let b = search(edge_center, along, midpoint, threshold);
    let nearest_delta = select(b.delta, a.delta, a.distance < b.distance);
    // Reject an endpoint on the same luma side as the center pixel.
    let good_span = (nearest_delta < 0.0) != (center < midpoint);
    let edge_offset = select(0.0, 0.5 - min(a.distance, b.distance) / (a.distance + b.distance), good_span);

    let mean = (2.0*(n + s + w + e) + nw + ne + sw + se) / 12.0;
    let subpixel = smoothstep(0.0, 1.0, abs(mean - center) / contrast);
    let offset = max(edge_offset, SUBPIXEL_STRENGTH * subpixel * subpixel);
    let rgb = vec3<u32>(round(bilinear(vec2<f32>(p) + normal * offset) * 255.0));
    output[index] = rgb.r | (rgb.g << 8u) | (rgb.b << 16u);
}

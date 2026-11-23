// LCD scanlines: blend registers, then 240 visible pairs of BGR555 layers.
// Scanlines are captured during emulation so HBlank changes affect the correct row.
@group(0) @binding(0) var<storage, read> scanlines: array<vec2<u32>>;
@group(0) @binding(1) var<storage, read_write> rgb888: array<u32>;

fn channels(color: u32) -> vec3<u32> {
    return (vec3(color) >> vec3(0u, 5u, 10u)) & vec3(31u);
}

fn pixel(layers: vec2<u32>, regs: vec2<u32>) -> u32 {
    var top = channels(layers.x);
    let second = channels(layers.x >> 16u);
    let first_target = (layers.y & regs.x & 63u) != 0u && (layers.y & (32u << 24u)) != 0u;
    let second_target = (layers.y & regs.x & (63u << 8u)) != 0u;
    let semi = (layers.y & (1u << 16u)) != 0u;
    let mode = (regs.x >> 6u) & 3u;
    let evy = (regs.y >> 16u) & 31u;
    if second_target && (semi || (first_target && mode == 1u)) {
        let eva = regs.y & 31u;
        let evb = (regs.y >> 8u) & 31u;
        top = min((top * eva + second * evb) >> vec3(4u), vec3(31u));
    } else if first_target && mode == 2u {
        top += ((vec3(31u) - top) * evy) >> vec3(4u);
    } else if first_target && mode == 3u {
        top -= (top * evy) >> vec3(4u);
    }
    let rgb = (top << vec3(3u)) | (top >> vec3(2u));
    return rgb.r | (rgb.g << 8u) | (rgb.b << 16u);
}

// Four RGB888 pixels share three aligned storage words.
@compute @workgroup_size(8, 8)
fn compose(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= 60u || id.y >= 160u { return; }
    let row = id.y * 241u;
    let first = row + 1u + id.x * 4u;
    let regs = scanlines[row];
    let a = pixel(scanlines[first], regs);
    let b = pixel(scanlines[first + 1u], regs);
    let c = pixel(scanlines[first + 2u], regs);
    let d = pixel(scanlines[first + 3u], regs);
    let out = (id.y * 60u + id.x) * 3u;
    rgb888[out] = a | (b << 24u);
    rgb888[out + 1u] = (b >> 8u) | (c << 16u);
    rgb888[out + 2u] = (c >> 16u) | (d << 8u);
}

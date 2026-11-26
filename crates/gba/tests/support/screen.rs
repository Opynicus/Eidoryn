use eidoryn_gba::{Machine, WIDTH};

/// FNV-1a over an RGB rectangle. Compare static UI elements so guest animation
/// and small differences in the number of instructions per field can vary.
pub fn region_signature(machine: &Machine, x: usize, y: usize, w: usize, h: usize) -> u32 {
    machine
        .pixels()
        .as_chunks::<{ WIDTH * 3 }>()
        .0
        .iter()
        .skip(y)
        .take(h)
        .flat_map(|row| &row[x * 3..(x + w) * 3])
        .fold(0x811c_9dc5, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        })
}

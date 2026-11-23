use crate::{
    Error,
    cpu::{Bus, load_word},
};

/// `HuffUnComp` reads the tree and bitstream from guest memory and writes words.
pub(crate) fn huffman(bus: &mut impl Bus, src: u32, mut dst: u32) -> Result<(u32, u32), Error> {
    let src = src & !3;
    let header = bus.read32(src);
    let width = header & 15;
    if (header as u8 >> 4) != 2 || !matches!(width, 1 | 2 | 4 | 8) {
        return Err(Error::InvalidCompressedData("invalid Huffman header"));
    }
    let table = src.wrapping_add(4);
    let table_len = (u32::from(bus.read8(table)) + 1) * 2;
    let mut stream = table.wrapping_add(table_len);
    let mut bits = 0_u32;
    let mut available = 0;
    for _ in 0..(header >> 8).div_ceil(4) {
        let mut word = 0;
        for shift in (0..32).step_by(width as usize) {
            let mut node = 1;
            loop {
                if available == 0 {
                    // Even-sized tree descriptors can leave the ARM LDR halfword-aligned.
                    bits = load_word(bus, stream);
                    stream = stream.wrapping_add(4);
                    available = 32;
                }
                let branch = bits >> 31;
                bits <<= 1;
                available -= 1;

                let descriptor = bus.read8(table.wrapping_add(node));
                // Children come in pairs relative to the even address of the parent.
                node = (node & !1) + 2 * (u32::from(descriptor & 63) + 1) + branch;
                if node >= table_len {
                    return Err(Error::InvalidCompressedData("Huffman child outside tree"));
                }
                if descriptor & (0x80 >> branch) != 0 {
                    let symbol = u32::from(bus.read8(table.wrapping_add(node)));
                    word |= (symbol & ((1 << width) - 1)) << shift;
                    break;
                }
            }
        }
        bus.write32(dst, word);
        dst = dst.wrapping_add(4);
    }
    Ok((stream, dst))
}

//! GBA BIOS bit unpacking and decompression. The VRAM decompression variants issue
//! halfword writes, including when the caller passes work RAM as the destination.
use crate::{Error, cpu::Bus};

struct Output {
    next: u32,
    remaining: u32,
    halfword: bool,
    low: u8,
}

impl Output {
    fn put(&mut self, bus: &mut impl Bus, byte: u8) {
        if self.halfword {
            if self.next & 1 == 0 {
                self.low = byte;
            } else {
                bus.write16(self.next - 1, u16::from_le_bytes([self.low, byte]));
            }
        } else {
            bus.write8(self.next, byte);
        }
        self.next = self.next.wrapping_add(1);
        self.remaining -= 1;
    }
}

fn take(bus: &impl Bus, src: &mut u32) -> u8 {
    let byte = bus.read8(*src);
    *src = src.wrapping_add(1);
    byte
}

pub(super) fn bit_unpack(
    bus: &mut impl Bus,
    mut src: u32,
    mut dst: u32,
    info: u32,
) -> Result<(u32, u32), Error> {
    let length = bus.read16(info);
    let src_bits = bus.read8(info.wrapping_add(2));
    let dst_bits = bus.read8(info.wrapping_add(3));
    if !matches!(src_bits, 1 | 2 | 4 | 8) || !matches!(dst_bits, 1 | 2 | 4 | 8 | 16 | 32) {
        return Err(Error::InvalidCompressedData("invalid BitUnPack unit width"));
    }
    let flags = bus.read32(info.wrapping_add(4));
    let offset = flags & 0x7fff_ffff;
    let mask = (1u32 << src_bits) - 1;
    let mut out = 0;
    let mut used = 0;
    for _ in 0..length {
        let byte = u32::from(take(bus, &mut src));
        for shift in (0..8).step_by(usize::from(src_bits)) {
            let mut value = (byte >> shift) & mask;
            if value != 0 || flags >> 31 != 0 {
                value += offset;
            }
            out |= value << used;
            used += dst_bits;
            // The BIOS writes only complete words, even when the destination is VRAM.
            if used == 32 {
                bus.write32(dst, out);
                dst = dst.wrapping_add(4);
                out = 0;
                used = 0;
            }
        }
    }
    Ok((src, dst))
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Format {
    Lz77 = 1,
    RunLength = 3,
}

pub(super) fn decode(
    bus: &mut impl Bus,
    format: Format,
    halfword: bool,
    src: u32,
    dst: u32,
) -> Result<(), Error> {
    let mut src = src & !3;
    let header = bus.read32(src);
    let size = header >> 8;
    if (header >> 4) & 15 != format as u32 {
        return Err(Error::InvalidCompressedData(
            "unexpected compression header",
        ));
    }
    let mut out = Output {
        next: if halfword { dst & !1 } else { dst },
        remaining: size,
        halfword,
        low: 0,
    };
    src = src.wrapping_add(4);
    if format == Format::Lz77 {
        lz77(bus, &mut src, &mut out, size)?;
    } else {
        runs(bus, &mut src, &mut out);
        // RLUnComp also zero-pads the destination to the next four-byte length.
        out.remaining = (4 - (size & 3)) & 3;
        while out.remaining > 0 {
            out.put(bus, 0);
        }
    }
    Ok(())
}

fn lz77(bus: &mut impl Bus, src: &mut u32, out: &mut Output, size: u32) -> Result<(), Error> {
    while out.remaining > 0 {
        let flags = take(bus, src);
        for bit in (0..8).rev() {
            if flags & (1 << bit) == 0 {
                let literal = take(bus, src);
                out.put(bus, literal);
            } else {
                let token = u32::from(take(bus, src));
                let distance = (((token & 15) << 8) | u32::from(take(bus, src))) + 1;
                if distance > size - out.remaining {
                    return Err(Error::InvalidCompressedData(
                        "LZ77 reference precedes output",
                    ));
                }
                let count = (3 + (token >> 4)).min(out.remaining);
                for _ in 0..count {
                    // Read the actual destination, so overlaps and pending VRAM
                    // halfwords are observed in the same order as a cartridge.
                    let value = bus.read8(out.next.wrapping_sub(distance));
                    out.put(bus, value);
                }
            }
            if out.remaining == 0 {
                break;
            }
        }
    }
    Ok(())
}

fn runs(bus: &mut impl Bus, src: &mut u32, out: &mut Output) {
    while out.remaining > 0 {
        let flag = take(bus, src);
        let length = u32::from(flag & 127);
        if flag & 128 != 0 {
            let value = take(bus, src);
            for _ in 0..(length + 3).min(out.remaining) {
                out.put(bus, value);
            }
        } else {
            for _ in 0..(length + 1).min(out.remaining) {
                let value = take(bus, src);
                out.put(bus, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Memory;

    const ROM: u32 = 0x0800_0000;
    const WRAM: u32 = 0x0200_0000;
    const VRAM: u32 = 0x0600_0000;

    #[test]
    fn bit_unpack_expands_a_glyph_into_vram_with_optional_transparent_index_bias() {
        use crate::{HEADER_LEN, Machine};
        let mut machine = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        let src = WRAM + 9; // Byte-aligned source; least significant pixel comes first.
        machine.memory.write8(src, 0b1011_0001);
        machine.memory.write16(WRAM, 1);
        machine.memory.write8(WRAM + 2, 1);
        machine.memory.write8(WRAM + 3, 4);
        for (offset, expected) in [(2u32, 0x3033_0003), (0x8000_0002, 0x3233_2223)] {
            machine.memory.write32(WRAM + 4, offset);
            machine.cpu.regs[..3].copy_from_slice(&[src, VRAM, WRAM]);
            machine.bios_call(0x10).unwrap();
            assert_eq!(machine.memory.read32(VRAM), expected);
            assert_eq!(machine.cpu.regs[..2], [src + 1, VRAM + 4]);
        }
    }

    #[test]
    fn bit_unpack_reads_units_low_first_and_leaves_the_last_partial_word_untouched() {
        let mut bus = Memory::new(vec![0; 192]);
        for (src_bits, dst_bits, byte, expected) in [
            (2, 8, 0xe4, 0x0302_0100), // Four two-bit values become palette indices.
            (4, 16, 0xba, 0x000b_000a),
            (8, 32, 0xab, 0x0000_00ab), // A single output element fills a word.
        ] {
            bus.write8(WRAM + 8, byte);
            bus.write16(WRAM, 1);
            bus.write8(WRAM + 2, src_bits);
            bus.write8(WRAM + 3, dst_bits);
            assert_eq!(
                bit_unpack(&mut bus, WRAM + 8, VRAM, WRAM).unwrap(),
                (WRAM + 9, VRAM + 4)
            );
            assert_eq!(bus.read32(VRAM), expected);
        }
        bus.write32(VRAM, 0x1234_5678);
        bus.write8(WRAM + 2, 8);
        bus.write8(WRAM + 3, 16);
        assert_eq!(
            bit_unpack(&mut bus, WRAM + 8, VRAM, WRAM).unwrap(),
            (WRAM + 9, VRAM)
        );
        assert_eq!(bus.read32(VRAM), 0x1234_5678);
        bus.write8(WRAM + 2, 0);
        assert!(matches!(
            bit_unpack(&mut bus, WRAM + 8, VRAM, WRAM),
            Err(Error::InvalidCompressedData(_))
        ));
    }

    #[test]
    fn lz77_overlapping_reference_repeats_prior_output() {
        // Two literals and a six-byte match with a two-byte distance.
        let mut bus = Memory::new(vec![0x10, 8, 0, 0, 0x20, b'A', b'B', 0x30, 1]);
        for (halfword, dst) in [(false, WRAM), (true, VRAM)] {
            decode(&mut bus, Format::Lz77, halfword, ROM, dst).unwrap();
            assert_eq!(bus.read32(dst).to_le_bytes(), *b"ABAB");
            assert_eq!(bus.read32(dst + 4).to_le_bytes(), *b"ABAB");
        }
    }

    #[test]
    fn rle_literal_and_repeated_bytes_flush_across_halfword_boundaries() {
        let mut bus = Memory::new(vec![0x30, 7, 0, 0, 0, 1, 0x80, 2, 2, 3, 4, 5]);
        for (halfword, dst) in [(false, WRAM), (true, VRAM)] {
            bus.write32(dst + 4, u32::MAX);
            decode(&mut bus, Format::RunLength, halfword, ROM, dst).unwrap();
            assert_eq!(bus.read32(dst).to_le_bytes(), [1, 2, 2, 2]);
            assert_eq!(bus.read32(dst + 4).to_le_bytes(), [3, 4, 5, 0]);
        }
    }

    #[test]
    fn lz77_rejects_reference_before_first_literal() {
        let mut bus = Memory::new(vec![0x10, 6, 0, 0, 0x80, 0x30, 0]);
        assert!(matches!(
            decode(&mut bus, Format::Lz77, false, ROM, WRAM),
            Err(Error::InvalidCompressedData(_))
        ));
    }
}

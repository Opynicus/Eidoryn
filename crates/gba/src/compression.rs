//! GBA BIOS LZ77 and run-length decompression. The VRAM variants issue
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

pub(crate) fn decode(bus: &mut impl Bus, code: u8, src: u32, dst: u32) -> Result<(), Error> {
    let (lz, halfword) = match code {
        0x11 => (true, false),
        0x12 => (true, true),
        0x14 => (false, false),
        0x15 => (false, true),
        _ => return Err(Error::UnsupportedBiosCall(code)),
    };
    let mut src = src & !3;
    let header = bus.read32(src);
    let size = header >> 8;
    if (header >> 4) & 15 != if lz { 1 } else { 3 } {
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
    if lz {
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
    fn lz77_overlapping_reference_repeats_prior_output() {
        // Two literals and a six-byte match with a two-byte distance.
        let mut bus = Memory::new(vec![0x10, 8, 0, 0, 0x20, b'A', b'B', 0x30, 1]);
        for (swi, dst) in [(0x11, WRAM), (0x12, VRAM)] {
            decode(&mut bus, swi, ROM, dst).unwrap();
            assert_eq!(bus.read32(dst).to_le_bytes(), *b"ABAB");
            assert_eq!(bus.read32(dst + 4).to_le_bytes(), *b"ABAB");
        }
    }

    #[test]
    fn rle_literal_and_repeated_bytes_flush_across_halfword_boundaries() {
        let mut bus = Memory::new(vec![0x30, 7, 0, 0, 0, 1, 0x80, 2, 2, 3, 4, 5]);
        for (swi, dst) in [(0x14, WRAM), (0x15, VRAM)] {
            bus.write32(dst + 4, u32::MAX);
            decode(&mut bus, swi, ROM, dst).unwrap();
            assert_eq!(bus.read32(dst).to_le_bytes(), [1, 2, 2, 2]);
            assert_eq!(bus.read32(dst + 4).to_le_bytes(), [3, 4, 5, 0]);
        }
    }

    #[test]
    fn lz77_rejects_reference_before_first_literal() {
        let mut bus = Memory::new(vec![0x10, 6, 0, 0, 0x80, 0x30, 0]);
        assert!(matches!(
            decode(&mut bus, 0x11, ROM, WRAM),
            Err(Error::InvalidCompressedData(_))
        ));
    }
}

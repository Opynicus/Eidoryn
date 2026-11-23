//! Fixed-point affine matrix services used by the GBA BIOS.
use crate::{Bus, Machine};

// First quadrant of trunc(2^14 * sin(2π * i / 256)). The BIOS angle uses
// eight significant bits; reflecting this quadrant also preserves its rounding.
const SIN_Q14: [i16; 65] = [
    0, 402, 803, 1_205, 1_605, 2_005, 2_404, 2_801, 3_196, 3_589, 3_980, 4_369, 4_756, 5_139,
    5_519, 5_896, 6_269, 6_639, 7_005, 7_366, 7_723, 8_075, 8_423, 8_765, 9_102, 9_434, 9_759,
    10_079, 10_393, 10_701, 11_002, 11_297, 11_585, 11_866, 12_139, 12_406, 12_665, 12_916, 13_159,
    13_395, 13_622, 13_842, 14_053, 14_255, 14_449, 14_634, 14_810, 14_978, 15_136, 15_286, 15_426,
    15_557, 15_678, 15_790, 15_892, 15_985, 16_069, 16_142, 16_206, 16_260, 16_305, 16_339, 16_364,
    16_379, 16_384,
];

fn sine(angle: u8) -> i32 {
    let quadrant = angle >> 6;
    let phase = usize::from(angle & 63);
    let index = if quadrant & 1 == 0 { phase } else { 64 - phase };
    let value = i32::from(SIN_Q14[index]);
    if quadrant < 2 { value } else { -value }
}

fn matrix(bus: &impl Bus, scale: u32) -> [i32; 4] {
    let x = i32::from(bus.read16(scale) as i16);
    let y = i32::from(bus.read16(scale.wrapping_add(2)) as i16);
    let angle = (bus.read16(scale.wrapping_add(4)) >> 8) as u8;
    let sin = sine(angle);
    let cos = sine(angle.wrapping_add(64));
    // PB negates the *shifted* product, which matters for fractional and negative scales.
    [
        (x * cos) >> 14,
        -((x * sin) >> 14),
        (y * sin) >> 14,
        (y * cos) >> 14,
    ]
}

impl Machine {
    pub(super) fn bg_affine_set(&mut self) {
        let [mut src, mut dst, count, ..] = self.cpu.regs;
        for _ in 0..(count as i32) {
            let [pa, pb, pc, pd] = matrix(&self.memory, src.wrapping_add(12));
            let ox = self.memory.read32(src);
            let oy = self.memory.read32(src.wrapping_add(4));
            let x = i32::from(self.memory.read16(src.wrapping_add(8)) as i16);
            let y = i32::from(self.memory.read16(src.wrapping_add(10)) as i16);
            let ref_x = ox
                .wrapping_sub((pa * x) as u32)
                .wrapping_sub((pb * y) as u32);
            let ref_y = oy
                .wrapping_sub((pc * x) as u32)
                .wrapping_sub((pd * y) as u32);
            self.memory.write32(dst.wrapping_add(8), ref_x);
            self.memory.write32(dst.wrapping_add(12), ref_y);
            for coefficient in [pa, pb, pc, pd] {
                self.memory.write16(dst, coefficient as u16);
                dst = dst.wrapping_add(2);
            }
            dst = dst.wrapping_add(8);
            src = src.wrapping_add(20);
            self.cpu.regs[3] = pa as u32;
        }
        self.cpu.regs[0] = src;
        self.cpu.regs[1] = dst;
    }

    pub(super) fn obj_affine_set(&mut self) {
        let [mut src, mut dst, count, stride, ..] = self.cpu.regs;
        for _ in 0..(count as i32) {
            for coefficient in matrix(&self.memory, src) {
                self.memory.write16(dst, coefficient as u16);
                dst = dst.wrapping_add(stride);
            }
            src = src.wrapping_add(8);
        }
        self.cpu.regs[0] = src;
        self.cpu.regs[1] = dst;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HEADER_LEN;

    #[test]
    fn background_batch_translates_the_signed_pivot_and_advances_each_record() {
        let mut m = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        let src = 0x0300_0000;
        let dst = 0x0300_0100;
        m.memory.write32(src, u32::MAX - 7);
        m.memory.write32(src + 4, 3000);
        m.memory.write16(src + 8, (-2_i16) as u16);
        m.memory.write16(src + 10, 3);
        m.memory.write16(src + 12, 0x100);
        m.memory.write16(src + 14, (-128_i16) as u16);
        m.memory.write16(src + 16, 0x40ff);
        m.memory.write16(src + 20 + 12, 0x180);
        m.memory.write16(src + 20 + 14, 0x200);
        m.cpu.regs[..3].copy_from_slice(&[src, dst, 2]);
        m.bios_call(0x0e).unwrap();
        let coefficients = |p| [0, 2, 4, 6].map(|i| m.memory.read16(p + i) as i16);
        assert_eq!(coefficients(dst), [0, -256, -128, 0]);
        assert_eq!(
            [m.memory.read32(dst + 8), m.memory.read32(dst + 12)],
            [760, 2744]
        );
        assert_eq!(coefficients(dst + 16), [384, 0, 0, 512]);
        assert_eq!(m.cpu.regs[..4], [src + 40, dst + 32, 2, 384]);
    }

    #[test]
    fn sprite_stride_leaves_surrounding_attributes_untouched() {
        let mut m = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        let src = 0x0300_0000;
        let dst = 0x0700_0006;
        m.memory.write16(src, (-256_i16) as u16);
        m.memory.write16(src + 2, 256);
        m.memory.write16(src + 4, 0x0200);
        // The second 8-byte record uses a quarter turn.
        m.memory.write16(src + 8, 256);
        m.memory.write16(src + 10, 256);
        m.memory.write16(src + 12, 0x4000);
        for offset in (0..64).step_by(2) {
            m.memory.write16(0x0700_0000 + offset, 0xa55a);
        }
        m.cpu.regs[..4].copy_from_slice(&[src, dst, 2, 8]);
        m.bios_call(0x0f).unwrap();
        let expected = [-256_i16, 13, 12, 255, 0, -256, 256, 0];
        for (group, coefficient) in (0..8).zip(expected) {
            let attr = 0x0700_0000 + group * 8;
            assert_eq!(
                [0, 2, 4].map(|offset| m.memory.read16(attr + offset)),
                [0xa55a; 3]
            );
            assert_eq!(m.memory.read16(attr + 6) as i16, coefficient);
        }
        assert_eq!(m.cpu.regs[..4], [src + 16, dst + 64, 2, 8]);
    }
}

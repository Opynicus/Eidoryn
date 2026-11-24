//! Integer trigonometry with the precision and wrapping of the BIOS math routines.
use crate::Machine;

impl Machine {
    pub(super) fn arc_tan(&mut self) {
        let slope = self.cpu.regs[0] as i32;
        let square = -(slope.wrapping_mul(slope) >> 14);
        let polynomial = [0x390, 0x91c, 0xfb6, 0x16aa, 0x2081, 0x3651, 0xa2f9]
            .into_iter()
            .fold(0xa9_i32, |acc, c| (acc.wrapping_mul(square) >> 14) + c);
        self.cpu.regs[0] = (polynomial.wrapping_mul(slope) >> 16) as u32;
        self.cpu.regs[1] = square as u32;
        self.cpu.regs[3] = polynomial as u32;
    }

    pub(super) fn arc_tan2(&mut self) {
        let x = self.cpu.regs[0] as i32;
        let y = self.cpu.regs[1] as i32;
        let angle = if y == 0 {
            if x < 0 { 0x8000 } else { 0 }
        } else if x == 0 {
            if y < 0 { 0xc000 } else { 0x4000 }
        } else {
            // Division in the chosen octant keeps the signed Q14 slope within
            // [-1,1] for the signed 16-bit Cartesian input range.
            let horizontal = x.unsigned_abs() > y.unsigned_abs()
                || (x.unsigned_abs() == y.unsigned_abs() && (x > 0 || y > 0));
            let (numerator, denominator) = if horizontal { (y, x) } else { (x, y) };
            self.cpu.regs[0] = (numerator << 14).wrapping_div(denominator) as u32;
            self.arc_tan();
            let arc = self.cpu.regs[0];
            if horizontal {
                let base: u32 = if x < 0 {
                    0x8000
                } else if y < 0 {
                    0x1_0000
                } else {
                    0
                };
                base.wrapping_add(arc)
            } else {
                let base: u32 = if y > 0 { 0x4000 } else { 0xc000 };
                base.wrapping_sub(arc)
            }
        };
        self.cpu.regs[0] = angle;
        // The Thumb wrapper pops its BIOS return address into r3.
        self.cpu.regs[3] = 0x170;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HEADER_LEN;

    #[test]
    fn arctan_polynomial_preserves_the_signed_rounding_and_scratch_registers() {
        let mut m = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        for (x, angle) in [(0x1000_i32, 0x9fb_i32), (-0x1000, -0x9fc), (0x4000, 0x2000)] {
            m.cpu.regs[0] = x as u32;
            m.bios_call(0x09).unwrap();
            assert_eq!(m.cpu.regs[0] as i32, angle);
            if x == 0x1000 {
                assert_eq!((m.cpu.regs[1], m.cpu.regs[3]), (0xffff_fc00, 0x9fb3));
            }
        }
    }

    #[test]
    fn arctan2_axes_octants_and_origin() {
        let mut m = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        for (x, y, expected) in [
            (0_i32, 0_i32, 0),
            (1, 0, 0),
            (0, 1, 0x4000),
            (-1, 0, 0x8000),
            (0, -1, 0xc000),
            (4, 1, 0x09fb),
            (1, 4, 0x3605),
            (-1, 4, 0x49fc),
            (-4, 1, 0x7604),
            (-4, -1, 0x89fb),
            (-1, -4, 0xb605),
            (1, -4, 0xc9fc),
            (4, -1, 0xf604),
        ] {
            m.cpu.regs[..3].copy_from_slice(&[x as u32, y as u32, 77]);
            m.bios_call(0x0a).unwrap();
            assert_eq!(
                (m.cpu.regs[0], m.cpu.regs[2], m.cpu.regs[3]),
                (expected, 77, 0x170)
            );
        }
    }
}

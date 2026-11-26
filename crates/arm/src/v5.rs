//! `ARMv5TE` instructions added by the ARM946E-S.
use super::{Bus, Cpu, LR, PC, arm::reg_index};
const Q: u32 = 1 << 27;

impl Cpu {
    pub(super) fn v5_extension(&mut self, bus: &mut impl Bus, op: u32, pc: u32) -> bool {
        let rd = reg_index(op, 12);
        let rm = self.regs[reg_index(op, 0)];
        let rn = reg_index(op, 16);
        if op & 0x0fff_fff0 == 0x012f_ff30 {
            self.regs[LR] = pc.wrapping_add(4);
            self.exchange(rm);
        } else if op & 0x0fff_0ff0 == 0x016f_0f10 {
            self.regs[rd] = rm.leading_zeros();
        } else if op & 0x0f00_0f10 == 0x0e00_0f10 {
            let crm = (op & 15) as u8;
            let opc = ((op >> 5) & 7) as u8;
            if op & (1 << 20) == 0 {
                bus.cp15_write(rn as u8, crm, opc, self.reg(rd, pc.wrapping_add(8)));
            } else {
                let value = bus.cp15_read(rn as u8, crm, opc);
                if rd == PC {
                    self.cpsr = (self.cpsr & 0x0fff_ffff) | (value & 0xf000_0000);
                } else {
                    self.regs[rd] = value;
                }
            }
        } else if op & 0x0f90_0ff0 == 0x0100_0050 {
            let x = i64::from(rm as i32);
            let mut y = i64::from(self.regs[rn] as i32);
            if op & (1 << 22) != 0 {
                y = i64::from(self.saturate(y * 2));
            }
            let result = if op & (1 << 21) == 0 { x + y } else { x - y };
            self.regs[rd] = self.saturate(result) as u32;
        } else if op & 0x0f90_0090 == 0x0100_0080 {
            let half =
                |value: u32, high| i32::from((if high { value >> 16 } else { value }) as i16);
            let rs = half(self.regs[reg_index(op, 8)], op & 64 != 0);
            let product = half(rm, op & 32 != 0) * rs;
            match (op >> 21) & 3 {
                0 => self.regs[rn] = self.mac(product, self.regs[rd]),
                1 => {
                    let word = ((i64::from(rm as i32) * i64::from(rs)) >> 16) as i32;
                    self.regs[rn] = if op & 32 == 0 {
                        self.mac(word, self.regs[rd])
                    } else {
                        word as u32
                    };
                }
                2 => {
                    let acc = (u64::from(self.regs[rn]) << 32) | u64::from(self.regs[rd]);
                    let total = acc.wrapping_add_signed(i64::from(product));
                    self.regs[rn] = (total >> 32) as u32;
                    self.regs[rd] = total as u32;
                }
                _ => self.regs[rn] = product as u32,
            }
        } else {
            return false;
        }
        true
    }

    fn mac(&mut self, product: i32, add: u32) -> u32 {
        let (sum, overflow) = product.overflowing_add(add as i32);
        if overflow {
            self.cpsr |= Q;
        }
        sum as u32
    }
    fn saturate(&mut self, value: i64) -> i32 {
        if let Ok(value) = i32::try_from(value) {
            return value;
        }
        self.cpsr |= Q;
        if value < 0 { i32::MIN } else { i32::MAX }
    }
}

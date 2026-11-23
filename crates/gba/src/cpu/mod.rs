//! ARM7TDMI execution. R15 holds the next fetch address between instructions.
mod arm;
mod thumb;

#[cfg(test)]
mod tests;

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const I: u32 = 1 << 7;
const F: u32 = 1 << 6;
const T: u32 = 1 << 5;
const MODE: u32 = 0x1f;
const IRQ: u32 = 0x12;
const SVC: u32 = 0x13;
const SP: usize = 13;
const LR: usize = 14;
const PC: usize = 15;
// Return address in the BIOS reserved range, intercepted by the BIOS-less IRQ epilogue.
const IRQ_RETURN: u32 = 0x0000_0138;

pub(crate) trait Bus {
    /// Set the code region used by BIOS access protection, including its two-stage prefetch.
    fn begin_instruction(&mut self, _pc: u32, _width: u32) {}
    fn read8(&self, addr: u32) -> u8;
    fn write8(&mut self, addr: u32, value: u8);
    fn read16(&self, addr: u32) -> u16 {
        let addr = addr & !1;
        u16::from_le_bytes([self.read8(addr), self.read8(addr + 1)])
    }
    fn read32(&self, addr: u32) -> u32 {
        let addr = addr & !3;
        u32::from(self.read16(addr)) | (u32::from(self.read16(addr + 2)) << 16)
    }
    fn write16(&mut self, addr: u32, value: u16) {
        let addr = addr & !1;
        let [lo, hi] = value.to_le_bytes();
        self.write8(addr, lo);
        self.write8(addr + 1, hi);
    }
    fn write32(&mut self, addr: u32, value: u32) {
        let addr = addr & !3;
        self.write16(addr, value as u16);
        self.write16(addr + 2, (value >> 16) as u16);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Step {
    Run,
    Swi(u8),
}

pub(crate) struct Cpu {
    pub(crate) regs: [u32; 16],
    cpsr: u32,
    // User/System, FIQ, IRQ, Supervisor, Abort and Undefined modes.
    bank_sp_lr: [[u32; 2]; 6],
    other_fiq_regs: [u32; 5],
    spsr: [u32; 6],
    irq_depth: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        let mut cpu = Self::power_on();
        cpu.regs[SP] = 0x0300_7f00;
        cpu.regs[PC] = 0x0800_0000;
        cpu.bank_sp_lr[2][0] = 0x0300_7fa0;
        cpu.bank_sp_lr[3][0] = 0x0300_7fe0;
        cpu.cpsr = MODE;
        cpu
    }
}

impl Cpu {
    pub(crate) fn power_on() -> Self {
        Self {
            regs: [0; 16],
            cpsr: SVC | I | F,
            bank_sp_lr: [[0; 2]; 6],
            other_fiq_regs: [0; 5],
            spsr: [0; 6],
            irq_depth: 0,
        }
    }
    /// A decoded SWI leaves R15 just past its encoding and preserves the caller's T bit.
    pub(crate) fn swi_location(&self) -> (u32, crate::InstructionSet) {
        let next = self.regs[PC];
        if self.cpsr & T == 0 {
            (next.wrapping_sub(4), crate::InstructionSet::Arm)
        } else {
            (next.wrapping_sub(2), crate::InstructionSet::Thumb)
        }
    }

    pub(crate) fn step(&mut self, bus: &mut impl Bus) -> Result<Step, String> {
        let pc = self.regs[PC];
        if pc == IRQ_RETURN && self.in_irq() && self.cpsr & MODE == IRQ {
            let mut addr = self.regs[SP];
            for reg in [0, 1, 2, 3, 12, LR] {
                self.regs[reg] = bus.read32(addr);
                addr = addr.wrapping_add(4);
            }
            self.regs[SP] = addr;
            let resume = self.regs[LR].wrapping_sub(4);
            self.restore_status()?;
            self.irq_depth -= 1;
            self.branch(resume);
            return Ok(Step::Run);
        }
        if self.cpsr & T == 0 {
            bus.begin_instruction(pc, 4);
            self.regs[PC] = pc.wrapping_add(4);
            self.arm(bus, pc, bus.read32(pc))
        } else {
            bus.begin_instruction(pc, 2);
            self.regs[PC] = pc.wrapping_add(2);
            self.thumb(bus, pc, u32::from(bus.read16(pc)))
        }
    }

    fn enter_exception(&mut self, mode: u32, vector: u32, resume: u32) {
        let previous = self.cpsr;
        self.set_status((previous & !(MODE | T)) | mode | I);
        self.spsr[Self::bank(mode)] = previous;
        self.regs[LR] = resume;
        self.regs[PC] = vector;
    }

    pub(crate) fn software_interrupt(&mut self) {
        // SWI is synchronous. PC already denotes the instruction following the SWI.
        self.enter_exception(SVC, 0x08, self.regs[PC]);
    }

    pub(crate) fn hardware_irq(&mut self) {
        if self.cpsr & I == 0 {
            self.enter_exception(IRQ, 0x18, self.regs[PC].wrapping_add(4));
        }
    }

    /// Enter the IRQ dispatcher normally supplied by the BIOS.
    pub(crate) fn service_irq(&mut self, bus: &mut impl Bus) {
        if self.cpsr & I != 0 {
            return;
        }
        let handler = bus.read32(0x0300_7ffc);
        if handler == 0 {
            return;
        }
        self.hardware_irq();
        self.regs[SP] = self.regs[SP].wrapping_sub(24);
        let mut addr = self.regs[SP];
        for reg in [0, 1, 2, 3, 12, LR] {
            bus.write32(addr, self.regs[reg]);
            addr = addr.wrapping_add(4);
        }
        self.regs[0] = 0x0400_0000;
        self.regs[LR] = IRQ_RETURN;
        self.irq_depth += 1;
        self.exchange(handler);
    }

    pub(crate) fn in_irq(&self) -> bool {
        self.irq_depth != 0
    }

    fn bank(mode: u32) -> usize {
        match mode & MODE {
            0x11 => 1,
            IRQ => 2,
            0x13 => 3,
            0x17 => 4,
            0x1b => 5,
            _ => 0,
        }
    }

    fn set_status(&mut self, value: u32) {
        let old = Self::bank(self.cpsr);
        let new = Self::bank(value);
        if old != new {
            self.bank_sp_lr[old] = [self.regs[SP], self.regs[LR]];
            [self.regs[SP], self.regs[LR]] = self.bank_sp_lr[new];
            if old == 1 || new == 1 {
                self.regs[8..13].swap_with_slice(&mut self.other_fiq_regs);
            }
        }
        self.cpsr = value;
    }

    fn restore_status(&mut self) -> Result<(), String> {
        let bank = Self::bank(self.cpsr);
        if bank == 0 {
            return Err("attempt to restore SPSR outside exception mode".into());
        }
        self.set_status(self.spsr[bank]);
        Ok(())
    }

    fn branch(&mut self, addr: u32) {
        self.regs[PC] = addr & if self.cpsr & T == 0 { !3 } else { !1 };
    }

    fn exchange(&mut self, addr: u32) {
        self.cpsr = (self.cpsr & !T) | ((addr & 1) << 5);
        self.branch(addr);
    }

    fn reg(&self, index: usize, visible_pc: u32) -> u32 {
        if index == PC {
            visible_pc
        } else {
            self.regs[index]
        }
    }

    fn assign(&mut self, index: usize, value: u32) {
        if index == PC {
            self.branch(value);
        } else {
            self.regs[index] = value;
        }
    }

    fn condition(&self, cond: u32) -> bool {
        let n = self.cpsr & N != 0;
        let z = self.cpsr & Z != 0;
        let c = self.cpsr & C != 0;
        let v = self.cpsr & V != 0;
        let pass = match cond >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => !z && n == v,
            _ => true,
        };
        pass != (cond & 1 != 0)
    }

    fn set_nz(&mut self, val: u32) {
        self.cpsr = (self.cpsr & !(N | Z)) | (val & N) | if val == 0 { Z } else { 0 };
    }

    fn logical(&mut self, val: u32, carry: bool) -> u32 {
        self.set_nz(val);
        self.cpsr = (self.cpsr & !C) | if carry { C } else { 0 };
        val
    }

    fn add(&mut self, x: u32, y: u32, carry: bool, flags: bool) -> u32 {
        let sum = u64::from(x) + u64::from(y) + u64::from(carry);
        let value = sum as u32;
        if flags {
            let overflow = (!(x ^ y) & (x ^ value) & N) != 0;
            self.logical(value, sum >> 32 != 0);
            self.cpsr = (self.cpsr & !V) | if overflow { V } else { 0 };
        }
        value
    }

    fn shift(&self, value: u32, kind: u32, amount: u32, immediate: bool) -> (u32, bool) {
        let old_c = self.cpsr & C != 0;
        if amount == 0 && (!immediate || kind == 0) {
            return (value, old_c);
        }
        if amount == 0 && kind == 3 {
            return ((value >> 1) | (u32::from(old_c) << 31), value & 1 != 0);
        }
        let amount = if amount == 0 { 32 } else { amount };
        match kind {
            0 => (
                value.checked_shl(amount).unwrap_or(0),
                amount <= 32 && value & (1 << (32 - amount)) != 0,
            ),
            1 => (
                value.checked_shr(amount).unwrap_or(0),
                amount <= 32 && value & (1 << (amount - 1)) != 0,
            ),
            2 => (
                ((value as i32) >> amount.min(31)) as u32,
                value & (1 << (amount.min(32) - 1)) != 0,
            ),
            _ => {
                let result = value.rotate_right(amount);
                (result, result & N != 0)
            }
        }
    }
}

fn load_word(bus: &impl Bus, addr: u32) -> u32 {
    bus.read32(addr & !3).rotate_right((addr & 3) * 8)
}

fn load_half(bus: &impl Bus, addr: u32) -> u32 {
    u32::from(bus.read16(addr & !1)).rotate_right((addr & 1) * 8)
}

fn load_signed_byte(bus: &impl Bus, addr: u32) -> u32 {
    i32::from(bus.read8(addr) as i8) as u32
}

fn load_signed_half(bus: &impl Bus, addr: u32) -> u32 {
    if addr & 1 == 0 {
        i32::from(bus.read16(addr) as i16) as u32
    } else {
        load_signed_byte(bus, addr)
    }
}

fn sign_extend(value: u32, bits: u32) -> u32 {
    (((value << (32 - bits)) as i32) >> (32 - bits)) as u32
}

//! Guest pipeline bookkeeping around native, non-trapping integer blocks.
use super::Cpu;
use crate::{EmuError, bus::Bus, cpu_engine::Compiled};

impl Cpu {
    pub(crate) fn native_ready(&self) -> bool {
        self.load.is_none()
            && !self.branch_pending
            && self.pc & 3 == 0
            && self.next_pc == self.pc.wrapping_add(4)
            && self.registers[0] == 0
    }

    pub(crate) fn run_native(
        &mut self,
        bus: &mut Bus,
        block: &Compiled,
        limit: usize,
    ) -> Result<(usize, bool), EmuError> {
        let limit = limit.min(block.words.len());
        let Self {
            registers,
            pc,
            next_pc,
            instruction_pc,
            in_delay_slot,
            instructions,
            cop0,
            ..
        } = self;
        let mut failure = None;
        let mut changed = false;
        let completed = block.code.run(registers, limit, |completed| {
            if completed != 0 {
                // Native ALU operations cannot trap or change control flow.
                // Complete precisely one guest step before advancing devices.
                *instruction_pc = *pc;
                *pc = *next_pc;
                *next_pc = next_pc.wrapping_add(4);
                *in_delay_slot = false;
                *instructions += 1;
                if let Err(error) = bus.tick(1) {
                    failure = Some(error);
                    return false;
                }
            }
            if completed == limit {
                return false;
            }
            // DMA can modify code during the tick above. Check every opcode,
            // including aliases and writes made through direct debug RAM access.
            if bus.peek32(*pc) != Some(block.words[completed]) {
                changed = true;
                return false;
            }
            let cause = (cop0[13] & !(1 << 10)) | (u32::from(bus.irq_pending()) << 10);
            if cop0[12] & 1 != 0 && cop0[12] & cause & 0xff00 != 0 {
                // The interpreter takes the exception at this same boundary.
                // Do not advance the PC, clock or delayed pipeline state here.
                return false;
            }
            cop0[13] = cause;
            true
        });
        failure.map_or(Ok((completed, changed)), Err)
    }
}

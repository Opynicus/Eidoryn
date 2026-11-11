//! Instruction-level CPU timing approximations in the shared device-clock domain.
//! Reference: https://psx-spx.consoledev.net/dev/ps1/cpu/cpuspecifications/#load-timing
//! and https://psx-spx.consoledev.net/dev/ps1/cpu/cpuspecifications/#multiplydivide
use super::Cpu;
use crate::bus::physical;

impl Cpu {
    /// Called only after a successfully executed instruction. The interpreter
    /// models each instruction's effects atomically; devices advance afterward.
    pub(super) fn instruction_cycles(&mut self, instruction: u32, source: u32, now: u64) -> u32 {
        let opcode = instruction >> 26;
        if opcode == 0 {
            match instruction & 63 {
                0x18 | 0x19 => {
                    // Signed multiplication classifies the sign-extended source.
                    let magnitude = if instruction & 63 == 0x18 && (source as i32) < 0 {
                        !source
                    } else {
                        source
                    };
                    let latency = if magnitude <= 0x7ff {
                        6
                    } else if magnitude <= 0x000f_ffff {
                        9
                    } else {
                        13
                    };
                    // The initiating instruction itself takes one cycle; its
                    // asynchronous execution then overlaps following steps.
                    self.muldiv_ready = Some(now.wrapping_add(1 + latency));
                }
                0x1a | 0x1b => self.muldiv_ready = Some(now.wrapping_add(1 + 36)),
                0x10 | 0x12 => {
                    let remaining = self.muldiv_ready.map_or(0, |ready| ready.wrapping_sub(now));
                    // Deadlines are at most 36 cycles ahead. Larger wrapping
                    // differences represent deadlines already in the past.
                    if remaining <= 36 {
                        return 1 + remaining as u32;
                    }
                }
                // Writes cancel the previous operation's wait. Partial HI/LO
                // values during a cancelled operation are not modeled yet.
                0x11 | 0x13 => self.muldiv_ready = None,
                _ => {}
            }
        }
        if matches!(opcode, 0x20..=0x26 | 0x32) {
            let address = physical(source.wrapping_add(i32::from(instruction as i16) as u32));
            return match address {
                // Main RAM reads take seven cycles including instruction issue.
                0x0000_0000..=0x007f_ffff => 7,
                // Interrupt, DMA, and timer registers use the on-die decoder.
                0x1f80_1070..=0x1f80_1077
                | 0x1f80_1080..=0x1f80_10f7
                | 0x1f80_1100..=0x1f80_112f => 5,
                // Scratchpad and unmodeled bus timings retain one issue cycle.
                _ => 1,
            };
        }
        1
    }
}

//! Optional native integer blocks. The interpreter remains the reference and
//! executes memory, branch, coprocessor, overflow and delay-slot instructions.
use crate::{EmuError, bus::Bus, cpu::Cpu};
use std::{io, str::FromStr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CpuMode {
    #[default]
    Interpreter,
    Jit,
}
impl CpuMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Interpreter => "Interpreter",
            Self::Jit => "JIT (experimental)",
        }
    }
}
impl FromStr for CpuMode {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "interpreter" => Ok(Self::Interpreter),
            "jit" => Ok(Self::Jit),
            _ => Err("--cpu expects interpreter or jit"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuStats {
    pub native_instructions: u64,
    pub compiled_blocks: u64,
    pub code_invalidations: u64,
}

#[derive(Clone, Default)]
pub(crate) struct Execution {
    mode: CpuMode,
    #[cfg(feature = "jit")]
    cache: Option<Box<native::Cache>>,
}

impl Execution {
    pub(crate) fn mode(&self) -> CpuMode {
        self.mode
    }
    pub(crate) fn set_mode(&mut self, mode: CpuMode) -> io::Result<()> {
        if mode == self.mode {
            return Ok(());
        }
        #[cfg(feature = "jit")]
        {
            let cache = if mode == CpuMode::Jit {
                Some(Box::new(native::Cache::new()?))
            } else {
                None
            };
            self.cache = cache;
            self.mode = mode;
            Ok(())
        }
        #[cfg(not(feature = "jit"))]
        {
            Err(io::Error::other(
                "JIT support is not compiled in; build with --features jit",
            ))
        }
    }
    pub(crate) fn stats(&self) -> CpuStats {
        #[cfg(feature = "jit")]
        if let Some(cache) = &self.cache {
            return cache.stats;
        }
        CpuStats::default()
    }
    pub(crate) fn try_run(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut Bus,
        limit: usize,
    ) -> Result<usize, EmuError> {
        #[cfg(feature = "jit")]
        if let Some(cache) = &mut self.cache {
            return cache.run(cpu, bus, limit);
        }
        let _ = (cpu, bus, limit);
        Ok(0)
    }
}

#[cfg(feature = "jit")]
mod native {
    use super::{Bus, Cpu, CpuStats, EmuError, io};
    use eidoryn_jit_runtime::{Binary, Block, Immediate, MAX_OPERATIONS, Operation, Shift};
    use std::sync::Arc;

    const CACHE_ENTRIES: usize = 4096;
    const HOT_THRESHOLD: u8 = 8;

    pub(crate) struct Compiled {
        pub code: Block,
        pub words: Vec<u32>,
    }

    #[derive(Clone, Default)]
    struct Entry {
        pc: u32,
        first: u32,
        hits: u8,
        interpreted_steps: usize,
        block: Option<Arc<Compiled>>,
    }

    #[derive(Clone)]
    pub(super) struct Cache {
        entries: Box<[Entry]>,
        pub stats: CpuStats,
    }

    impl Cache {
        pub(super) fn new() -> io::Result<Self> {
            // Check executable-memory support before selecting the mode. No
            // guest state is touched if the architecture or OS rejects it.
            let probe = Block::compile(&[Operation::Immediate {
                kind: Immediate::Constant,
                destination: 1,
                source: 0,
                value: 7,
            }])?;
            let mut registers = [0; 32];
            if probe.run(&mut registers, 1, |_| true) != 1 || registers[1] != 7 {
                return Err(io::Error::other("Native JIT self-check failed"));
            }
            Ok(Self {
                entries: vec![Entry::default(); CACHE_ENTRIES].into_boxed_slice(),
                stats: CpuStats::default(),
            })
        }

        pub(super) fn run(
            &mut self,
            cpu: &mut Cpu,
            bus: &mut Bus,
            limit: usize,
        ) -> Result<usize, EmuError> {
            if limit == 0 || !cpu.native_ready() {
                return Ok(0);
            }
            // peek32 cannot perform MMIO reads. Never speculatively fetch from
            // a device: those instructions remain in the interpreter.
            let Some(first) = bus.peek32(cpu.pc) else {
                return Ok(0);
            };
            // Fold upper PC bits so functions spaced 16 KiB apart do not
            // continually evict each other, including hot BIOS/game loops.
            let index = ((cpu.pc >> 2) ^ (cpu.pc >> 14)) as usize & (CACHE_ENTRIES - 1);
            let entry = &mut self.entries[index];
            if entry.hits == 0 || entry.pc != cpu.pc || entry.first != first {
                if entry.block.is_some() && entry.pc == cpu.pc {
                    self.stats.code_invalidations += 1;
                }
                *entry = Entry {
                    pc: cpu.pc,
                    first,
                    hits: 1,
                    ..Entry::default()
                };
            } else {
                entry.hits = entry.hits.saturating_add(1);
            }
            if entry.block.is_none() && entry.interpreted_steps == 0 {
                if entry.hits < HOT_THRESHOLD {
                    return Ok(0);
                }
                let mut words = Vec::new();
                let mut operations = Vec::new();
                for offset in 0..MAX_OPERATIONS {
                    let Some(word) = bus.peek32(cpu.pc.wrapping_add(offset as u32 * 4)) else {
                        break;
                    };
                    let Some(operation) = decode(word) else {
                        break;
                    };
                    words.push(word);
                    operations.push(operation);
                }
                if words.len() < 2 {
                    entry.interpreted_steps = interpreted_span(bus, cpu.pc);
                } else {
                    let code =
                        Block::compile(&operations).map_err(|e| EmuError::Jit(e.to_string()))?;
                    entry.block = Some(Arc::new(Compiled { code, words }));
                    self.stats.compiled_blocks += 1;
                }
            }
            if entry.interpreted_steps != 0 {
                // Avoid a JIT lookup for every instruction of short load/branch
                // polling loops. This is only a dispatch hint: each step still
                // fetches its real opcode, so code writes/IRQs need no guard.
                let count = entry.interpreted_steps.min(limit);
                for _ in 0..count {
                    cpu.step_timed(bus)?;
                }
                return Ok(count);
            }
            let block = entry.block.as_ref().expect("hot supported block compiled");
            let (completed, changed) = cpu.run_native(bus, block, limit)?;
            self.stats.native_instructions += completed as u64;
            if changed {
                entry.hits = 0;
                entry.block = None;
                self.stats.code_invalidations += 1;
            }
            Ok(completed)
        }
    }

    fn interpreted_span(bus: &Bus, pc: u32) -> usize {
        const MAX_STEPS: usize = 16;
        for offset in 0..MAX_STEPS {
            let Some(word) = bus.peek32(pc.wrapping_add(offset as u32 * 4)) else {
                return offset.max(1);
            };
            if matches!(word >> 26, 1..=7) || (word >> 26 == 0 && matches!(word & 63, 8 | 9)) {
                return (offset + 2).min(MAX_STEPS); // Include the delay slot.
            }
        }
        MAX_STEPS
    }

    fn decode(word: u32) -> Option<Operation> {
        let (source, other, destination) = (
            ((word >> 21) & 31) as u8,
            ((word >> 16) & 31) as u8,
            ((word >> 11) & 31) as u8,
        );
        let signed = i32::from(word as i16) as u32;
        Some(match word >> 26 {
            0 => match word & 63 {
                function @ (0 | 2 | 3) => Operation::Shift {
                    kind: match function {
                        0 => Shift::Left,
                        2 => Shift::Right,
                        _ => Shift::ArithmeticRight,
                    },
                    destination,
                    value: other,
                    amount: ((word >> 6) & 31) as u8,
                },
                function @ (4 | 6 | 7) => Operation::VariableShift {
                    kind: match function {
                        4 => Shift::Left,
                        6 => Shift::Right,
                        _ => Shift::ArithmeticRight,
                    },
                    destination,
                    value: other,
                    amount_register: source,
                },
                function @ (33 | 35..=39 | 42 | 43) => Operation::Binary {
                    kind: match function {
                        33 => Binary::Add,
                        35 => Binary::Subtract,
                        36 => Binary::And,
                        37 => Binary::Or,
                        38 => Binary::Xor,
                        39 => Binary::Nor,
                        42 => Binary::LessSigned,
                        _ => Binary::LessUnsigned,
                    },
                    destination,
                    left: source,
                    right: other,
                },
                _ => return None,
            },
            op @ 9..=15 => Operation::Immediate {
                kind: match op {
                    9 => Immediate::Add,
                    10 => Immediate::LessSigned,
                    11 => Immediate::LessUnsigned,
                    12 => Immediate::And,
                    13 => Immediate::Or,
                    14 => Immediate::Xor,
                    _ => Immediate::Constant,
                },
                destination: other,
                source,
                value: match op {
                    9..=11 => signed,
                    15 => word << 16,
                    _ => word & 0xffff,
                },
            },
            _ => return None,
        })
    }
}

#[cfg(feature = "jit")]
pub(crate) use native::Compiled;

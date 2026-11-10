//! A small, original x86-64 integer code generator with a safe execution API.
//! Only this crate crosses the executable-memory/FFI boundary. It does not
//! accept arbitrary machine code, embed guest addresses or expose code pointers.
//! See SAFETY.md for the ABI, memory bounds and callback/unwind invariants.
use std::io;

#[derive(Clone, Copy, Debug)]
pub enum Binary {
    Add,
    Subtract,
    And,
    Or,
    Xor,
    Nor,
    LessSigned,
    LessUnsigned,
}
#[derive(Clone, Copy, Debug)]
pub enum Immediate {
    Add,
    And,
    Or,
    Xor,
    LessSigned,
    LessUnsigned,
    Constant,
}
#[derive(Clone, Copy, Debug)]
pub enum Shift {
    Left,
    Right,
    ArithmeticRight,
}

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Binary {
        kind: Binary,
        destination: u8,
        left: u8,
        right: u8,
    },
    Immediate {
        kind: Immediate,
        destination: u8,
        source: u8,
        value: u32,
    },
    Shift {
        kind: Shift,
        destination: u8,
        value: u8,
        amount: u8,
    },
    VariableShift {
        kind: Shift,
        destination: u8,
        value: u8,
        amount_register: u8,
    },
}

impl Operation {
    fn registers(self) -> [u8; 3] {
        match self {
            Self::Binary {
                destination,
                left,
                right,
                ..
            } => [destination, left, right],
            Self::Immediate {
                destination,
                source,
                ..
            } => [destination, source, 0],
            Self::Shift {
                destination,
                value,
                amount,
                ..
            } => [destination, value, amount],
            Self::VariableShift {
                destination,
                value,
                amount_register,
                ..
            } => [destination, value, amount_register],
        }
    }
}

pub const SUPPORTED: bool = cfg!(all(target_arch = "x86_64", target_os = "linux"));
pub const MAX_OPERATIONS: usize = 64;

pub struct Block {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    executable: memmap2::Mmap,
    count: usize,
}

impl Block {
    pub fn compile(operations: &[Operation]) -> io::Result<Self> {
        if operations.is_empty()
            || operations.len() > MAX_OPERATIONS
            || operations
                .iter()
                .any(|op| op.registers().iter().any(|&r| r >= 32))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "JIT block/register/shift is out of bounds",
            ));
        }
        #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
        {
            let bytes = x64::emit(operations);
            let mut writable = memmap2::MmapMut::map_anon(bytes.len())?;
            writable.copy_from_slice(&bytes);
            // W^X: the writable mapping is consumed; callers never receive it.
            let executable = writable.make_exec()?;
            Ok(Self {
                executable,
                count: operations.len(),
            })
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Native JIT currently supports Linux x86-64",
            ))
        }
    }

    /// Execute at most `limit` operations. The gate runs at boundary 0 and
    /// after every completed operation, including the final one. Returning
    /// false exits before the next operation; callback panics resume in Rust
    /// only after returning from the native frame. Register zero is read-only.
    pub fn run(
        &self,
        registers: &mut [u32; 32],
        limit: usize,
        gate: impl FnMut(usize) -> bool,
    ) -> usize {
        #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
        {
            x64::run(&self.executable, registers, limit.min(self.count), gate)
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
        {
            let _ = (registers, limit, gate);
            unreachable!("Unsupported hosts cannot construct a Block")
        }
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
mod x64;

#[cfg(all(test, target_arch = "x86_64", target_os = "linux"))]
mod tests;

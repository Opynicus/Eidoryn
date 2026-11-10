//! SysV AMD64 ABI, integer registers only. r12 holds the 32-word guest register
//! array, r13 the callback context, r14 its trampoline. Three pushes align rsp
//! to 16 bytes before CALL; all callee-saved registers are restored on every exit.
use super::{Binary, Immediate, Operation, Shift};
use std::{
    any::Any,
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};

type Gate = unsafe extern "C" fn(*mut c_void, u32) -> bool;
type Entry = unsafe extern "C" fn(*mut u32, *mut c_void, Gate) -> u32;

struct Context<F> {
    gate: F,
    limit: usize,
    panic: Option<Box<dyn Any + Send>>,
}

unsafe extern "C" fn trampoline<F: FnMut(usize) -> bool>(
    context: *mut c_void,
    completed: u32,
) -> bool {
    // SAFETY: run supplies the live, aligned Context<F> on its stack; generated
    // code forwards that exact pointer synchronously and never retains it.
    let context = unsafe { &mut *context.cast::<Context<F>>() };
    match catch_unwind(AssertUnwindSafe(|| (context.gate)(completed as usize))) {
        Ok(keep_running) => keep_running && (completed as usize) < context.limit,
        Err(panic) => {
            context.panic = Some(panic);
            false
        }
    }
}

pub(super) fn run<F: FnMut(usize) -> bool>(
    mapping: &memmap2::Mmap,
    registers: &mut [u32; 32],
    limit: usize,
    gate: F,
) -> usize {
    let mut context = Context {
        gate,
        limit,
        panic: None,
    };
    // SAFETY: mapping contains only emit's validated SysV function, starting at
    // offset zero, and is RX for this entire borrow. Entry matches its exact
    // ABI. emit bounds every register load/store to this exclusive 32-word
    // array. Context and its matching trampoline live until Entry returns.
    // The trampoline catches panics before they could cross a native frame.
    let completed = unsafe {
        let entry: Entry = std::mem::transmute(mapping.as_ptr());
        entry(
            registers.as_mut_ptr(),
            (&raw mut context).cast(),
            trampoline::<F>,
        )
    };
    if let Some(panic) = context.panic {
        resume_unwind(panic);
    }
    completed as usize
}

pub(super) fn emit(operations: &[Operation]) -> Vec<u8> {
    let mut code = Vec::with_capacity(operations.len() * 64 + 64);
    // ENDBR64; push r12/r13/r14; mov r12,rdi; mov r13,rsi; mov r14,rdx.
    code.extend_from_slice(&[
        0xf3, 0x0f, 0x1e, 0xfa, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x49, 0x89, 0xfc, 0x49, 0x89,
        0xf5, 0x49, 0x89, 0xd6,
    ]);
    for (index, operation) in operations.iter().copied().enumerate() {
        boundary(&mut code, index as u32);
        let destination = operation.registers()[0];
        if destination == 0 {
            continue;
        }
        match operation {
            Operation::Binary {
                kind, left, right, ..
            } => {
                load(&mut code, 0, left);
                load(&mut code, 1, right);
                match kind {
                    Binary::Add => code.extend_from_slice(&[0x01, 0xc8]),
                    Binary::Subtract => code.extend_from_slice(&[0x29, 0xc8]),
                    Binary::And => code.extend_from_slice(&[0x21, 0xc8]),
                    Binary::Or => code.extend_from_slice(&[0x09, 0xc8]),
                    Binary::Xor => code.extend_from_slice(&[0x31, 0xc8]),
                    Binary::Nor => code.extend_from_slice(&[0x09, 0xc8, 0xf7, 0xd0]),
                    Binary::LessSigned | Binary::LessUnsigned => {
                        code.extend_from_slice(&[0x39, 0xc8]);
                        compare(&mut code, matches!(kind, Binary::LessSigned));
                    }
                }
            }
            Operation::Immediate {
                kind,
                source,
                value,
                ..
            } => {
                load(&mut code, 0, source);
                code.push(match kind {
                    Immediate::Add => 0x05,
                    Immediate::And => 0x25,
                    Immediate::Or => 0x0d,
                    Immediate::Xor => 0x35,
                    Immediate::Constant => 0xb8,
                    Immediate::LessSigned | Immediate::LessUnsigned => 0x3d,
                });
                code.extend_from_slice(&value.to_le_bytes());
                if matches!(kind, Immediate::LessSigned | Immediate::LessUnsigned) {
                    compare(&mut code, matches!(kind, Immediate::LessSigned));
                }
            }
            Operation::Shift {
                kind,
                value,
                amount,
                ..
            } => {
                load(&mut code, 0, value);
                code.extend_from_slice(&[0xc1, shift_code(kind), amount]);
            }
            Operation::VariableShift {
                kind,
                value,
                amount_register,
                ..
            } => {
                load(&mut code, 0, value);
                load(&mut code, 1, amount_register);
                code.extend_from_slice(&[0xd3, shift_code(kind)]);
            }
        }
        // mov [r12 + destination*4],eax. The highest displacement is 124.
        code.extend_from_slice(&[0x41, 0x89, 0x44, 0x24, destination * 4]);
    }
    boundary(&mut code, operations.len() as u32);
    finish(&mut code, operations.len() as u32);
    code
}

fn load(code: &mut Vec<u8>, host: u8, guest: u8) {
    // mov eax/ecx,[r12 + guest*4], including the SIB byte required by r12.
    code.extend_from_slice(&[0x41, 0x8b, 0x44 | (host << 3), 0x24, guest * 4]);
}
fn compare(code: &mut Vec<u8>, signed: bool) {
    code.extend_from_slice(&[
        0x0f,
        if signed { 0x9c } else { 0x92 },
        0xc0,
        0x0f,
        0xb6,
        0xc0,
    ]);
}
fn shift_code(kind: Shift) -> u8 {
    match kind {
        Shift::Left => 0xe0,
        Shift::Right => 0xe8,
        Shift::ArithmeticRight => 0xf8,
    }
}
fn boundary(code: &mut Vec<u8>, completed: u32) {
    // mov rdi,r13; mov esi,completed; call r14; test al,al; jnz +12.
    code.extend_from_slice(&[0x4c, 0x89, 0xef, 0xbe]);
    code.extend_from_slice(&completed.to_le_bytes());
    code.extend_from_slice(&[0x41, 0xff, 0xd6, 0x84, 0xc0, 0x75, 12]);
    finish(code, completed);
}
fn finish(code: &mut Vec<u8>, completed: u32) {
    code.push(0xb8); // mov eax,completed; pop r14/r13/r12; ret.
    code.extend_from_slice(&completed.to_le_bytes());
    code.extend_from_slice(&[0x41, 0x5e, 0x41, 0x5d, 0x41, 0x5c, 0xc3]);
}

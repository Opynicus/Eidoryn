use super::*;

fn reference(op: Operation, r: &mut [u32; 32]) {
    let destination = op.registers()[0] as usize;
    let value = match op {
        Operation::Binary {
            kind, left, right, ..
        } => {
            let (a, b) = (r[left as usize], r[right as usize]);
            match kind {
                Binary::Add => a.wrapping_add(b),
                Binary::Subtract => a.wrapping_sub(b),
                Binary::And => a & b,
                Binary::Or => a | b,
                Binary::Xor => a ^ b,
                Binary::Nor => !(a | b),
                Binary::LessSigned => u32::from((a as i32) < (b as i32)),
                Binary::LessUnsigned => u32::from(a < b),
            }
        }
        Operation::Immediate {
            kind,
            source,
            value,
            ..
        } => {
            let a = r[source as usize];
            match kind {
                Immediate::Add => a.wrapping_add(value),
                Immediate::And => a & value,
                Immediate::Or => a | value,
                Immediate::Xor => a ^ value,
                Immediate::Constant => value,
                Immediate::LessSigned => u32::from((a as i32) < (value as i32)),
                Immediate::LessUnsigned => u32::from(a < value),
            }
        }
        Operation::Shift {
            kind,
            value,
            amount,
            ..
        } => shift(kind, r[value as usize], u32::from(amount)),
        Operation::VariableShift {
            kind,
            value,
            amount_register,
            ..
        } => shift(kind, r[value as usize], r[amount_register as usize]),
    };
    if destination != 0 {
        r[destination] = value;
    }
}
fn shift(kind: Shift, value: u32, amount: u32) -> u32 {
    match kind {
        Shift::Left => value.wrapping_shl(amount),
        Shift::Right => value.wrapping_shr(amount),
        Shift::ArithmeticRight => (value as i32).wrapping_shr(amount) as u32,
    }
}

#[test]
fn native_blocks_match_reference_at_every_exit_and_preserve_array_bounds() {
    let mut seed = 0x1234_5678_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for case in 0..120 {
        let mut operations = Vec::new();
        for index in 0..64 {
            let (destination, left, right) = (
                (next() & 31) as u8,
                (next() & 31) as u8,
                (next() & 31) as u8,
            );
            operations.push(match (case + index) % 4 {
                0 => Operation::Binary {
                    kind: [
                        Binary::Add,
                        Binary::Subtract,
                        Binary::And,
                        Binary::Or,
                        Binary::Xor,
                        Binary::Nor,
                        Binary::LessSigned,
                        Binary::LessUnsigned,
                    ][next() as usize % 8],
                    destination,
                    left,
                    right,
                },
                1 => Operation::Immediate {
                    kind: [
                        Immediate::Add,
                        Immediate::And,
                        Immediate::Or,
                        Immediate::Xor,
                        Immediate::LessSigned,
                        Immediate::LessUnsigned,
                        Immediate::Constant,
                    ][next() as usize % 7],
                    destination,
                    source: left,
                    value: next(),
                },
                2 => Operation::Shift {
                    kind: [Shift::Left, Shift::Right, Shift::ArithmeticRight][next() as usize % 3],
                    destination,
                    value: left,
                    amount: right,
                },
                _ => Operation::VariableShift {
                    kind: [Shift::Left, Shift::Right, Shift::ArithmeticRight][next() as usize % 3],
                    destination,
                    value: left,
                    amount_register: right,
                },
            });
        }
        let code = Block::compile(&operations).unwrap();
        let initial = std::array::from_fn(|_| next());
        for limit in 0..=64 {
            let mut storage = [0xdead_beef; 34];
            let registers: &mut [u32; 32] = (&mut storage[1..33]).try_into().unwrap();
            *registers = initial;
            let mut seen = Vec::new();
            assert_eq!(
                code.run(registers, limit, |n| {
                    seen.push(n);
                    true
                }),
                limit
            );
            assert_eq!(seen, (0..=limit).collect::<Vec<_>>());
            let mut expected = initial;
            for &op in &operations[..limit] {
                reference(op, &mut expected);
            }
            assert_eq!(*registers, expected, "case={case} exit={limit}");
            assert_eq!(storage[0], 0xdead_beef);
            assert_eq!(storage[33], 0xdead_beef);
        }
    }
}

#[test]
fn callbacks_can_exit_or_panic_without_unwinding_through_native_frames() {
    let operation = Operation::Immediate {
        kind: Immediate::Add,
        destination: 1,
        source: 1,
        value: 1,
    };
    let code = Block::compile(&[operation; 8]).unwrap();
    let mut registers = [0; 32];
    assert_eq!(code.run(&mut registers, 8, |n| n != 3), 3);
    assert_eq!(registers[1], 3);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        code.run(&mut registers, 8, |n| {
            assert_ne!(n, 4, "callback panic");
            true
        })
    }));
    assert!(panic.is_err());
    assert_eq!(registers[1], 7);
    assert_eq!(code.run(&mut registers, 8, |_| true), 8);
    assert_eq!(registers[1], 15);
}

#[test]
fn rejects_unbounded_code_and_invalid_registers_and_shift_counts() {
    let operation = Operation::Immediate {
        kind: Immediate::Constant,
        destination: 31,
        source: 0,
        value: 1,
    };
    assert!(Block::compile(&[]).is_err());
    assert!(Block::compile(&[operation; 65]).is_err());
    for bad in [
        Operation::Immediate {
            kind: Immediate::Add,
            destination: 32,
            source: 0,
            value: 0,
        },
        Operation::Binary {
            kind: Binary::Or,
            destination: 1,
            left: 255,
            right: 0,
        },
        Operation::Shift {
            kind: Shift::Left,
            destination: 1,
            value: 2,
            amount: 32,
        },
    ] {
        assert!(Block::compile(&[bad]).is_err());
    }
}

#[test]
fn executable_memory_is_not_writable_and_can_be_shared_between_threads() {
    let code = std::sync::Arc::new(
        Block::compile(
            &[Operation::Immediate {
                kind: Immediate::Add,
                destination: 31,
                source: 31,
                value: 5,
            }; 64],
        )
        .unwrap(),
    );
    let address = code.executable.as_ptr() as usize;
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let permissions = maps
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields.next()?.split_once('-')?;
            let start = usize::from_str_radix(start, 16).ok()?;
            let end = usize::from_str_radix(end, 16).ok()?;
            (start <= address && address < end).then(|| fields.next().unwrap())
        })
        .unwrap();
    assert!(permissions.contains('x'));
    assert!(!permissions.contains('w'));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let code = std::sync::Arc::clone(&code);
            std::thread::spawn(move || {
                let mut registers = [0; 32];
                for _ in 0..100 {
                    assert_eq!(code.run(&mut registers, 64, |_| true), 64);
                }
                assert_eq!(registers[31], 32_000);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

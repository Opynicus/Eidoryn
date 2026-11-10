use eidoryn_psx::{bus::Bus, cpu::Cpu, gte::Gte};

const SF: u32 = 1 << 19;
const LM: u32 = 1 << 10;

fn xy(x: i16, y: i16) -> u32 {
    u32::from(x as u16) | (u32::from(y as u16) << 16)
}
fn vector(gte: &mut Gte, n: usize, value: [i16; 3]) {
    gte.write_data(n * 2, xy(value[0], value[1]));
    gte.write_data(n * 2 + 1, value[2] as u32);
}
fn identity(gte: &mut Gte, base: usize) {
    for (n, value) in [4096, 0, 4096, 0, 4096].into_iter().enumerate() {
        gte.write_control(base + n, value);
    }
}
fn ir(gte: &Gte) -> [i32; 3] {
    std::array::from_fn(|n| gte.read_data(9 + n) as i32)
}

#[test]
fn register_widths_fifo_aliases_color_conversion_and_leading_bits() {
    let mut gte = Gte::default();
    gte.write_data(1, 0x1234_8001);
    gte.write_data(7, 0x1234_8001);
    gte.write_control(26, 0x1234_8001);
    assert_eq!(gte.read_data(1), 0xffff_8001);
    assert_eq!(gte.read_data(7), 0x8001);
    assert_eq!(gte.read_control(26), 0xffff_8001);
    for n in 1..=3 {
        gte.write_data(15, n);
    }
    assert_eq!([12, 13, 14, 15].map(|r| gte.read_data(r)), [1, 2, 3, 3]);
    gte.write_data(14, 4);
    assert_eq!(gte.read_data(13), 2);
    assert_eq!(gte.read_data(15), 4);
    gte.write_data(28, 0x7c1f);
    assert_eq!(ir(&gte), [3968, 0, 3968]);
    gte.write_data(9, (-128_i32) as u32);
    gte.write_data(10, 5000);
    assert_eq!(gte.read_data(29), 0x7fe0);
    gte.write_data(29, 0);
    assert_eq!(gte.read_data(28), 0x7fe0);
    for (source, count) in [
        (0, 32),
        (u32::MAX, 32),
        (0x00ff_ffff, 8),
        (0xff00_0000, 8),
        (0x8000_0000, 1),
    ] {
        gte.write_data(30, source);
        assert_eq!(gte.read_data(31), count);
        gte.write_data(31, 0);
        assert_eq!(gte.read_data(31), count);
    }
    gte.write_control(31, u32::MAX);
    assert_eq!(gte.read_control(31), 0xffff_f000);
    gte.write_control(31, 1 << 22);
    assert_eq!(gte.read_control(31), 1 << 22); // IR3 alone is excluded from summary bit 31.
}

#[test]
fn perspective_projects_vertices_and_advances_both_coordinate_fifos() {
    let mut gte = Gte::default();
    identity(&mut gte, 0);
    gte.write_control(24, 160 << 16);
    gte.write_control(25, 120 << 16);
    gte.write_control(26, 128);
    gte.write_control(27, 256);
    vector(&mut gte, 0, [32, -16, 256]);
    vector(&mut gte, 1, [-64, 32, 512]);
    vector(&mut gte, 2, [0, 0, 128]);
    assert!(gte.execute(SF | 1));
    assert_eq!(gte.read_data(14), xy(176, 112));
    assert_eq!(gte.read_data(19), 256);
    assert_eq!(gte.read_data(8), 2048);
    assert_eq!(gte.read_control(31), 0);
    assert!(gte.execute(SF | 0x30));
    assert_eq!(
        [12, 13, 14].map(|r| gte.read_data(r)),
        [xy(176, 112), xy(144, 128), xy(160, 120)]
    );
    assert_eq!(
        [16, 17, 18, 19].map(|r| gte.read_data(r)),
        [256, 256, 512, 128]
    );
    assert_eq!(gte.read_data(8), 4096);
    assert_eq!(ir(&gte), [0, 0, 128]);
    assert_eq!(gte.read_control(31), 0);
}

#[test]
fn perspective_near_clip_screen_saturation_and_ir3_flag_quirk() {
    let mut gte = Gte::default();
    identity(&mut gte, 0);
    gte.write_control(26, 128);
    vector(&mut gte, 0, [2000, -2000, 64]);
    gte.execute(SF | 1);
    assert_eq!(gte.read_data(14), xy(1023, -1024));
    assert_eq!(gte.read_control(31), 0x8002_6000);
    vector(&mut gte, 0, [0, 0, 10000]);
    gte.execute(1); // sf=0 stores saturated IR3; flag still tests Z / 4096.
    assert_eq!(gte.read_data(11), 32767);
    assert_eq!(gte.read_data(19), 10000);
    assert_eq!(gte.read_control(31), 0);
    vector(&mut gte, 0, [0, 0, -100]);
    gte.execute(SF | LM | 1);
    assert_eq!(gte.read_data(11), 0);
    assert_eq!(gte.read_control(31) & (1 << 22), 0); // Flag ignores lm=1's negative clamp.
    assert_ne!(gte.read_control(31) & (1 << 18), 0);
    assert_ne!(gte.read_control(31) & (1 << 17), 0);
}

#[test]
fn reciprocal_rounding_can_cap_without_division_overflow() {
    let mut gte = Gte::default();
    identity(&mut gte, 0);
    vector(&mut gte, 0, [0, 0, 0x7f20]);
    gte.write_control(26, 0xfe3f);
    gte.write_control(27, 1);
    gte.execute(SF | 1);
    assert_eq!(gte.read_data(24), 0x1ffff);
    assert_eq!(gte.read_control(31), 0); // UNR rounds up to 0x20000, capped without FLAG.17.
}

#[test]
fn matrix_transform_far_color_bug_lm_and_internal_44_bit_wrap() {
    let mut gte = Gte::default();
    identity(&mut gte, 0);
    vector(&mut gte, 0, [20, 30, 40]);
    for (n, value) in [10_i32, -10, 50].into_iter().enumerate() {
        gte.write_control(5 + n, value as u32);
    }
    gte.execute(SF | 0x12);
    assert_eq!(ir(&gte), [30, 20, 90]);
    gte.write_control(5, (-100_i32) as u32);
    gte.execute(SF | LM | 0x12);
    assert_eq!(ir(&gte), [0, 20, 90]);
    assert_eq!(gte.read_control(31), 0x8100_0000);
    for n in 21..24 {
        gte.write_control(n, 100);
    }
    gte.execute(SF | (2 << 13) | 0x12);
    assert_eq!(ir(&gte), [0, 30, 40]); // First column and FC are discarded by hardware.
    vector(&mut gte, 0, [32767, 0, 0]);
    gte.write_control(0, 32767);
    gte.write_control(5, i32::MAX as u32);
    gte.execute(SF | 0x12);
    assert_ne!(gte.read_control(31) & (1 << 30), 0);
    assert_eq!(ir(&gte)[0], -32768); // Overflow wraps before the IR saturation step.
    assert!((gte.read_data(25) as i32) < 0);
}

#[test]
fn clipping_and_depth_sort_check_full_mac_before_truncation() {
    let mut gte = Gte::default();
    for (n, point) in [xy(-32768, -32768), xy(32767, -32768), xy(-32768, 32767)]
        .into_iter()
        .enumerate()
    {
        gte.write_data(12 + n, point);
    }
    gte.execute(6);
    assert_eq!(gte.read_data(24), 0xfffe_0001);
    assert_eq!(gte.read_control(31), 0x8001_0000);
    for (n, z) in [100, 200, 300, 400].into_iter().enumerate() {
        gte.write_data(16 + n, z);
    }
    gte.write_control(30, 1024);
    gte.execute(0x2e);
    assert_eq!(gte.read_data(7), 250);
    assert_eq!(gte.read_data(24), 1_024_000);
    gte.write_control(29, 4096);
    gte.execute(0x2d);
    assert_eq!(gte.read_data(7), 900);
    gte.write_control(30, (-1024_i32) as u32);
    gte.execute(0x2e);
    assert_eq!(gte.read_data(7), 0);
    assert_eq!(gte.read_control(31), 0x8004_0000);
}

#[test]
fn lighting_color_modulation_and_depth_cue_single_and_triple() {
    for (single, triple, expected) in [
        (0x1e, 0x20, [0x20c0_8040, 0x2020_4080, 0x2040_c020]),
        (0x1b, 0x3f, [0x2060_4020, 0x2010_2040, 0x2020_6010]),
        (0x13, 0x16, [0x20b0_a090, 0x2088_90a0, 0x2090_b088]),
    ] {
        let mut gte = Gte::default();
        identity(&mut gte, 8);
        identity(&mut gte, 16);
        vector(&mut gte, 0, [1024, 2048, 3072]);
        vector(&mut gte, 1, [2048, 1024, 512]);
        vector(&mut gte, 2, [512, 3072, 1024]);
        gte.write_data(6, 0x2080_8080);
        gte.write_data(8, 2048);
        for n in 21..24 {
            gte.write_control(n, 4096);
        }
        gte.execute(SF | LM | single);
        assert_eq!(gte.read_data(22), expected[0]);
        gte.execute(SF | LM | triple);
        assert_eq!([20, 21, 22].map(|r| gte.read_data(r)), expected);
        assert_eq!(gte.read_control(31), 0);
    }
}

#[test]
fn standalone_color_interpolation_and_color_fifo_source_order() {
    let mut gte = Gte::default();
    gte.write_data(6, 0x2060_4020);
    gte.write_data(8, 2048);
    for (n, value) in [2560, 2048, 1024].into_iter().enumerate() {
        gte.write_control(21 + n, value);
    }
    gte.execute(SF | 0x10);
    assert_eq!(gte.read_data(22), 0x2050_6060);
    let colors = [0x3030_2010, 0x4060_4020, 0x5080_6040];
    for (n, value) in colors.into_iter().enumerate() {
        gte.write_data(20 + n, value);
    }
    gte.write_data(8, 0);
    gte.execute(SF | 0x2a);
    assert_eq!(
        [20, 21, 22].map(|r| gte.read_data(r)),
        colors.map(|v| (v & 0xff_ffff) | 0x2000_0000)
    );
    for (opcode, expected) in [
        (0x1c, 0x2060_4020),
        (0x14, 0x20b0_a090),
        (0x29, 0x20b0_a090),
        (0x11, 0x20e0_c0a0),
    ] {
        identity(&mut gte, 16);
        for (n, value) in [1024, 2048, 3072].into_iter().enumerate() {
            gte.write_data(9 + n, value);
        }
        for n in 21..24 {
            gte.write_control(n, 4096);
        }
        gte.write_data(6, 0x2080_8080);
        gte.write_data(8, 2048);
        gte.execute(SF | LM | opcode);
        assert_eq!(gte.read_data(22), expected, "opcode={opcode:x}");
    }
}

#[test]
fn general_vector_operations_and_color_saturation() {
    let mut gte = Gte::default();
    for (n, value) in [5, 6, 7].into_iter().enumerate() {
        gte.write_data(9 + n, value);
    }
    for (n, value) in [2, 3, 4].into_iter().enumerate() {
        gte.write_control(n * 2, value);
    }
    gte.execute(0x0c);
    assert_eq!(ir(&gte), [-3, 6, -3]);
    for (n, value) in [-4096_i32, 2048, 1024].into_iter().enumerate() {
        gte.write_data(9 + n, value as u32);
    }
    gte.execute(SF | 0x28);
    assert_eq!(ir(&gte), [4096, 1024, 256]);
    for (n, value) in [-16_i32, 32, 5000].into_iter().enumerate() {
        gte.write_data(9 + n, value as u32);
    }
    gte.write_data(8, 2);
    gte.execute(0x3d);
    assert_eq!(ir(&gte), [-32, 64, 10000]);
    assert_eq!(gte.read_data(22), 0x00ff_0400);
    assert_eq!(gte.read_control(31), (1 << 21) | (1 << 19));
    gte.write_data(8, 4096);
    gte.execute(SF | 0x3e);
    assert_eq!(ir(&gte), [-64, 128, 20000]);
    assert!(!gte.execute(0x3c));
}

#[test]
fn cpu_cop2_transfers_have_load_delay_and_memory_alignment_exceptions() {
    let mut bus = Bus::default();
    let mut cpu = Cpu::new(0);
    cpu.cop0[12] |= 1 << 30;
    cpu.registers[8] = 0x1234_8001;
    cpu.registers[9] = 42;
    let words = [
        0x4888_4800, // mtc2 t0, IR1
        0x4809_4800, // mfc2 t1, IR1
        0x0120_5021, // addu t2, t1, zero (old value)
        0x0120_5821, // addu t3, t1, zero (loaded value)
        0xc80c_0100, // lwc2 SXY0, 0x100(zero)
        0xe80c_0104, // swc2 SXY0, 0x104(zero)
        0xc80c_0101, // unaligned lwc2
    ];
    for (n, word) in words.into_iter().enumerate() {
        bus.write32(n as u32 * 4, word).unwrap();
    }
    bus.write32(0x100, 0x1234_5678).unwrap();
    for _ in 0..6 {
        cpu.step(&mut bus).unwrap();
    }
    assert_eq!(cpu.registers[10], 42);
    assert_eq!(cpu.registers[11], 0xffff_8001);
    assert_eq!(bus.peek32(0x104), Some(0x1234_5678));
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.cop0[13] >> 2 & 31, 4);
    assert_eq!(cpu.cop0[8], 0x101);
    let mut disabled = Cpu::new(0);
    disabled.step(&mut bus).unwrap();
    assert_eq!(disabled.cop0[13] >> 2 & 31, 11);
    assert_eq!(disabled.cop0[13] >> 28 & 3, 2);
    assert_eq!(disabled.gte.read_data(9), 0);
}

use super::{DAY, EPOCH_2000, MODE_24H, Rtc};

#[test]
fn a_programmed_leap_day_rolls_over_and_survives_battery_reload() {
    let mut rtc = Rtc::default();
    let host = EPOCH_2000 + DAY * 8000;
    rtc.program(2, [0x24, 0x02, 0x29, 3, 0x23, 0x59, 0x59], host);
    let mut restored = Rtc::default();
    restored.restore_battery(&rtc.battery()).unwrap();
    assert_eq!(restored.datetime(host + 1), [0x24, 3, 1, 4, 0, 0, 0]);
    restored.program(1, [0; 7], host);
    restored.program(3, [0x81, 0x05, 0x27, 0, 0, 0, 0], host + 1);
    assert_eq!(restored.datetime(host + 1), [0x24, 3, 1, 4, 0x81, 5, 0x27]);
    restored.control = MODE_24H;
    assert_eq!(restored.datetime(host + 1)[4], 0x13);
    let before = restored.battery();
    restored.program(2, [0x25, 2, 0x29, 1, 1, 0, 0], host);
    assert_eq!(restored.battery(), before);
    assert!(restored.restore_battery(&[0xff; 10]).is_err());
    assert_eq!(restored.battery(), before);
}

#[test]
fn gpio_reads_are_gated_and_a_partial_command_survives_a_snapshot() {
    use crate::Machine;
    use crate::cpu::Bus;
    let mut rom = vec![0; 0x180];
    rom[0xc4..0xc6].copy_from_slice(&0x1234_u16.to_le_bytes());
    rom[0xe0..0xe8].copy_from_slice(b"SIIRTC_V");
    let mut machine = Machine::from_rom(rom).unwrap();
    let data = 0x0800_00c4;
    assert_eq!(machine.memory.read16(data), 0x1234);
    machine.memory.write16(data + 4, 1);
    assert_eq!(machine.memory.read16(data), 0);
    machine.memory.write16(data + 2, 7);
    machine.memory.write16(data, 1);
    machine.memory.write16(data, 5);
    let send = |m: &mut Machine, from: u32, to: u32| {
        for index in (from..to).rev() {
            let bits = 4 | (((0x63u16 >> index) & 1) << 1);
            m.memory.write16(data, bits);
            m.memory.write16(data, bits | 1);
        }
    };
    send(&mut machine, 4, 8);
    let partial = machine.save_state().unwrap();
    let status = |m: &mut Machine| {
        send(m, 0, 4);
        m.memory.write16(data + 2, 5);
        let mut status = 0;
        for i in 0..8 {
            m.memory.write16(data, 4);
            m.memory.write16(data, 5);
            status |= ((m.memory.read16(data) >> 1) & 1) << i;
        }
        status
    };
    assert_eq!(status(&mut machine), u16::from(MODE_24H));
    machine.reset();
    machine.load_state(&partial).unwrap();
    assert_eq!(status(&mut machine), u16::from(MODE_24H));
}

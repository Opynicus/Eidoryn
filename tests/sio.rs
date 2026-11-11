use eidoryn_psx::{
    bus::Bus,
    sio::{ACK_DELAY, ACK_PULSE, Sio},
};

const DATA: u32 = 0x1f80_1040;
const MODE: u32 = DATA + 8;
const CTRL: u32 = DATA + 10;
const BAUD: u32 = DATA + 14;
const BYTE_CLOCKS: u32 = 0x88 * 8;

fn pad() -> Sio {
    let mut sio = Sio::default();
    sio.write(MODE, 2, 0x0d);
    sio.write(BAUD, 2, 0x88);
    sio.write(CTRL, 2, 3);
    sio
}

fn exchange(sio: &mut Sio, byte: u8) -> u8 {
    sio.write(DATA, 1, u32::from(byte));
    sio.tick(BYTE_CLOCKS);
    let reply = sio.read(DATA, 1) as u8;
    sio.tick(ACK_DELAY + ACK_PULSE);
    reply
}

#[test]
fn batched_bus_ticks_match_each_serial_transfer_and_ack_edge() {
    let mut batched = Bus::default();
    batched.write16(MODE, 0x0d).unwrap();
    batched.write16(BAUD, 0x88).unwrap();
    batched.write16(CTRL, 0x1003).unwrap();
    batched.write32(0x1f80_1074, 1 << 7).unwrap();
    batched.sio.set_buttons(1 << 14);
    let mut single = batched.clone();

    for (byte, reply) in [(1, 0xff), (0x42, 0x41), (0, 0x5a), (0, 0xff), (0, 0xbf)] {
        for bus in [&mut batched, &mut single] {
            bus.write8(DATA, byte).unwrap();
        }
        for clocks in [7, BYTE_CLOCKS - 8, 1, ACK_DELAY - 1, 1, ACK_PULSE] {
            batched.tick(clocks).unwrap();
            for _ in 0..clocks {
                single.tick(1).unwrap();
            }
            assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
            assert_eq!(batched.sio.status(), single.sio.status());
            assert_eq!(batched.sio.transfers, single.sio.transfers);
            assert_eq!(batched.sio.polls, single.sio.polls);
            assert_eq!(batched.read32(0x1f80_1070), single.read32(0x1f80_1070));
            assert_eq!(batched.read16(0x1f80_1100), single.read16(0x1f80_1100));
        }
        assert_eq!(batched.read8(DATA).unwrap(), reply);
        assert_eq!(single.read8(DATA).unwrap(), reply);
        for bus in [&mut batched, &mut single] {
            bus.write16(CTRL, 0x1013).unwrap();
            bus.write32(0x1f80_1070, !(1 << 7)).unwrap();
        }
    }
}

#[test]
fn pad_poll_is_active_low_and_samples_buttons_once_per_command() {
    let mut sio = pad();
    sio.set_buttons((1 << 3) | (1 << 4) | (1 << 14) | 6);
    assert_eq!(exchange(&mut sio, 1), 0xff);
    assert_eq!(exchange(&mut sio, 0x42), 0x41);
    sio.set_buttons(0);
    assert_eq!(exchange(&mut sio, 0), 0x5a);
    assert_eq!(exchange(&mut sio, 0), 0xe7);
    assert_eq!(exchange(&mut sio, 0), 0xbf);
    assert_eq!(sio.polls, 1);
    assert_eq!(exchange(&mut sio, 1), 0xff); // Must deselect before another poll.
    sio.write(CTRL, 2, 1);
    sio.write(CTRL, 2, 3);
    assert_eq!(
        [1, 0x42, 0, 0, 0].map(|b| exchange(&mut sio, b)),
        [0xff, 0x41, 0x5a, 0xff, 0xff]
    );
    assert_eq!(sio.polls, 2);
}

#[test]
fn byte_and_ack_have_separate_delays_and_final_byte_has_no_ack() {
    let mut sio = pad();
    sio.write(CTRL, 2, 0x1003);
    sio.write(DATA, 4, 0x1234_5601); // Upper bytes do not transmit.
    assert_eq!(sio.status() & 7, 1);
    sio.tick(BYTE_CLOCKS - 1);
    assert_eq!(sio.status() & 2, 0);
    sio.tick(1);
    assert_eq!(sio.status() & 7, 7);
    assert_eq!(sio.read(DATA, 1), 0xff);
    sio.tick(ACK_DELAY - 1);
    assert_eq!(sio.status() & 0x280, 0);
    sio.tick(1);
    assert_eq!(sio.status() & 0x280, 0x280);
    assert!(sio.take_irq_edge());
    assert!(!sio.take_irq_edge());
    sio.tick(ACK_PULSE);
    assert_eq!(sio.status() & 0x280, 0x200);
    sio.write(CTRL, 2, 0x1013);
    for byte in [0x42, 0, 0] {
        exchange(&mut sio, byte);
        assert!(sio.take_irq_edge());
        sio.write(CTRL, 2, 0x1013);
    }
    exchange(&mut sio, 0);
    assert!(!sio.take_irq_edge());
    assert_eq!(sio.status() & 0x280, 0);
    assert_eq!(sio.transfers, 5);
}

#[test]
fn pipelined_poll_can_clear_the_previous_ack_while_the_next_byte_is_shifting() {
    let mut sio = pad();
    sio.set_buttons(1 << 3);
    sio.write(CTRL, 2, 0x1003);
    for (byte, reply, ack) in [
        (1, 0xff, true),
        (0x42, 0x41, true),
        (0, 0x5a, true),
        (0, 0xf7, true),
        (0, 0xff, false),
    ] {
        sio.write(DATA, 1, byte);
        // The next byte can start at the previous ACK's leading edge. Acknowledge
        // the old interrupt after a short software delay, before RX is ready.
        sio.tick(76);
        sio.write(CTRL, 2, 0x1013);
        assert!(!sio.take_irq_edge());
        sio.tick(BYTE_CLOCKS - 76);
        assert_eq!(sio.read(DATA, 1), reply);
        sio.tick(ACK_DELAY);
        assert_eq!(sio.take_irq_edge(), ack);
    }
}

#[test]
fn irq7_is_edge_latched_and_acknowledging_an_active_condition_retriggers() {
    let mut bus = Bus::default();
    bus.write16(MODE, 0x0d).unwrap();
    bus.write16(BAUD, 0x88).unwrap();
    bus.write16(CTRL, 0x1003).unwrap();
    bus.write32(0x1f80_1074, 1 << 7).unwrap();
    bus.write8(DATA, 1).unwrap();
    bus.tick(BYTE_CLOCKS + ACK_DELAY).unwrap();
    assert!(bus.irq_pending());
    bus.write32(0x1f80_1070, !(1 << 7)).unwrap();
    bus.tick(1).unwrap();
    assert!(!bus.irq_pending()); // Device latch alone does not hold I_STAT high.
    bus.write16(CTRL, 0x1013).unwrap();
    bus.tick(1).unwrap();
    assert!(bus.irq_pending()); // ACK line is still active; a fresh device edge.
    bus.tick(ACK_PULSE).unwrap();
    bus.write16(CTRL, 0x1013).unwrap();
    bus.write32(0x1f80_1070, !(1 << 7)).unwrap();
    bus.tick(1).unwrap();
    assert!(!bus.irq_pending());
}

#[test]
fn receive_threshold_halfword_preview_word_pop_and_overrun() {
    let mut sio = pad();
    sio.write(CTRL, 2, 0x0903); // RX IRQ after two bytes.
    for byte in [1, 0x42] {
        sio.write(DATA, 1, byte);
        sio.tick(BYTE_CLOCKS + ACK_DELAY + ACK_PULSE);
    }
    assert!(sio.take_irq_edge());
    assert_eq!(sio.read(DATA, 2), 0x41ff); // Only consumes ff.
    sio.write(CTRL, 2, 0x0913);
    assert!(!sio.take_irq_edge());
    for byte in [0; 10] {
        sio.write(DATA, 1, byte);
        sio.tick(BYTE_CLOCKS + ACK_DELAY + ACK_PULSE);
    }
    assert!(sio.take_irq_edge());
    assert_eq!(sio.read(DATA, 4), 0xffff_5a41);
    assert_ne!(sio.status() & 2, 0);
    assert_eq!(sio.read(DATA, 4), u32::MAX);
    assert_eq!(sio.status() & 2, 0); // FIFO is limited to eight bytes.
}

#[test]
fn pending_transmit_remembers_enable_and_reset_keeps_host_buttons() {
    let mut sio = pad();
    sio.write(DATA, 1, 1);
    sio.write(DATA, 1, 0x42);
    assert_eq!(sio.status() & 5, 0);
    sio.write(CTRL, 2, 2); // Latched TXEN still transmits the second byte.
    sio.tick(BYTE_CLOCKS * 2);
    assert_eq!(sio.read(DATA, 2), 0x41ff);
    assert_eq!(sio.read(DATA, 1), 0x41);
    sio.write(DATA, 1, 0);
    sio.tick(BYTE_CLOCKS * 2);
    assert_eq!(sio.transfers, 2); // A disabled TX must stay queued.
    sio.write(CTRL, 2, 3);
    sio.tick(BYTE_CLOCKS);
    assert_eq!(sio.read(DATA, 1), 0x5a);
    sio.set_buttons(1 << 14);
    sio.write(CTRL, 1, 0x40);
    assert_eq!(sio.status(), 5);
    assert_eq!(sio.read(MODE, 4), 0);
    assert_eq!(sio.read(BAUD, 2), 0);
    assert_eq!(sio.buttons(), 1 << 14);
}

#[test]
fn absent_card_second_port_and_invalid_command_do_not_acknowledge() {
    for (control, bytes) in [
        (0x1003, [0x81, 0x52]),
        (0x3003, [1, 0x42]),
        (0x1003, [1, 0x43]),
    ] {
        let mut sio = pad();
        sio.write(CTRL, 2, control);
        exchange(&mut sio, bytes[0]);
        sio.take_irq_edge();
        sio.write(CTRL, 2, control | 0x10);
        assert_eq!(exchange(&mut sio, bytes[1]), 0xff);
        assert!(!sio.take_irq_edge());
        assert_eq!(sio.polls, 0);
    }
}

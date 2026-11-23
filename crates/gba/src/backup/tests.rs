use crate::{HEADER_LEN, Machine, cpu::Bus};

const PARALLEL: u32 = 0x0e00_0000;
const SERIAL: u32 = 0x0dff_ff00;
const RAM: u32 = 0x0200_0000;

fn machine(tag: &[u8]) -> Machine {
    let mut rom = vec![0; HEADER_LEN];
    rom.extend_from_slice(tag);
    Machine::from_rom(rom).unwrap()
}

fn unlock(cart: &mut Machine, command: u8) {
    cart.memory.write8(PARALLEL + 0x5555, 0xaa);
    cart.memory.write8(PARALLEL + 0x2aaa, 0x55);
    cart.memory.write8(PARALLEL + 0x5555, command);
}

fn program(cart: &mut Machine, offset: u32, value: u8) {
    unlock(cart, 0xa0);
    cart.memory.write8(PARALLEL + offset, value);
}

#[test]
fn flash_ids_sectors_banks_and_partially_issued_commands() {
    for (tag, size, chip_id) in [
        (&b"FLASH_V126"[..], 65_536, [0x32, 0x1b]),
        (&b"FLASH512_V130"[..], 65_536, [0x32, 0x1b]),
        (&b"FLASH1M_V103"[..], 131_072, [0x62, 0x13]),
    ] {
        let mut cart = machine(tag);
        assert_eq!(cart.battery_save().len(), size);
        unlock(&mut cart, 0x90);
        assert_eq!(
            [cart.memory.read8(PARALLEL), cart.memory.read8(PARALLEL + 1)],
            chip_id
        );
        unlock(&mut cart, 0xf0);
        cart.memory.write8(PARALLEL + 7, 0);
        assert_eq!(cart.memory.read8(PARALLEL + 7), 0xff);
        assert!(!cart.take_battery_dirty());
        program(&mut cart, 7, 0x9b);
        program(&mut cart, 7, 0x71);
        assert_eq!(cart.battery_save()[7], 0x11);
        program(&mut cart, 0x1123, 0xe1);
        if size > 65_536 {
            unlock(&mut cart, 0xb0);
            cart.memory.write8(PARALLEL, 1);
            assert_eq!(cart.memory.read8(PARALLEL + 7), 0xff);
            program(&mut cart, 7, 0xd0);
            assert_eq!(
                (cart.battery_save()[7], cart.battery_save()[65_536 + 7]),
                (0x11, 0xd0)
            );
            unlock(&mut cart, 0xb0);
            cart.memory.write8(PARALLEL, 0);
        }
        unlock(&mut cart, 0x80);
        cart.memory.write8(PARALLEL + 0x5555, 0xaa);
        let saved = cart.save_state().unwrap();
        cart.memory.write8(PARALLEL + 0x2aaa, 0x55);
        cart.memory.write8(PARALLEL, 0x30);
        assert_eq!(cart.battery_save()[7], 0xff);
        assert_eq!(cart.battery_save()[0x1123], 0xe1);
        assert!(cart.take_battery_dirty());
        cart.load_state(&saved).unwrap();
        assert_eq!(cart.battery_save()[7], 0x11);
        // The pending erase handshake also survived restoration.
        cart.memory.write8(PARALLEL + 0x2aaa, 0x55);
        cart.memory.write8(PARALLEL + 0x5555, 0x10);
        assert!(cart.battery_save().iter().all(|&b| b == 0xff));
    }
}

fn dma(cart: &mut Machine, source: u32, destination: u32, count: u32) {
    cart.memory.write32(0x0400_00d4, source);
    cart.memory.write32(0x0400_00d8, destination);
    cart.memory.write32(0x0400_00dc, count | 0x8000_0000);
}

fn serial_command(cart: &mut Machine, width: u32, address: u32, data: Option<u64>) {
    let mut bits = vec![1, u16::from(data.is_none())];
    bits.extend((0..width).rev().map(|i| ((address >> i) & 1) as u16));
    if let Some(data) = data {
        bits.extend((0..64).rev().map(|i| ((data >> i) & 1) as u16));
    }
    bits.push(0);
    for (i, bit) in bits.iter().enumerate() {
        cart.memory.write16(RAM + i as u32 * 2, 0xfffe | bit);
    }
    dma(cart, RAM, SERIAL, bits.len() as u32);
}

#[test]
fn serial_eeprom_dma3_round_trips_raw_bytes_and_a_pending_read() {
    let data = 0xabc1_8002_127a_44e3_u64;
    for (width, address, size) in [(6, 0x3d, 512), (14, 0x3d0, 8192)] {
        let mut cart = machine(b"EEPROM_V124");
        serial_command(&mut cart, width, address, Some(data));
        assert_eq!(cart.memory.read16(SERIAL), 0);
        assert!(cart.take_battery_dirty());
        let offset = address as usize * 8;
        assert_eq!(cart.battery_save().len(), size);
        assert_eq!(cart.battery_save()[offset..offset + 8], data.to_be_bytes());
        cart.memory.backup.tick(super::EEPROM_WRITE_CYCLES);
        assert_eq!(cart.memory.read16(SERIAL), 1);
        let raw = cart.battery_save().to_vec();
        let mut fresh = machine(b"EEPROM_V124");
        fresh.restore_battery_save(&raw).unwrap();
        serial_command(&mut fresh, width, address, None);
        let state = fresh.save_state().unwrap();
        for _ in 0..2 {
            dma(&mut fresh, SERIAL, RAM, 68);
            assert!((0..4).all(|i| fresh.memory.read16(RAM + i * 2) == 0));
            let result = (4..68).fold(0_u64, |acc, i| {
                (acc << 1) | u64::from(fresh.memory.read16(RAM + i * 2))
            });
            assert_eq!(result, data);
            fresh.load_state(&state).unwrap();
        }
        assert!(fresh.restore_battery_save(&[0; 32]).is_err());
        assert_eq!(fresh.battery_save(), raw);
    }
}

#[test]
fn large_rom_only_maps_eeprom_in_its_last_256_bytes() {
    let mut rom = vec![0; 0x0100_0002];
    rom[0x100_0000..].copy_from_slice(&[0x91, 0x73]);
    rom[..11].copy_from_slice(b"EEPROM_V124");
    let cart = Machine::from_rom(rom).unwrap();
    assert_eq!(cart.memory.read16(0x0d00_0000), 0x7391);
    assert_eq!(cart.memory.read16(SERIAL), 1);
}

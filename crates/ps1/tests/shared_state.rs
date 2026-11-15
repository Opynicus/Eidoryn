use eidoryn_ps1::{
    bus::BIOS_SIZE,
    machine::Machine,
    memory_card::{CARD_SIZE, MemoryCard, SECTOR_SIZE},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[test]
fn rewind_bios_is_shared_until_a_host_edit() {
    let mut machine = Machine::from_bios(&vec![0x11; BIOS_SIZE], "Shared BIOS").unwrap();
    let mut first = machine.rewind_state().unwrap().into_machine();
    let second = machine.rewind_state().unwrap().into_machine();
    assert!(Arc::ptr_eq(&machine.bus.bios, &first.bus.bios));
    assert!(Arc::ptr_eq(&first.bus.bios, &second.bus.bios));

    Arc::make_mut(&mut machine.bus.bios)[10] = 0x22;
    assert!(!Arc::ptr_eq(&machine.bus.bios, &first.bus.bios));
    assert_eq!(first.bus.bios[10], 0x11);
    Arc::make_mut(&mut first.bus.bios)[10] = 0x33;
    assert!(!Arc::ptr_eq(&first.bus.bios, &second.bus.bios));
    assert_eq!(machine.bus.bios[10], 0x22);
    assert_eq!(first.bus.bios[10], 0x33);
    assert_eq!(second.bus.bios[10], 0x11);
}

fn write_sector(
    card: &mut MemoryCard,
    sector: u16,
    bytes: &[u8; SECTOR_SIZE],
    valid_checksum: bool,
) -> u8 {
    let [high, low] = sector.to_be_bytes();
    let checksum = bytes.iter().fold(high ^ low, |sum, byte| sum ^ byte);
    card.select();
    for byte in [0x57, 0, 0, high, low]
        .into_iter()
        .chain(bytes.iter().copied())
    {
        card.exchange(byte);
    }
    card.exchange(checksum ^ u8::from(!valid_checksum));
    card.exchange(0);
    card.exchange(0);
    card.exchange(0).0
}

#[test]
fn card_clones_detach_only_when_a_valid_write_changes_content() {
    let original = MemoryCard::formatted();
    let sector = 7;
    let start = usize::from(sector) * SECTOR_SIZE;
    let previous = original.bytes()[start..start + SECTOR_SIZE]
        .try_into()
        .unwrap();
    let mut same = original.clone();
    assert_eq!(write_sector(&mut same, sector, &previous, true), 0x47);
    assert!(std::ptr::eq(original.bytes(), same.bytes()));
    assert_eq!(same.revision(), original.revision());
    assert_eq!(same.writes, 1);
    assert_eq!(original.writes, 0);

    let mut rejected = original.clone();
    assert_eq!(
        write_sector(&mut rejected, sector, &[0x42; SECTOR_SIZE], false),
        0x4e
    );
    assert_eq!(
        write_sector(&mut rejected, 1024, &[0x42; SECTOR_SIZE], true),
        0xff
    );
    assert!(std::ptr::eq(original.bytes(), rejected.bytes()));
    assert_eq!(rejected.revision(), original.revision());
    assert_eq!(rejected.writes, 0);

    let mut changed = original.clone();
    assert_eq!(
        write_sector(&mut changed, sector, &[0x42; SECTOR_SIZE], true),
        0x47
    );
    assert!(!std::ptr::eq(original.bytes(), changed.bytes()));
    assert_eq!(
        &changed.bytes()[start..start + SECTOR_SIZE],
        &[0x42; SECTOR_SIZE]
    );
    assert_eq!(&original.bytes()[start..start + SECTOR_SIZE], &previous);
    assert_eq!(changed.revision(), original.revision() + 1);

    let mut restored = original.clone();
    assert_eq!(
        write_sector(&mut restored, sector, &[0x24; SECTOR_SIZE], true),
        0x47
    );
    assert_eq!(
        &restored.bytes()[start..start + SECTOR_SIZE],
        &[0x24; SECTOR_SIZE]
    );
    assert_eq!(
        &changed.bytes()[start..start + SECTOR_SIZE],
        &[0x42; SECTOR_SIZE]
    );
    assert_eq!(&original.bytes()[start..start + SECTOR_SIZE], &previous);
}

#[test]
fn shared_storage_preserves_the_original_v1_save_bytes() {
    let bios: Vec<u8> = (0..BIOS_SIZE).map(|n| (n as u8).wrapping_mul(17)).collect();
    let mut machine = Machine::from_bios(&bios, "V1 COW compatibility").unwrap();
    machine.cpu.registers[5] = 0x1234_5678;
    machine.bus.ram[1024..2048].fill(0x35);
    machine.bus.gpu.vram_mut()[137] = 0x4352;
    machine.bus.gpu.latch_display();
    for (slot, card) in machine.bus.sio.cards.iter_mut().enumerate() {
        let bytes: Vec<u8> = (0..CARD_SIZE)
            .map(|n| (n as u8).wrapping_mul(7).wrapping_add(slot as u8))
            .collect();
        *card = Some(MemoryCard::from_bytes(bytes).unwrap());
    }

    let bytes = machine.save_state().unwrap();
    // Captured from the Vec-backed V1 implementation at commit 43317b4.
    assert_eq!(bytes.len(), 4_648_657);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "ea92360ecf6051880daeec9c8bdc04e51383e158cc3767c800e1b7d80ab8bece"
    );
    let restored = Machine::load_state(&bytes).unwrap();
    assert_eq!(restored.save_state().unwrap(), bytes);
}

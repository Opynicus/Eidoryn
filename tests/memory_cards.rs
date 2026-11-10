use eidoryn_psx::{
    card_file::CardStore,
    machine::Machine,
    memory_card::{CARD_ACK_DELAY, CARD_SIZE, MemoryCard, READ_ACK_DELAY, SECTOR_SIZE},
    runner::{Command, Runner, Snapshot},
    sio::{ACK_PULSE, Sio},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const DATA: u32 = 0x1f80_1040;
const MODE: u32 = DATA + 8;
const CTRL: u32 = DATA + 10;
const BAUD: u32 = DATA + 14;
const BYTE: u32 = 0x88 * 8;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "eidoryn-card-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn card(&self) -> PathBuf {
        self.0.join("card.mcd")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn payload() -> [u8; 128] {
    std::array::from_fn(|i| (i * 73 + 19) as u8)
}

fn write_packet(sector: u16, data: &[u8; 128], bad_checksum: bool) -> Vec<u8> {
    let [high, low] = sector.to_be_bytes();
    let checksum = data.iter().fold(high ^ low, |sum, byte| sum ^ byte);
    let mut bytes = vec![0x57, 0, 0, high, low];
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(&[checksum ^ u8::from(bad_checksum), 0, 0, 0]);
    bytes
}

fn write_card(
    card: &mut MemoryCard,
    sector: u16,
    data: &[u8; 128],
    bad_checksum: bool,
) -> Vec<(u8, Option<u32>)> {
    card.select();
    write_packet(sector, data, bad_checksum)
        .into_iter()
        .map(|byte| card.exchange(byte))
        .collect()
}

fn read_card(card: &mut MemoryCard, sector: u16) -> Vec<(u8, Option<u32>)> {
    card.select();
    let [high, low] = sector.to_be_bytes();
    let mut bytes = vec![0x52, 0, 0, high, low];
    bytes.resize(139, 0);
    bytes.into_iter().map(|byte| card.exchange(byte)).collect()
}

fn sio() -> Sio {
    let mut sio = Sio::default();
    sio.write(MODE, 2, 0x0d);
    sio.write(BAUD, 2, 0x88);
    sio.write(CTRL, 2, 0x1003);
    sio
}

fn serial(sio: &mut Sio, byte: u8) -> u8 {
    sio.write(DATA, 1, u32::from(byte));
    sio.tick(BYTE);
    let reply = sio.read(DATA, 1) as u8;
    sio.tick(READ_ACK_DELAY + ACK_PULSE);
    reply
}

#[test]
fn formatted_card_has_valid_directory_and_can_report_its_identity() {
    let mut card = MemoryCard::formatted();
    assert_eq!(card.bytes().len(), CARD_SIZE);
    assert_eq!(&card.bytes()[..2], b"MC");
    for frame in card.bytes()[..36 * 128].as_chunks::<128>().0 {
        assert_eq!(
            frame[..127].iter().fold(0, |sum, byte| sum ^ byte),
            frame[127]
        );
    }
    for frame in card.bytes()[128..16 * 128].as_chunks::<128>().0 {
        assert_eq!(frame[0], 0xa0);
        assert_eq!(&frame[8..10], &[0xff; 2]);
    }
    assert_eq!(&card.bytes()[16 * 128..16 * 128 + 4], &[0xff; 4]);
    card.select();
    let replies = [0x53, 0, 0, 0, 0, 0, 0, 0, 0].map(|byte| card.exchange(byte));
    assert_eq!(
        replies.map(|r| r.0),
        [8, 0x5a, 0x5d, 0x5c, 0x5d, 4, 0, 0, 0x80]
    );
    assert!(replies[..8].iter().all(|r| r.1 == Some(CARD_ACK_DELAY)));
    assert_eq!(replies[8].1, None);
}

#[test]
fn sector_write_read_echo_checksum_flags_and_end_ack_are_correct() {
    let mut card = MemoryCard::formatted();
    let data = payload();
    let written = write_card(&mut card, 1023, &data, false);
    assert_eq!(written[0].0, 8);
    assert_eq!(written[4].0, 3); // Echo address high.
    assert_eq!(written[5].0, 0xff); // Echo address low.
    assert_eq!(written[133].0, data[127]);
    assert_eq!(
        &written[134..].iter().map(|r| r.0).collect::<Vec<_>>(),
        &[0x5c, 0x5d, 0x47]
    );
    assert_eq!(written.last().unwrap().1, None);
    assert_eq!(card.writes, 1);
    assert_eq!(card.revision(), 1);
    let read = read_card(&mut card, 1023);
    assert_eq!(read[0].0, 0); // Successful write clears insertion flag.
    assert_eq!(read[5], (0x5c, Some(READ_ACK_DELAY)));
    assert_eq!(read[7].0, 3);
    assert_eq!(read[8].0, 0xff);
    assert_eq!(read[9..137].iter().map(|r| r.0).collect::<Vec<_>>(), data);
    assert_eq!(
        read[137].0,
        data.iter().fold(3 ^ 0xff, |sum, byte| sum ^ byte)
    );
    assert_eq!(read[138], (0x47, None));
    assert_eq!(card.reads, 1);
    assert_eq!(card.exchange(0x52), (0xff, None));
}

#[test]
fn rejected_and_interrupted_writes_never_modify_storage() {
    let mut card = MemoryCard::formatted();
    let before = card.bytes().to_vec();
    assert_eq!(
        write_card(&mut card, 12, &payload(), true)
            .last()
            .unwrap()
            .0,
        0x4e
    );
    assert_eq!(
        write_card(&mut card, 1024, &payload(), false)
            .last()
            .unwrap()
            .0,
        0xff
    );
    card.select();
    for byte in write_packet(12, &payload(), false).into_iter().take(80) {
        card.exchange(byte);
    }
    card.deselect();
    assert_eq!(card.exchange(0), (0xff, None));
    assert_eq!(card.bytes(), before);
    assert_eq!((card.writes, card.revision()), (0, 0));
    let invalid_read = read_card(&mut card, 0x400);
    assert_eq!(invalid_read[7], (0xff, Some(CARD_ACK_DELAY)));
    assert_eq!(invalid_read[8], (0xff, None));
    assert!(invalid_read[9..].iter().all(|r| *r == (0xff, None)));
    card.select();
    assert_eq!(card.exchange(0x99), (12, None));
}

#[test]
fn both_sio_slots_work_and_controller_polling_survives_card_transactions() {
    let mut sio = sio();
    sio.cards = [Some(MemoryCard::formatted()), Some(MemoryCard::formatted())];
    for (slot, byte) in [(0, 0x37), (1, 0x92)] {
        let control = 0x1003 | (slot << 13);
        sio.write(CTRL, 2, control & !2);
        sio.write(CTRL, 2, control);
        assert_eq!(serial(&mut sio, 0x81), 0xff);
        let replies: Vec<_> = write_packet(64, &[byte; 128], false)
            .into_iter()
            .map(|b| serial(&mut sio, b))
            .collect();
        assert_eq!(*replies.last().unwrap(), 0x47);
        assert_eq!(
            sio.cards[slot as usize].as_ref().unwrap().bytes()[8192],
            byte
        );
    }
    sio.write(CTRL, 2, 1);
    sio.write(CTRL, 2, 0x1003);
    assert_eq!(
        [1, 0x42, 0, 0, 0].map(|b| serial(&mut sio, b)),
        [0xff, 0x41, 0x5a, 0xff, 0xff]
    );
    sio.write(CTRL, 2, 0x40);
    assert_eq!(sio.cards[0].as_ref().unwrap().bytes()[8192], 0x37);
    assert_eq!(sio.cards[1].as_ref().unwrap().bytes()[8192], 0x92);
}

#[test]
fn memory_read_ack_has_its_own_delay_and_final_byte_has_no_irq() {
    let mut sio = sio();
    sio.cards[0] = Some(MemoryCard::formatted());
    for byte in [0x81, 0x52, 0, 0, 0, 0] {
        serial(&mut sio, byte);
    }
    sio.take_irq_edge();
    sio.write(CTRL, 2, 0x1013);
    sio.write(DATA, 1, 0);
    sio.tick(BYTE);
    assert_eq!(sio.read(DATA, 1), 0x5c);
    sio.tick(READ_ACK_DELAY - 1);
    assert!(!sio.take_irq_edge());
    sio.tick(1);
    assert!(sio.take_irq_edge());
    sio.tick(ACK_PULSE);
    for _ in 6..138 {
        serial(&mut sio, 0);
    }
    sio.take_irq_edge();
    sio.write(CTRL, 2, 0x1013);
    assert_eq!(serial(&mut sio, 0), 0x47);
    assert!(!sio.take_irq_edge());
}

#[test]
fn persistent_cards_reload_and_remain_exclusively_locked_across_atomic_replacement() {
    let fixture = Fixture::new();
    let (mut store, mut cards) = CardStore::open([Some(fixture.card()), None]).unwrap();
    write_card(cards[0].as_mut().unwrap(), 511, &payload(), false);
    assert!(store.status(&cards)[0].as_ref().unwrap().pending);
    store.flush(&cards).unwrap();
    assert!(!store.status(&cards)[0].as_ref().unwrap().pending);
    assert!(CardStore::open([Some(fixture.card()), None]).is_err());
    drop(store);
    let (_store, cards) = CardStore::open([Some(fixture.card()), None]).unwrap();
    assert_eq!(
        &cards[0].as_ref().unwrap().bytes()[511 * 128..512 * 128],
        payload()
    );
}

#[test]
fn invalid_existing_cards_and_duplicate_slot_paths_are_never_reformatted() {
    let fixture = Fixture::new();
    std::fs::write(fixture.card(), b"old save: wrong format").unwrap();
    assert!(CardStore::open([Some(fixture.card()), None]).is_err());
    assert_eq!(
        std::fs::read(fixture.card()).unwrap(),
        b"old save: wrong format"
    );
    std::fs::write(fixture.card(), MemoryCard::formatted().bytes()).unwrap();
    assert!(CardStore::open([Some(fixture.card()), Some(fixture.card())]).is_err());
    assert_eq!(&std::fs::read(fixture.card()).unwrap()[..2], b"MC");
}

#[test]
fn failed_atomic_save_keeps_dirty_data_and_can_be_retried() {
    let fixture = Fixture::new();
    let (mut store, mut cards) = CardStore::open([Some(fixture.card()), None]).unwrap();
    let original = std::fs::read(fixture.card()).unwrap();
    let backup = fixture.0.join("original.mcd");
    std::fs::rename(fixture.card(), &backup).unwrap();
    std::fs::create_dir(fixture.card()).unwrap();
    write_card(cards[0].as_mut().unwrap(), 123, &payload(), false);
    assert!(store.flush(&cards).is_err());
    assert!(store.status(&cards)[0].as_ref().unwrap().pending);
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|ext| ext == "tmp")
    }));
    std::fs::remove_dir(fixture.card()).unwrap();
    std::fs::rename(backup, fixture.card()).unwrap();
    store.flush(&cards).unwrap();
    assert_eq!(
        &std::fs::read(fixture.card()).unwrap()[123 * 128..124 * 128],
        payload()
    );
}

fn wait(runner: &Runner, condition: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(snapshot) = runner.take_snapshot()
            && condition(&snapshot)
        {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "worker timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn worker_reset_and_media_replacement_keep_newly_written_card_data() {
    let fixture = Fixture::new();
    let (store, cards) = CardStore::open([Some(fixture.card()), None]).unwrap();
    let mut machine = Machine::demo();
    machine.bus.sio = sio();
    machine.bus.sio.cards = cards;
    serial(&mut machine.bus.sio, 0x81);
    let packet = write_packet(123, &payload(), false);
    for &byte in &packet[..133] {
        serial(&mut machine.bus.sio, byte);
    }
    // Commit the sector during emulated CPU execution, after the reset baseline is captured.
    machine.bus.sio.write(BAUD, 2, 2);
    machine.bus.sio.write(DATA, 1, u32::from(packet[133]));
    let runner = Runner::with_card_store(machine, false, store).unwrap();
    runner.send(Command::Slice).unwrap();
    wait(&runner, |s| {
        s.cards[0]
            .as_ref()
            .is_some_and(|c| c.writes == 1 && !c.pending)
    });
    runner.send(Command::Reset).unwrap();
    let snapshot = wait(&runner, |s| s.cpu.instructions == 0);
    assert_eq!(snapshot.cards[0].as_ref().unwrap().writes, 1);
    runner.send(Command::Demo).unwrap();
    let snapshot = wait(&runner, |s| s.running);
    assert_eq!(snapshot.cards[0].as_ref().unwrap().writes, 1);
    drop(runner);
    assert_eq!(
        &std::fs::read(fixture.card()).unwrap()[123 * 128..124 * 128],
        payload()
    );
}

#[test]
fn close_waits_for_failed_save_to_be_retried() {
    let fixture = Fixture::new();
    let (store, cards) = CardStore::open([Some(fixture.card()), None]).unwrap();
    let backup = fixture.0.join("original.mcd");
    std::fs::rename(fixture.card(), &backup).unwrap();
    std::fs::create_dir(fixture.card()).unwrap();
    let mut machine = Machine::demo();
    machine.bus.sio.cards = cards;
    write_card(
        machine.bus.sio.cards[0].as_mut().unwrap(),
        64,
        &payload(),
        false,
    );
    let runner = Runner::with_card_store(machine, false, store).unwrap();
    runner.send(Command::RequestClose).unwrap();
    let snapshot = wait(&runner, |s| s.message.is_some());
    assert!(!snapshot.close_ready);
    assert!(snapshot.cards[0].as_ref().unwrap().pending);
    std::fs::remove_dir(fixture.card()).unwrap();
    std::fs::rename(backup, fixture.card()).unwrap();
    runner.send(Command::FlushCards).unwrap();
    wait(&runner, |s| {
        s.message.is_none() && !s.cards[0].as_ref().unwrap().pending
    });
    runner.send(Command::RequestClose).unwrap();
    wait(&runner, |s| s.close_ready);
    assert_eq!(
        &std::fs::read(fixture.card()).unwrap()[64 * SECTOR_SIZE..65 * SECTOR_SIZE],
        payload()
    );
}

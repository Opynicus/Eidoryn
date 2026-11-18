use super::*;
use crate::{cpu::Cpu, cpu_engine::CpuMode, demo, gpu::RendererMode, memory_card::MemoryCard};

#[test]
fn rewind_charge_tracks_audio_and_is_stable_for_shared_cards() {
    let mut machine = Machine::demo();
    machine.bus.sio.cards[0] = Some(MemoryCard::formatted());
    let first = machine.rewind_state().unwrap();
    let second = machine.rewind_state().unwrap();
    assert_eq!(first.memory_size(), second.memory_size());
    let third = machine.rewind_state().unwrap();
    assert_eq!(first.memory_size(), third.memory_size());
    machine.bus.tick(crate::spu::SAMPLE_CLOCKS * 1024).unwrap();
    let with_audio = machine.rewind_state().unwrap().memory_size();
    let queued = machine.bus.spu.drain_audio().count();
    let without_audio = machine.rewind_state().unwrap().memory_size();
    assert!(with_audio >= without_audio + 2 * queued * size_of::<crate::spu::StereoFrame>());
    assert_eq!(first.memory_size(), second.memory_size());
}

#[test]
fn rewind_preserves_guest_state_without_retaining_host_backends() {
    let mut machine = Machine::demo();
    machine.bus.sio.cards[0] = Some(MemoryCard::formatted());
    machine
        .bus
        .gpu
        .set_renderer(RendererMode::Threaded)
        .unwrap();
    #[cfg(all(feature = "jit", target_os = "linux", target_arch = "x86_64"))]
    machine.set_cpu_mode(CpuMode::Jit).unwrap();
    machine.run_slice().unwrap();
    let bytes = machine.save_state().unwrap();
    let state = machine.rewind_state().unwrap();
    assert!(state.memory_size() > bytes.len());
    assert!(state.memory_size() < 32 * 1024 * 1024);
    assert_eq!(machine.bus.gpu.renderer_mode(), RendererMode::Threaded);
    let cpu_mode = machine.cpu_mode();
    machine.run_slice().unwrap();
    let mut restored = state.into_machine();
    assert_eq!(restored.cpu_mode(), CpuMode::Interpreter);
    assert_eq!(restored.bus.gpu.renderer_mode(), RendererMode::Serial);
    assert_eq!(restored.save_state().unwrap(), bytes);
    restored.set_cpu_mode(cpu_mode).unwrap();
    restored
        .bus
        .gpu
        .set_renderer(RendererMode::Threaded)
        .unwrap();
    let mut reference = Machine::load_state(&bytes).unwrap();
    restored.run_slice().unwrap();
    reference.run_slice().unwrap();
    assert_eq!(
        restored.save_state().unwrap(),
        reference.save_state().unwrap()
    );
}

#[test]
fn rewind_keeps_mounted_media_without_reopening_the_path() {
    use crate::{
        cdrom::{COMMAND_CLOCKS, SECTOR_CLOCKS},
        disc::DiscImage,
    };
    let path = std::env::temp_dir().join(format!(
        "eidoryn-rewind-disc-{}-{}.iso",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let moved = path.with_extension("moved");
    let data: Vec<u8> = (0..6 * 2048).map(|index| (index / 2048) as u8).collect();
    std::fs::write(&path, data).unwrap();
    let mut original = Machine::demo();
    original.bus.cdrom.insert(DiscImage::open(&path).unwrap());
    original.bus.cdrom.write(0, 1);
    original.bus.cdrom.write(2, 0x1f);
    original.bus.cdrom.write(0, 0);
    original.bus.cdrom.write(1, 6);
    std::fs::rename(&path, &moved).unwrap();
    let state = original.rewind_state().unwrap();
    let bytes = original.save_state().unwrap();
    assert!(Machine::load_state(&bytes).is_err());
    let mut restored = state.into_machine();
    assert_eq!(restored.save_state().unwrap(), bytes);
    for sector in 0..2 {
        for machine in [&mut original, &mut restored] {
            if sector == 0 {
                machine.bus.tick(COMMAND_CLOCKS).unwrap();
                machine.bus.cdrom.write(0, 1);
                machine.bus.cdrom.write(3, 0x1f);
                machine.bus.cdrom.write(0, 0);
            }
            machine.bus.tick(SECTOR_CLOCKS).unwrap();
            machine.bus.cdrom.write(3, 0x80);
            assert_eq!(machine.bus.cdrom.data_available(), 2048);
            for _ in 0..2048 {
                assert_eq!(machine.bus.cdrom.read_data(), sector);
            }
            machine.bus.cdrom.write(0, 1);
            machine.bus.cdrom.write(3, 0x1f);
            machine.bus.cdrom.write(0, 0);
        }
    }
    assert_eq!(
        restored.save_state().unwrap(),
        original.save_state().unwrap()
    );
    drop(original);
    drop(restored);
    std::fs::remove_file(moved).unwrap();
}

fn pipeline_machine() -> Machine {
    let mut machine = Machine::from_exe(&demo::executable(), "State test").unwrap();
    machine.cpu = Cpu::new(0x8001_0000);
    // Load delay, taken branch delay slot, then unfinished multiplication.
    for (index, word) in [
        0x8c01_2000,
        0x1000_0001,
        0x2402_0007,
        0x0022_0018,
        0x0000_1812,
    ]
    .into_iter()
    .enumerate()
    {
        machine
            .bus
            .write32(0x8001_0000 + index as u32 * 4, word)
            .unwrap();
    }
    machine.bus.write32(0x2000, 3).unwrap();
    machine.bus.write32(0x1f80_0000, 0x1234_5678).unwrap();
    machine.bus.write16(0x1f80_1108, 317).unwrap();
    machine.bus.write16(0x1f80_1104, 0x58).unwrap();
    machine.bus.cdrom.write(1, 0x01); // Command still waiting to complete.
    machine.bus.sio.write(0x1f80_1048, 2, 0x0c);
    machine.bus.sio.write(0x1f80_104a, 2, 0x1003);
    machine.bus.sio.write(0x1f80_104e, 2, 136);
    machine.bus.sio.write(0x1f80_1040, 1, 1); // Partially shifted pad byte.
    machine.bus.mdec.write_data(0x4000_0001).unwrap();
    machine.bus.mdec.write_data(0x1234_5678).unwrap();
    machine.bus.spu.write(0x1f80_1dac, 4).unwrap();
    machine.bus.spu.write(0x1f80_1da6, 0x200).unwrap();
    machine.bus.spu.write(0x1f80_1da8, 0x7777).unwrap();
    machine.bus.spu.write(0x1f80_1daa, 0x8010).unwrap();
    machine.bus.sio.cards[0] = Some(MemoryCard::formatted());
    let card = machine.bus.sio.cards[0].as_mut().unwrap();
    card.select();
    for byte in [0x57, 0, 0, 0, 2, 17, 23] {
        card.exchange(byte);
    }
    machine
}

fn finish_pipeline_transaction(candidate: &mut Machine) {
    candidate.run_steps(1).unwrap();
    candidate.bus.tick(3000).unwrap();
    for _ in 0..31 {
        candidate.bus.mdec.write_data(0x1111_1111).unwrap();
    }
    let card = candidate.bus.sio.cards[0].as_mut().unwrap();
    for _ in 0..126 {
        card.exchange(0);
    }
    for byte in [2 ^ 0x11 ^ 0x17, 0, 0, 0] {
        card.exchange(byte);
    }
}

#[test]
fn save_state_resumes_pipeline_devices_audio_and_card_transaction() {
    let mut machine = pipeline_machine();
    for steps in [1, 1, 2] {
        machine.run_steps(steps).unwrap();
        let bytes = machine.save_state().unwrap();
        let mut restored = Machine::load_state(&bytes).unwrap();
        assert_eq!(restored.save_state().unwrap(), bytes);
        let mut reference = machine.clone();
        finish_pipeline_transaction(&mut reference);
        finish_pipeline_transaction(&mut restored);
        assert_eq!(
            restored.save_state().unwrap(),
            reference.save_state().unwrap()
        );
        assert_eq!(restored.bus.sio.cards[0].as_ref().unwrap().writes, 1);
        assert!(restored.bus.spu.generated_samples > 0);
    }
}

#[test]
fn frozen_v1_pipeline_matches_before_and_after_resuming() {
    use std::io::Read;

    // a3142a1, after the load and branch, before executing the delay slot.
    let mut legacy = Vec::new();
    flate2::read::GzDecoder::new(&include_bytes!("../../tests/fixtures/pipeline-v1.bin.gz")[..])
        .read_to_end(&mut legacy)
        .unwrap();
    let mut original = pipeline_machine();
    original.run_steps(2).unwrap();
    assert!(
        original.save_state().unwrap() == legacy,
        "V1 encoding changed"
    );

    let mut restored = Machine::load_state(&legacy).unwrap();
    assert!(
        restored.save_state().unwrap() == legacy,
        "V1 restoration changed"
    );
    finish_pipeline_transaction(&mut restored);
    // Whole-file digest from running the same continuation on a3142a1.
    assert_eq!(
        format!("{:x}", Sha256::digest(restored.save_state().unwrap())),
        "abf4020cff6095fddfcfd76766a47d9b94a3b7b40d23f329e439129ab254a982"
    );
}

#[test]
fn save_state_preserves_voice_decoder_and_audio_output() {
    let mut machine = Machine::demo();
    let spu = &mut machine.bus.spu;
    spu.write(0x1f80_1dac, 4).unwrap();
    spu.write(0x1f80_1da6, 0x200).unwrap();
    for word in [0x4444_0700, 0x4444_4444, 0x4444_4444, 0x4444_4444] {
        spu.dma_write(word).unwrap();
    }
    for (register, value) in [
        (0, 0x3fff),
        (2, 0x3fff),
        (4, 0x1000),
        (6, 0x200),
        (8, 0x7fff),
        (10, 0x1fc0),
        (0x180, 0x3fff),
        (0x182, 0x3fff),
        (0x1aa, 0xc000),
        (0x188, 1),
        (12, 0x4000),
    ] {
        spu.write(0x1f80_1c00 + register, value).unwrap();
    }
    machine.bus.tick(768 * 19 + 317).unwrap();
    let mut restored = Machine::load_state(&machine.save_state().unwrap()).unwrap();
    machine.bus.tick(768 * 39).unwrap();
    restored.bus.tick(768 * 39).unwrap();
    let expected = machine.bus.spu.drain_audio().collect::<Vec<_>>();
    assert!(expected.iter().any(|sample| *sample != [0; 2]));
    assert_eq!(restored.bus.spu.drain_audio().collect::<Vec<_>>(), expected);
    assert_eq!(
        restored.save_state().unwrap(),
        machine.save_state().unwrap()
    );
}

#[test]
fn save_state_rejects_damage_versions_and_invalid_hardware() {
    let bytes = Machine::demo().save_state().unwrap();
    for length in [0, 8, 43, bytes.len() - 1] {
        assert!(Machine::load_state(&bytes[..length]).is_err());
    }
    let mut damaged = bytes.clone();
    damaged[HEADER_SIZE + 100] ^= 1;
    assert!(Machine::load_state(&damaged).is_err());
    let mut newer = bytes;
    newer[8] = 2;
    assert!(Machine::load_state(&newer).is_err());
    let mut invalid = Machine::demo();
    invalid.bus.ram.clear();
    let bytes = invalid.save_state().unwrap();
    assert!(Machine::load_state(&bytes).is_err());
    assert!(bincode::deserialize::<Machine>(&bytes[HEADER_SIZE..]).is_err());
    assert!(
        bincode::deserialize::<crate::bus::Bus>(&bincode::serialize(&invalid.bus).unwrap())
            .is_err()
    );
    let mut halted = Machine::demo();
    halted.halted = Some(crate::EmuError::Dma("chopping requires CD-ROM input"));
    let restored = Machine::load_state(&halted.save_state().unwrap()).unwrap();
    assert_eq!(restored.halted, halted.halted);
}

#[test]
fn save_state_resumes_mdec_table_transfers_and_completed_idle() {
    for (command, word_count) in [(0x4000_0000, 16), (0x4000_0001, 32), (0x6000_0000, 32)] {
        let words: Vec<u32> = (0..word_count)
            .map(|index| 0x0302_0100 + index as u32 * 0x0404_0404)
            .collect();
        for written in [0, word_count / 2, word_count - 1, word_count] {
            let context = format!("command {command:#010x}, {written}/{word_count} words");
            let mut original = Machine::demo();
            original.bus.write32(0x1f80_1820, command).unwrap();
            for &word in &words[..written] {
                original.bus.write32(0x1f80_1820, word).unwrap();
            }
            assert_eq!(
                original.bus.mdec.status() & (1 << 29) != 0,
                written < word_count,
                "{context}"
            );
            let bytes = original.save_state().unwrap();
            let mut restored =
                Machine::load_state(&bytes).unwrap_or_else(|error| panic!("{context}: {error}"));
            assert_eq!(restored.save_state().unwrap(), bytes, "{context}");

            for machine in [&mut original, &mut restored] {
                for &word in &words[written..] {
                    machine.bus.write32(0x1f80_1820, word).unwrap();
                }
                assert_eq!(machine.bus.mdec.status() & (1 << 29), 0, "{context}");
                // A restored completed transfer must accept the next command.
                machine.bus.write32(0x1f80_1820, 0x2800_0001).unwrap();
                machine.bus.write32(0x1f80_1820, 0xfe00_0480).unwrap();
                assert_eq!(machine.bus.mdec.commands, 2, "{context}");
                assert_eq!(machine.bus.mdec.blocks_decoded, 1, "{context}");
                machine.run_steps(16).unwrap();
            }
            assert_eq!(
                restored.save_state().unwrap(),
                original.save_state().unwrap(),
                "continued execution: {context}"
            );
        }
    }
}

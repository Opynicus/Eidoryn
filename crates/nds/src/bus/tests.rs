use super::{Memory, Port};
use eidoryn_arm::Bus;

#[test]
fn repeated_single_word_card_dma_copies_each_sector_and_raises_completion_irq() {
    // The SDK arms a repeated one-word transfer and waits for the card IRQ.
    let mut rom = vec![0; 0x8200];
    for (i, byte) in rom[0x8000..].iter_mut().enumerate() {
        *byte = (i ^ (i >> 8)) as u8;
    }
    for late_dma in [false, true] {
        let mut mem = Memory::default();
        let mut p = Port::<true> {
            m: &mut mem,
            rom: &rom,
        };
        let control = 0xaf00_0001;
        p.write32(0x0400_00c8, 0x0410_0010);
        p.write32(0x0400_00cc, 0x0200_0000);
        if !late_dma {
            p.write32(0x0400_00d0, control);
        }
        p.write16(0x0400_01a0, 0xc000);
        p.write32(0x0400_01a8, 0x8000_00b7);
        p.write32(0x0400_01a4, 0x8100_0000);
        if late_dma {
            p.write32(0x0400_00d0, control);
        }
        for _ in 0..127 {
            p.dma();
        }
        assert_eq!(p.read32(0x0400_0214) & (1 << 19), 0);
        p.dma();
        assert_ne!(p.read32(0x0400_0214) & (1 << 19), 0);
        assert_eq!(p.read32(0x0400_01a4) & 0x8080_0000, 0);
        p.dma();
        assert_eq!(&p.m.main[..0x200], &rom[0x8000..]);
        assert_eq!(p.read32(0x0200_0200), 0);
    }
}

#[test]
fn arm7_aux_spi_honors_chip_select_and_eeprom_page_programming() {
    let mut memory = Memory::default();
    let mut p = Port::<false> {
        m: &mut memory,
        rom: &[],
    };
    let mut transfer = |bytes: &[u8]| {
        let mut result = Vec::new();
        for (i, &byte) in bytes.iter().enumerate() {
            p.write16(
                0x0400_01a0,
                if i + 1 == bytes.len() { 0xa000 } else { 0xa040 },
            );
            p.write8(0x0400_01a2, byte);
            result.push(p.read8(0x0400_01a2));
        }
        result
    };
    assert_eq!(transfer(&[3, 0, 0x1f, 0])[3], 0xff);
    transfer(&[2, 0, 0x1f, 0x12]); // write disabled
    assert_eq!(transfer(&[3, 0, 0x1f, 0])[3], 0xff);
    transfer(&[6]);
    assert_eq!(transfer(&[5, 0])[1] & 2, 2);
    transfer(&[2, 0, 0x1f, 0x12, 0xab]); // wraps to the start of the 32-byte page
    assert_eq!(transfer(&[5, 0])[1] & 3, 0);
    assert_eq!(transfer(&[3, 0, 0x1f, 0, 0])[3..], [0x12, 0xff]);
    assert_eq!(transfer(&[3, 0, 0, 0])[3], 0xab);
    assert_eq!(memory.backup.data().unwrap().len(), 8192);
}

#[test]
fn card_id_and_rom_reads_respect_the_physical_chip_capacity() {
    let mut rom = vec![0; 0xa000];
    rom[0x14] = 9; // Trimmed image from a 64 MiB physical cartridge.
    rom[0x8ffc..0x9000].copy_from_slice(b"LAST");
    rom[0x8000..0x8004].copy_from_slice(b"WRAP");
    let mut m = Memory::default();
    let mut p = Port::<true> {
        m: &mut m,
        rom: &rom,
    };
    p.write8(0x0400_01a8, 0xb8);
    p.write32(0x0400_01a4, 0x8700_0000);
    assert_eq!(p.read32(0x0410_0010), 0x0000_3fc2);
    // Beyond 64 MiB mirrors the chip; stream data then wraps to the 4 KiB page.
    p.write32(0x0400_01a8, 0x8f00_04b7);
    p.write8(0x0400_01ac, 0xfc);
    p.write32(0x0400_01a4, 0x8100_0000);
    assert_eq!(p.read32(0x0410_0010).to_le_bytes(), *b"LAST");
    assert_eq!(p.read32(0x0410_0010).to_le_bytes(), *b"WRAP");
}

#[test]
fn arm7_wifi_packet_ram_registers_baseband_and_power_handshake() {
    let mut mem = Memory::default();
    let mut p = Port::<false> {
        m: &mut mem,
        rom: &[],
    };
    p.write16(0x0400_0304, 2); // POWCNT2 enables the wireless controller.
    let regs = 0x0480_8000;
    assert_eq!(p.read16(regs), 0x1440);
    p.write16(regs + 0x02a, u16::MAX);
    assert_eq!(p.read16(regs + 0x02a), 0x07ff);
    p.write32(0x0480_5ffc, 0x55aa_abcd);
    p.write8(0x0480_5ffc, 0); // The chip has no byte write enable.
    assert_eq!(p.read32(0x0480_dffc), 0x55aa_abcd);

    p.write16(regs + 0x15a, 0xa5);
    p.write16(regs + 0x158, 0x5013);
    p.write16(regs + 0x158, 0x6013);
    assert_eq!(p.read16(regs + 0x15c), 0xa5);
    p.write16(regs + 0x158, 0x6000);
    assert_eq!(p.read16(regs + 0x15c), 0x6d);

    p.write16(regs + 0x012, 1 << 11);
    p.write16(regs + 0x03c, 2);
    assert_ne!(p.read32(0x0400_0214) & (1 << 24), 0);
    p.write32(0x0400_0214, 1 << 24);
    p.write16(regs + 0x21c, 1 << 11);
    assert_eq!(p.read32(0x0400_0214) & (1 << 24), 0);
    p.write16(regs + 0x010, 1 << 11);
    p.write16(regs + 0x21c, 1 << 11);
    assert_ne!(p.read32(0x0400_0214) & (1 << 24), 0);
    p.write16(regs + 0x040, 0x8001);
    assert_eq!(p.read16(regs + 0x03c) >> 8, 2);
    assert_eq!(p.read16(regs + 0x214), 9);
    p.write16(regs + 0x040, 0x8000);
    assert_ne!(p.read16(regs + 0x03c) & 0x100, 0);
    p.write16(regs + 0x2d0, 0);
    assert_eq!(p.read16(regs + 0x03c) >> 8, 0);
}

const SOUND: u32 = 0x0400_0400;
const MASTER: u32 = SOUND + 0x100;
const WAVE: u32 = 0x0201_0000;
const ONE_SHOT: u32 = 2 << 27;
const PCM16: u32 = 1 << 29;
const ADPCM: u32 = 2 << 29;
const PSG: u32 = 3 << 29;
const KEY_ON: u32 = 1 << 31;

fn voice(p: &mut Port<'_, false>, id: u32, control: u32, loop_words: u16, words: u32) {
    let base = SOUND + id * 16;
    p.write32(base + 4, WAVE);
    p.write16(base + 8, 0xfe00); // One source step per 1024 ARM7 clocks, aligned to the DAC.
    p.write16(base + 10, loop_words);
    p.write32(base + 12, words);
    p.write32(base, control | KEY_ON | 127);
}
fn render(p: &mut Port<'_, false>, count: usize) -> Vec<[i16; 2]> {
    for _ in 0..count {
        p.m.advance(1024);
    }
    p.m.audio.take_samples()
}

#[test]
fn arm7_pcm_formats_use_signed_memory_and_can_be_mixed_to_separate_speakers() {
    let mut mem = Memory::default();
    let mut p = Port::<false> {
        m: &mut mem,
        rom: &[],
    };
    p.write16(MASTER, 0x807f);
    for i in 0..8 {
        p.write16(WAVE + i * 2, 0xc040);
    }
    voice(&mut p, 0, ONE_SHOT, 0, 4);
    voice(&mut p, 7, PCM16 | ONE_SHOT | (127 << 16), 0, 4);
    let out = render(&mut p, 12);
    assert_eq!(&out[..3], &[[0, 0]; 3]);
    assert_eq!(out[3], [16384, -16320]);
    assert_eq!(out[4], [-16384, -16320]);
    assert_eq!(out[11], [16384, 0]);
    assert_ne!(p.read32(SOUND) & KEY_ON, 0);
    assert_eq!(p.read32(SOUND + 7 * 16) & KEY_ON, 0);
    assert_eq!(p.read32(SOUND + 4), 0);
    let out = render(&mut p, 10);
    assert_eq!(out.last(), Some(&[0, 0]));
    assert_eq!(p.read32(SOUND) & KEY_ON, 0);
}

#[test]
fn adpcm_restores_predictor_at_the_word_loop_and_mixed_capture_writes_guest_ram() {
    let mut mem = Memory::default();
    let mut p = Port::<false> {
        m: &mut mem,
        rom: &[],
    };
    p.write16(MASTER, 0x807f);
    p.write16(WAVE, 1000);
    p.write8(WAVE + 2, 40);
    for i in 4..16 {
        p.write8(WAVE + i, 0x17);
    }
    voice(&mut p, 0, ADPCM | (1 << 27), 1, 3);
    let out = render(&mut p, 11 + 48);
    assert_eq!(&out[..11], &[[0, 0]; 11]);
    // Header predictor 1000, index 40 (step 337). First nibble 7 adds 631.
    assert_eq!(out[11], [1600, 0]);
    assert_eq!(&out[11..35], &out[35..59]);
    p.write16(SOUND + 16 + 8, 0xfe00);
    let target = 0x0380_1200;
    p.write32(MASTER + 0x10, target);
    p.write16(MASTER + 0x14, 1);
    p.write8(MASTER + 8, 0x84); // One word of left mixer, PCM16.
    let captured = render(&mut p, 2);
    assert_eq!(p.read8(MASTER + 8), 4);
    for (i, stereo) in captured.iter().enumerate() {
        assert_eq!(p.read16(target + i as u32 * 2) as i16 & !63, stereo[0]);
    }
    assert_ne!(p.read32(target), 0);
}

#[test]
fn pulse_and_noise_waveforms_follow_channel_timers_and_master_routing() {
    let mut mem = Memory::default();
    let mut p = Port::<false> {
        m: &mut mem,
        rom: &[],
    };
    p.write16(MASTER, 0x807f);
    voice(&mut p, 8, PSG | (3 << 24), 0, 0); // 50% duty to left
    voice(&mut p, 14, PSG | (127 << 16), 0, 0);
    let out = render(&mut p, 18);
    assert_eq!(out[0], [0, 0]);
    assert_eq!(
        out[1..9].iter().map(|v| v[0]).collect::<Vec<_>>(),
        [-32768, -32768, -32768, -32768, 32704, 32704, 32704, 32704]
    );
    assert!(out.iter().any(|v| v[1] < 0) && out.iter().any(|v| v[1] > 0));
    p.write16(MASTER, 0x857f); // Route silent channel 1 to each DAC.
    assert_eq!(render(&mut p, 3), [[0, 0]; 3]);
    p.write16(0x0400_0304, 0);
    p.write32(SOUND + 8 * 16, 0);
    assert_eq!(p.read32(SOUND + 8 * 16), 0);
    p.write16(0x0400_0304, 1);
    assert_ne!(p.read32(SOUND + 8 * 16) & KEY_ON, 0);
}

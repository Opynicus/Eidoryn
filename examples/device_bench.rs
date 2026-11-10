//! Original DMA-list and MDEC fixtures. No BIOS/disc needed; use a release build.
use eidoryn_psx::{bus::Bus, mdec::Mdec};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), eidoryn_psx::EmuError> {
    for round in 0..5 {
        let mut bus = Bus::default();
        bus.write32(0x1f80_10f0, 8 << 8)?;
        // Each node contains a legal no-op, so this isolates list traversal.
        for n in 0..1024 {
            let next = if n == 1023 {
                0x00ff_ffff
            } else {
                0x1000 + (n + 1) * 8
            };
            bus.write32(0x1000 + n * 8, 0x0100_0000 | next)?;
        }
        let start = Instant::now();
        for _ in 0..10_000 {
            bus.write32(0x1f80_10a0, 0x1000)?;
            bus.write32(0x1f80_10a8, 0x0100_0401)?;
        }
        println!(
            "DMA round={round} lists=10000 nodes=1024 elapsed_ms={:.3} commands={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(bus.gpu.commands)
        );

        let mut mdec = Mdec::default();
        mdec.write_data(0x6000_0000)?;
        for pair in 0..32 {
            mdec.write_data(if pair < 4 { 0x4000_4000 } else { 0 })?;
        }
        let start = Instant::now();
        let mut checksum = 0_u64;
        for _ in 0..20_000 {
            mdec.write_data(0x3000_0006)?;
            for dc in [128_u32, 896, 0, 128, 896, 256] {
                mdec.write_data(0xfe00_0000 | dc)?;
            }
            while mdec.status() & (1 << 31) == 0 {
                checksum = checksum.wrapping_add(u64::from(mdec.read_data()));
            }
        }
        println!(
            "MDEC round={round} macroblocks=20000 elapsed_ms={:.3} checksum={:016x}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
    }
    Ok(())
}

//! Original command/FIFO workload; no BIOS or game data. Use a release build.
use eidoryn_psx::{
    cdrom::{COMMAND_CLOCKS, CdRom, SECTOR_CLOCKS},
    disc::DiscImage,
};
use std::{hint::black_box, io::Write, path::PathBuf, time::Instant};

const SECTORS: usize = 8192;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "eidoryn-cd-bench-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        let fixture = Self(path);
        let mut output =
            std::io::BufWriter::new(std::fs::File::create_new(fixture.0.join("raw.bin"))?);
        let mut cooked =
            std::io::BufWriter::new(std::fs::File::create_new(fixture.0.join("cooked.iso"))?);
        let mut sector = [0_u8; 2352];
        sector[1..11].fill(0xff);
        sector[15] = 2;
        sector[18] = 8;
        sector[22] = 8;
        sector[24..2072].fill(90);
        for _ in 0..SECTORS {
            output.write_all(&sector)?;
            cooked.write_all(&sector[24..2072])?;
        }
        output.flush()?;
        cooked.flush()?;
        Ok(fixture)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn acknowledge(cd: &mut CdRom) {
    cd.write(0, 1);
    cd.write(3, 7);
    cd.write(0, 0);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cdrom_bytes={}", std::mem::size_of::<CdRom>());
    let fixture = Fixture::new()?;
    let disc = DiscImage::open(&fixture.0.join("raw.bin"))?;
    let cooked = DiscImage::open(&fixture.0.join("cooked.iso"))?;
    for round in 0..5 {
        let mut cd = CdRom::default();
        let mut checksum = 0_u64;
        let start = Instant::now();
        for _ in 0..1_000_000 {
            cd.write(1, 1); // Getstat has no parameters and produces one byte.
            cd.tick(COMMAND_CLOCKS);
            checksum += u64::from(cd.read(1));
            acknowledge(&mut cd);
        }
        println!(
            "commands round={round} count=1000000 elapsed_ms={:.3} checksum={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
        assert_eq!(checksum, 16_000_000); // No disc: shell-open status bit.

        let start = Instant::now();
        let mut checksum = 0_u64;
        for _ in 0..1_000_000 {
            cd.write(2, 0x80);
            cd.write(1, 0x0e); // Setmode transfers a parameter buffer to the command.
            cd.tick(COMMAND_CLOCKS);
            checksum += u64::from(cd.read(1));
            acknowledge(&mut cd);
        }
        println!(
            "parameters round={round} count=1000000 elapsed_ms={:.3} checksum={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
        assert_eq!(checksum, 16_000_000);

        let start = Instant::now();
        let mut checksum = 0_u64;
        for _ in 0..1_000_000 {
            cd.write(1, 0x1a); // GetID holds an eight-byte completion behind INT3.
            cd.tick(COMMAND_CLOCKS);
            checksum += u64::from(cd.read(1));
            acknowledge(&mut cd);
            cd.tick(COMMAND_CLOCKS * 2);
            for _ in 0..8 {
                checksum += u64::from(cd.read(1));
            }
            acknowledge(&mut cd);
        }
        println!(
            "completions round={round} count=1000000 elapsed_ms={:.3} checksum={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
        assert_eq!(checksum, 88_000_000);

        let mut cd = CdRom::default();
        cd.insert(disc.clone());
        cd.write(1, 6); // ReadN, default 2048-byte payload mode.
        cd.tick(COMMAND_CLOCKS);
        acknowledge(&mut cd);
        let mut checksum = 0_u64;
        let start = Instant::now();
        for _ in 0..SECTORS {
            cd.tick(SECTOR_CLOCKS);
            cd.write(3, 0x80);
            assert_eq!(cd.data_available(), 2048);
            for _ in 0..2048 {
                checksum += u64::from(cd.read_data());
            }
            acknowledge(&mut cd);
        }
        println!(
            "sectors round={round} count={SECTORS} elapsed_ms={:.3} checksum={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
        assert_eq!(checksum, SECTORS as u64 * 2048 * 90);
        assert_eq!(cd.command_errors, 0);

        let start = Instant::now();
        let mut checksum = 0_u64;
        for lba in 0..SECTORS as u32 {
            let sector = black_box(cooked.read_sector(lba)?);
            checksum += u64::from(u32::from_le_bytes(sector[2072..2076].try_into().unwrap()));
        }
        println!(
            "cooked round={round} count={SECTORS} elapsed_ms={:.3} edc_sum={}",
            start.elapsed().as_secs_f64() * 1000.0,
            black_box(checksum)
        );
    }
    Ok(())
}

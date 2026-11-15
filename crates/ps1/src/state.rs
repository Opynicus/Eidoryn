//! Versioned guest snapshots. Host rendering workers and native code are rebuilt.
use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::machine::Machine;

const MAGIC: &[u8; 8] = b"EIDOPS1\0";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 8 + 4 + 32;
const MAX_SIZE: usize = 128 * 1024 * 1024;

/// Trusted, process-local guest state. It keeps mounted media open, but owns no
/// GPU worker or native-code cache. Consume it while stepping back in history.
pub struct RewindState {
    machine: Machine,
    memory_size: usize,
}

impl RewindState {
    /// Conservative accounting for a bounded history, including shared scanout
    /// pixels. Queue capacity and track metadata are deliberately overestimated.
    pub fn memory_size(&self) -> usize {
        self.memory_size
    }

    /// The restored guest starts with serial rendering and the interpreter.
    /// Reapply the host backends once forward execution resumes.
    pub fn into_machine(self) -> Machine {
        self.machine
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_SIZE as u64)
        .reject_trailing_bytes()
}

impl Machine {
    /// Capture a rewind point without file I/O, checksumming, or cloning host
    /// caches. GPU work finishes before VRAM and the completed frame are copied.
    pub fn rewind_state(&self) -> Result<RewindState, String> {
        self.bus.gpu.synchronize_renderer();
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture rewind state: {error}"));
        }
        let machine = self.clone_guest();
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture rewind state: {error}"));
        }
        // Disc serialization only measures its cached path and identity, never
        // reopens or hashes the image. The snapshot renderer is already serial.
        let serialized = codec()
            .serialized_size(&machine)
            .map_err(|error| format!("Cannot size rewind state: {error}"))?;
        let memory_size = usize::try_from(serialized)
            .ok()
            .and_then(|size| size.checked_mul(2))
            .and_then(|size| size.checked_add(size_of::<Machine>() + 65_536))
            .ok_or("Rewind state size overflow")?;
        Ok(RewindState {
            machine,
            memory_size,
        })
    }

    /// Capture all guest hardware state at an instruction boundary, including
    /// pending device work, sound RAM, VRAM, memory cards and the current frame.
    /// Disc media are referenced by canonical path and verified on restoration.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        self.bus.gpu.synchronize_renderer();
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot save a failed GPU renderer: {error}"));
        }
        let payload = codec()
            .serialize(self)
            .map_err(|e| format!("Save state: {e}"))?;
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture GPU state: {error}"));
        }
        if payload.len() > MAX_SIZE - HEADER_SIZE {
            return Err("Save state exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&Sha256::digest(&payload));
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// Decode and validate without changing a running machine. Restoration
    /// starts in the interpreter with a serial renderer; the host may reapply
    /// its selected backends after this succeeds, using fresh caches/workers.
    pub fn load_state(bytes: &[u8]) -> Result<Self, String> {
        if !(HEADER_SIZE..=MAX_SIZE).contains(&bytes.len()) || &bytes[..8] != MAGIC {
            return Err("Invalid or truncated PS1 save state".into());
        }
        if bytes[8..12] != VERSION.to_le_bytes() {
            return Err("Unsupported PS1 save-state version".into());
        }
        let payload = &bytes[HEADER_SIZE..];
        if Sha256::digest(payload)[..] != bytes[12..HEADER_SIZE] {
            return Err("Save-state checksum mismatch".into());
        }
        let machine: Self = codec()
            .deserialize(payload)
            .map_err(|e| format!("Load state: {e}"))?;
        machine.cpu.validate_state()?;
        machine.bus.validate_state()?;
        Ok(machine)
    }
}

// Serde supports arrays up to 32 elements. Validate larger hardware tables
// through a length-checked temporary vector without unsafe initialization.
pub(crate) mod array {
    use super::{Deserialize, Serialize};

    pub fn serialize<T: Serialize, S: serde::Serializer, const N: usize>(
        value: &[T; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[T; N], D::Error> {
        Vec::<T>::deserialize(deserializer)?
            .try_into()
            .map_err(|_: Vec<T>| serde::de::Error::custom("Invalid hardware table length"))
    }
}

pub(crate) mod matrix {
    use super::{Deserialize, Serialize};

    pub fn serialize<T: Serialize, S: serde::Serializer, const N: usize, const M: usize>(
        value: &[[T; N]; M],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(<[T; N]>::as_slice)
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<
        'de,
        T: Deserialize<'de>,
        D: serde::Deserializer<'de>,
        const N: usize,
        const M: usize,
    >(
        deserializer: D,
    ) -> Result<[[T; N]; M], D::Error> {
        Vec::<Vec<T>>::deserialize(deserializer)?
            .into_iter()
            .map(|row| {
                row.try_into()
                    .map_err(|_: Vec<T>| serde::de::Error::custom("Invalid hardware table width"))
            })
            .collect::<Result<Vec<[T; N]>, D::Error>>()?
            .try_into()
            .map_err(|_: Vec<[T; N]>| serde::de::Error::custom("Invalid hardware table height"))
    }
}

pub(crate) fn static_error(value: &str) -> Result<&'static str, &'static str> {
    // Existing device errors use static diagnostic messages; decoding must not
    // leak file-controlled strings to manufacture a static lifetime.
    const MESSAGES: &[&str] = &[
        "CD-ROM DMA exceeds the available sector data",
        "DMA register read",
        "DMA register write",
        "MDEC requires incrementing request-mode DMA0 input or DMA1 output",
        "MDEC transfer exceeds RAM-sized limit",
        "byte read",
        "byte write",
        "chopping requires CD-ROM input",
        "input FIFO full; DMA/CPU must wait for output to drain",
        "linked list exceeds prototype transfer limit",
        "linked lists require RAM -> GPU channel 2",
        "manual-write FIFO is full",
        "only GPU, SPU, manual CD-ROM input, and manual OTC transfers are implemented",
        "only standard 512 KiB sound RAM configuration is implemented",
        "timer register read",
        "transfer exceeds prototype RAM-sized limit",
        "word read",
        "word write",
    ];
    MESSAGES
        .iter()
        .copied()
        .find(|message| *message == value)
        .ok_or("Unknown saved device error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cpu::Cpu, cpu_engine::CpuMode, demo, gpu::RendererMode, memory_card::MemoryCard};

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

    #[test]
    fn save_state_resumes_pipeline_devices_audio_and_card_transaction() {
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
        for steps in [1, 1, 2] {
            machine.run_steps(steps).unwrap();
            let bytes = machine.save_state().unwrap();
            let mut restored = Machine::load_state(&bytes).unwrap();
            assert_eq!(restored.save_state().unwrap(), bytes);
            let mut reference = machine.clone();
            for candidate in [&mut reference, &mut restored] {
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
            assert_eq!(
                restored.save_state().unwrap(),
                reference.save_state().unwrap()
            );
            assert_eq!(restored.bus.sio.cards[0].as_ref().unwrap().writes, 1);
            assert!(restored.bus.spu.generated_samples > 0);
        }
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
        let mut halted = Machine::demo();
        halted.halted = Some(crate::EmuError::Dma("chopping requires CD-ROM input"));
        let restored = Machine::load_state(&halted.save_state().unwrap()).unwrap();
        assert_eq!(restored.halted, halted.halted);
    }
}

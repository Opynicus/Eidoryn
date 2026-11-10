use crate::{
    EmuError,
    bus::{BIOS_SIZE, Bus, RAM_SIZE, physical},
    cpu::Cpu,
    cpu_engine::{CpuMode, CpuStats, Execution},
    demo,
    disc::DiscImage,
};

pub const INSTRUCTIONS_PER_SLICE: usize = 20_000;

#[derive(Clone)]
pub struct Machine {
    pub cpu: Cpu,
    pub bus: Bus,
    pub title: String,
    pub halted: Option<EmuError>,
    pub slices: u64,
    demo: bool,
    execution: Execution,
}

impl Machine {
    pub fn from_disc(
        path: &std::path::Path,
        bios: Option<&[u8]>,
        fast_boot: bool,
    ) -> Result<Self, EmuError> {
        let disc = DiscImage::open(path)?;
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let mut machine = if fast_boot {
            Self::from_exe(&disc.boot_executable()?, &format!("{name} / EXE fast boot"))?
        } else {
            Self::from_bios(bios.ok_or_else(|| EmuError::InvalidImage("Disc BIOS boot requires a 512 KiB BIOS; --fast-boot is only for BIOS-independent homebrew".into()))?, &format!("{name} / BIOS boot"))?
        };
        if let Some(bios) = bios
            && fast_boot
        {
            if bios.len() != BIOS_SIZE {
                return Err(EmuError::InvalidImage(
                    "BIOS must be exactly 512 KiB".into(),
                ));
            }
            machine.bus.bios.copy_from_slice(bios);
        }
        machine.bus.cdrom.insert(disc);
        Ok(machine)
    }

    pub fn demo() -> Self {
        let mut machine = Self::empty(demo::ENTRY, "Built-in MIPS demo");
        for (n, instruction) in demo::program().into_iter().enumerate() {
            machine.bus.ram[0x10000 + n * 4..0x10004 + n * 4]
                .copy_from_slice(&instruction.to_le_bytes());
        }
        machine.demo = true;
        machine
    }

    fn empty(pc: u32, title: &str) -> Self {
        Self {
            cpu: Cpu::new(pc),
            bus: Bus::default(),
            title: title.into(),
            halted: None,
            slices: 0,
            demo: false,
            execution: Execution::default(),
        }
    }

    pub fn from_bios(bytes: &[u8], title: &str) -> Result<Self, EmuError> {
        if bytes.len() != BIOS_SIZE {
            return Err(EmuError::InvalidImage(format!(
                "BIOS must be exactly 512 KiB; got {} bytes",
                bytes.len()
            )));
        }
        let mut machine = Self::empty(0xbfc0_0000, title);
        machine.bus.bios.copy_from_slice(bytes);
        Ok(machine)
    }

    /// Load a PS-X EXE directly into RAM. No BIOS services or executable relocations.
    pub fn from_exe(bytes: &[u8], title: &str) -> Result<Self, EmuError> {
        if bytes.len() < 0x800 || &bytes[..8] != b"PS-X EXE" {
            return Err(EmuError::InvalidImage(
                "Expected a PS-X EXE with a complete 2048-byte header".into(),
            ));
        }
        let word = |offset: usize| {
            u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("validated EXE header"),
            )
        };
        let entry = word(0x10);
        let destination = physical(word(0x18)) as usize;
        let length = word(0x1c) as usize;
        let zero_start = physical(word(0x28)) as usize;
        let zero_size = word(0x2c) as usize;
        if entry & 3 != 0 || physical(entry) as usize >= RAM_SIZE {
            return Err(EmuError::InvalidImage(
                "EXE entry point must be aligned and inside main RAM".into(),
            ));
        }
        if length == 0
            || destination >= RAM_SIZE
            || length > RAM_SIZE - destination
            || length > bytes.len() - 0x800
        {
            return Err(EmuError::InvalidImage(
                "EXE payload is truncated, empty, or outside main RAM".into(),
            ));
        }
        if zero_size != 0 && (zero_start >= RAM_SIZE || zero_size > RAM_SIZE - zero_start) {
            return Err(EmuError::InvalidImage(
                "EXE zero-fill region is outside main RAM".into(),
            ));
        }
        let mut machine = Self::empty(entry, title);
        machine.bus.ram[destination..destination + length]
            .copy_from_slice(&bytes[0x800..0x800 + length]);
        if zero_size != 0 {
            machine.bus.ram[zero_start..zero_start + zero_size].fill(0);
        }
        machine.cpu.registers[28] = word(0x14);
        if word(0x30) != 0 {
            machine.cpu.registers[29] = word(0x30).wrapping_add(word(0x34));
            machine.cpu.registers[30] = machine.cpu.registers[29];
        }
        Ok(machine)
    }

    pub fn step(&mut self) -> Result<(), EmuError> {
        if let Some(error) = &self.halted {
            return Err(error.clone());
        }
        if let Err(error) = self.cpu.step(&mut self.bus).and_then(|()| self.bus.tick(1)) {
            self.halted = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn cpu_mode(&self) -> CpuMode {
        self.execution.mode()
    }
    pub fn set_cpu_mode(&mut self, mode: CpuMode) -> std::io::Result<()> {
        self.execution.set_mode(mode)
    }
    pub fn cpu_stats(&self) -> CpuStats {
        self.execution.stats()
    }

    /// Run an exact guest-step budget, using native blocks when enabled. Debug
    /// `step()` always executes exactly one reference interpreter step.
    pub fn run_steps(&mut self, budget: usize) -> Result<(), EmuError> {
        if self.cpu_mode() == CpuMode::Interpreter {
            for _ in 0..budget {
                self.step()?;
            }
        } else {
            let mut completed = 0;
            while completed < budget {
                completed += self.execution_chunk(budget - completed)?;
            }
        }
        Ok(())
    }

    fn execution_chunk(&mut self, remaining: usize) -> Result<usize, EmuError> {
        if let Some(error) = &self.halted {
            return Err(error.clone());
        }
        match self
            .execution
            .try_run(&mut self.cpu, &mut self.bus, remaining)
        {
            Ok(0) => {
                self.step()?;
                Ok(1)
            }
            Ok(completed) => Ok(completed),
            Err(error) => {
                self.halted = Some(error.clone());
                Err(error)
            }
        }
    }

    /// One bounded host slice, not a cycle-accurate emulated video frame.
    /// The original demo yields at its guest-written frame-completion counter.
    pub fn run_slice(&mut self) -> Result<usize, EmuError> {
        self.check_renderer()?;
        if !self.demo {
            self.run_steps(INSTRUCTIONS_PER_SLICE)?;
            self.slices += 1;
            self.check_renderer()?;
            return Ok(INSTRUCTIONS_PER_SLICE);
        }
        let counter = self.demo_counter();
        let mut completed = 0;
        while completed < INSTRUCTIONS_PER_SLICE {
            // The BIOS-free demo can finish a frame on any guest store. Unlike
            // ordinary VBlank capture, this host-side counter needs per-step
            // observation, including instructions in cached interpreter spans.
            completed += self.execution_chunk(1)?;
            if self.demo && self.demo_counter() != counter {
                // This BIOS-free demo has an explicit guest frame boundary and
                // intentionally does not wait for the hardware VBlank clock.
                self.bus.gpu.latch_display();
                self.slices += 1;
                self.check_renderer()?;
                return Ok(completed);
            }
        }
        self.slices += 1;
        self.check_renderer()?;
        Ok(INSTRUCTIONS_PER_SLICE)
    }

    /// Check asynchronous device failures at a host scheduling boundary, without
    /// adding synchronization to every interpreted CPU instruction.
    pub fn check_renderer(&mut self) -> Result<(), EmuError> {
        if let Some(message) = self.bus.gpu.renderer_error() {
            let error = EmuError::GpuBackend(message);
            self.halted = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    fn demo_counter(&self) -> u32 {
        u32::from_le_bytes(
            self.bus.ram[demo::FRAME_COUNTER..demo::FRAME_COUNTER + 4]
                .try_into()
                .expect("demo counter within RAM"),
        )
    }
}

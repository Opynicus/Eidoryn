use crate::machine::Machine;

/// Trusted, process-local guest state. It keeps mounted media open, but owns no
/// GPU worker or native-code cache. Consume it while stepping back in history.
pub struct RewindState {
    machine: Machine,
    memory_size: usize,
}

impl RewindState {
    pub(super) fn capture(source: &Machine) -> Result<Self, String> {
        source.bus.gpu.synchronize_renderer();
        if let Some(error) = source.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture rewind state: {error}"));
        }
        let machine = source.clone_guest();
        if let Some(error) = source.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture rewind state: {error}"));
        }
        let memory_size = machine
            .rewind_heap_bytes()
            .checked_mul(2)
            .and_then(|size| size.checked_add(size_of::<Self>() + 65_536))
            .ok_or("Rewind state size overflow")?;
        Ok(Self {
            machine,
            memory_size,
        })
    }

    /// Conservative history charge, including dynamic capacity and this value's
    /// inline storage. Shared allocations are charged in full for each snapshot;
    /// this is not deduplicated memory use or RSS. The 2x margin and 64 KiB reserve
    /// cover allocator overhead and keep the existing history budget conservative.
    pub fn memory_size(&self) -> usize {
        self.memory_size
    }

    /// The restored guest starts with serial rendering and the interpreter.
    /// Reapply the host backends once forward execution resumes.
    pub fn into_machine(self) -> Machine {
        self.machine
    }
}

pub(crate) fn shared_bytes<T: ?Sized>(value: &T) -> usize {
    size_of_val(value) + 2 * size_of::<usize>()
}

impl Machine {
    fn rewind_heap_bytes(&self) -> usize {
        let error_bytes = match &self.halted {
            Some(
                crate::EmuError::GpuBackend(error)
                | crate::EmuError::Jit(error)
                | crate::EmuError::InvalidImage(error),
            ) => error.capacity(),
            _ => 0,
        };
        // clone_guest leaves the DMA scratch set and native execution cache empty.
        self.title.capacity()
            + error_bytes
            + self.bus.ram.capacity()
            + shared_bytes(&self.bus.bios[..])
            + self.bus.gpu.rewind_heap_bytes()
            + self.bus.cdrom.heap_bytes()
            + self.bus.spu.heap_bytes()
            + self.bus.sio.heap_bytes()
            + self.bus.mdec.heap_bytes()
    }
}

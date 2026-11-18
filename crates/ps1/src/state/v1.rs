//! Frozen V1 payload. Field order, enum tags and sequence encodings are disk ABI.
//! Each device owns its nested V1 layout and the conversion to validated hardware state.
use bincode::Options;
use serde::{Deserialize, Serialize};

use super::MAX_SIZE;
use crate::{EmuError, bus::BusV1, cpu::CpuV1, machine::Machine};

pub(super) const VERSION: u32 = 1;

/// Keep the original public Serde implementations on the same checked path as disk loading.
macro_rules! impl_serde_v1 {
    ($device:ty, $repr:ty) => {
        impl serde::Serialize for $device {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let saved = <$repr>::try_from(self).map_err(serde::ser::Error::custom)?;
                serde::Serialize::serialize(&saved, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $device {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let saved = <$repr as serde::Deserialize>::deserialize(deserializer)?;
                Self::try_from(saved).map_err(serde::de::Error::custom)
            }
        }
    };
}
pub(crate) use impl_serde_v1;

#[derive(Serialize, Deserialize)]
#[serde(rename = "Machine")]
pub(crate) struct MachineV1 {
    pub(crate) cpu: CpuV1,
    pub(crate) bus: BusV1,
    pub(crate) title: String,
    pub(crate) halted: Option<ErrorV1>,
    pub(crate) slices: u64,
    pub(crate) demo: bool,
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_SIZE as u64)
        .reject_trailing_bytes()
}

pub(super) fn encode(machine: &Machine) -> Result<Vec<u8>, String> {
    let saved = MachineV1::try_from(machine).map_err(|e| format!("Save state: {e}"))?;
    codec()
        .serialize(&saved)
        .map_err(|e| format!("Save state: {e}"))
}

pub(super) fn decode(payload: &[u8]) -> Result<Machine, String> {
    let saved: MachineV1 = codec()
        .deserialize(payload)
        .map_err(|e| format!("Load state: {e}"))?;
    Machine::try_from(saved)
}

#[derive(Serialize, Deserialize)]
#[serde(rename = "EmuError")]
pub(crate) enum ErrorV1 {
    Bus { address: u32, operation: String },
    UnsupportedInstruction { pc: u32, instruction: u32 },
    GpuCommand(u8),
    GpuBackend(String),
    Jit(String),
    Dma(String),
    Spu(String),
    Mdec(String),
    InvalidImage(String),
}

impl From<&EmuError> for ErrorV1 {
    fn from(error: &EmuError) -> Self {
        match error {
            EmuError::Bus { address, operation } => Self::Bus {
                address: *address,
                operation: (*operation).into(),
            },
            EmuError::UnsupportedInstruction { pc, instruction } => Self::UnsupportedInstruction {
                pc: *pc,
                instruction: *instruction,
            },
            EmuError::GpuCommand(command) => Self::GpuCommand(*command),
            EmuError::GpuBackend(message) => Self::GpuBackend(message.clone()),
            EmuError::Jit(message) => Self::Jit(message.clone()),
            EmuError::Dma(message) => Self::Dma((*message).into()),
            EmuError::Spu(message) => Self::Spu((*message).into()),
            EmuError::Mdec(message) => Self::Mdec((*message).into()),
            EmuError::InvalidImage(message) => Self::InvalidImage(message.clone()),
        }
    }
}

impl TryFrom<ErrorV1> for EmuError {
    type Error = &'static str;

    fn try_from(saved: ErrorV1) -> Result<Self, Self::Error> {
        Ok(match saved {
            ErrorV1::Bus { address, operation } => Self::Bus {
                address,
                operation: static_error(&operation)?,
            },
            ErrorV1::UnsupportedInstruction { pc, instruction } => {
                Self::UnsupportedInstruction { pc, instruction }
            }
            ErrorV1::GpuCommand(command) => Self::GpuCommand(command),
            ErrorV1::GpuBackend(message) => Self::GpuBackend(message),
            ErrorV1::Jit(message) => Self::Jit(message),
            ErrorV1::Dma(message) => Self::Dma(static_error(&message)?),
            ErrorV1::Spu(message) => Self::Spu(static_error(&message)?),
            ErrorV1::Mdec(message) => Self::Mdec(static_error(&message)?),
            ErrorV1::InvalidImage(message) => Self::InvalidImage(message),
        })
    }
}

impl_serde_v1!(EmuError, ErrorV1);

fn static_error(value: &str) -> Result<&'static str, &'static str> {
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

    #[test]
    fn v1_dma_error_retains_its_tag_and_string_payload() {
        let legacy = b"\x05\0\0\0\x1e\0\0\0\0\0\0\0chopping requires CD-ROM input";
        let error = EmuError::Dma("chopping requires CD-ROM input");
        assert_eq!(codec().serialize(&error).unwrap(), legacy);
        assert_eq!(codec().deserialize::<EmuError>(legacy).unwrap(), error);

        let unknown = ErrorV1::Dma("unrecognized".into());
        let bytes = codec().serialize(&unknown).unwrap();
        assert!(codec().deserialize::<EmuError>(&bytes).is_err());
    }
}

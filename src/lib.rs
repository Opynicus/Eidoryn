//! A deliberately small, testable PlayStation emulation core.
#![allow(clippy::doc_markdown)]

mod adpcm;
pub mod app;
pub mod audio;
pub mod bus;
pub mod card_file;
mod cd_audio;
pub mod cdrom;
pub mod cpu;
pub mod cpu_engine;
mod cue;
pub mod demo;
pub mod disc;
pub mod gpu;
pub mod gte;
pub mod machine;
pub mod mdec;
pub mod media;
pub mod memory_card;
pub mod runner;
pub mod sio;
pub mod spu;
#[cfg(feature = "desktop")]
pub mod theme;
pub mod timers;
pub mod video_filter;
mod xa;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmuError {
    Bus {
        address: u32,
        operation: &'static str,
    },
    UnsupportedInstruction {
        pc: u32,
        instruction: u32,
    },
    GpuCommand(u8),
    GpuBackend(String),
    Jit(String),
    Dma(&'static str),
    Spu(&'static str),
    Mdec(&'static str),
    InvalidImage(String),
}

impl std::fmt::Display for EmuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus { address, operation } => {
                write!(f, "Unsupported bus {operation} at 0x{address:08X}")
            }
            Self::UnsupportedInstruction { pc, instruction } => write!(
                f,
                "Unsupported instruction 0x{instruction:08X} at PC 0x{pc:08X}"
            ),
            Self::GpuCommand(command) => write!(f, "Unsupported GPU command 0x{command:02X}"),
            Self::GpuBackend(message) => write!(f, "GPU renderer: {message}"),
            Self::Jit(message) => write!(f, "Native JIT: {message}"),
            Self::Dma(message) => write!(f, "DMA: {message}"),
            Self::Spu(message) => write!(f, "SPU: {message}"),
            Self::Mdec(message) => write!(f, "MDEC: {message}"),
            Self::InvalidImage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EmuError {}

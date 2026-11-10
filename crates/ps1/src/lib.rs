//! Independent PlayStation hardware core. No frontend, system audio, or theme dependencies.
#![allow(clippy::doc_markdown)]

mod error;
pub use error::EmuError;

mod adpcm;
pub mod bus;
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
pub mod memory_card;
pub mod sio;
pub mod spu;
pub mod timers;
mod xa;

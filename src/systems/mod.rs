//! System-specific application adapters. Machine state lives in each core crate.
pub mod checkpoints;
#[cfg(feature = "desktop")]
pub mod gba;
mod persistence;
pub mod ps1;

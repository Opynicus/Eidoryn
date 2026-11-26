//! System-specific application adapters. Machine state lives in each core crate.
pub mod checkpoints;
#[cfg(feature = "desktop")]
pub mod gba;
#[cfg(feature = "desktop")]
pub mod nds;
mod persistence;
pub mod ps1;

#[cfg(all(test, feature = "desktop"))]
#[path = "../../tests/support/handheld_tone.rs"]
mod handheld_tone;

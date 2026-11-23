//! System-specific application adapters. Machine state lives in each core crate.
#[cfg(feature = "desktop")]
pub mod gba;
pub mod ps1;

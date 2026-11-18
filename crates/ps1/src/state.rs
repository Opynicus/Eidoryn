//! Versioned guest snapshots. Host rendering workers and native code are rebuilt.
mod array_codec;
mod rewind;
pub(crate) mod v1;

use sha2::{Digest, Sha256};

use crate::machine::Machine;
pub(crate) use array_codec::{array, matrix};
pub use rewind::RewindState;
pub(crate) use rewind::shared_bytes;

const MAGIC: &[u8; 8] = b"EIDOPS1\0";
const HEADER_SIZE: usize = 8 + 4 + 32;
const MAX_SIZE: usize = 128 * 1024 * 1024;

impl Machine {
    /// Capture a rewind point without file I/O, checksumming, or cloning host
    /// caches. GPU work finishes before VRAM and the completed frame are copied.
    pub fn rewind_state(&self) -> Result<RewindState, String> {
        RewindState::capture(self)
    }

    /// Capture all guest hardware state at an instruction boundary, including
    /// pending device work, sound RAM, VRAM, memory cards and the current frame.
    /// Disc media are referenced by canonical path and verified on restoration.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        self.bus.gpu.synchronize_renderer();
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot save a failed GPU renderer: {error}"));
        }
        let payload = v1::encode(self)?;
        if let Some(error) = self.bus.gpu.renderer_error() {
            return Err(format!("Cannot capture GPU state: {error}"));
        }
        if payload.len() > MAX_SIZE - HEADER_SIZE {
            return Err("Save state exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&v1::VERSION.to_le_bytes());
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
        if bytes[8..12] != v1::VERSION.to_le_bytes() {
            return Err("Unsupported PS1 save-state version".into());
        }
        let payload = &bytes[HEADER_SIZE..];
        if Sha256::digest(payload)[..] != bytes[12..HEADER_SIZE] {
            return Err("Save-state checksum mismatch".into());
        }
        v1::decode(payload)
    }
}

#[cfg(test)]
mod tests;

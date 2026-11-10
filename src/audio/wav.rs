//! Streaming, non-overwriting PCM diagnostic export, independent of any sound device.
use crate::spu::{SAMPLE_RATE, StereoFrame};
use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
};

pub struct WavWriter {
    file: BufWriter<File>,
    bytes: u32,
    finished: bool,
}
impl WavWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let mut writer = Self {
            file: BufWriter::new(File::create_new(path)?),
            bytes: 0,
            finished: false,
        };
        writer.header()?;
        Ok(writer)
    }
    fn header(&mut self) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(b"RIFF")?;
        self.file.write_all(&(36 + self.bytes).to_le_bytes())?;
        self.file.write_all(b"WAVEfmt \x10\0\0\0\x01\0\x02\0")?;
        self.file.write_all(&SAMPLE_RATE.to_le_bytes())?;
        self.file.write_all(&(SAMPLE_RATE * 4).to_le_bytes())?;
        self.file.write_all(b"\x04\0\x10\0data")?;
        self.file.write_all(&self.bytes.to_le_bytes())
    }
    pub fn write(&mut self, frames: impl IntoIterator<Item = StereoFrame>) -> std::io::Result<()> {
        for frame in frames {
            if self.bytes > u32::MAX - 40 {
                return Err(std::io::Error::other("WAV exceeds RIFF size limit"));
            }
            for sample in frame {
                self.file.write_all(&sample.to_le_bytes())?;
            }
            self.bytes += 4;
        }
        Ok(())
    }
    pub fn finish(mut self) -> std::io::Result<()> {
        self.header()?;
        self.file.flush()?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        // Preserve diagnostic PCM on an early return; finish() reports I/O failures.
        if !self.finished {
            let _ = self.header().and_then(|()| self.file.flush());
        }
    }
}

//! CD decoder output: resampled XA or raw CD-DA, followed by the controller's volume matrix.
use crate::{spu::StereoFrame, xa::XaDecoder};
use std::collections::VecDeque;

#[derive(Clone)]
pub(crate) struct CdAudio {
    decoder: XaDecoder,
    samples: VecDeque<StereoFrame>,
    pending: [u8; 4],
    volume: [u8; 4],
    xa_mute: bool,
    is_xa: bool,
}

impl Default for CdAudio {
    fn default() -> Self {
        Self {
            decoder: XaDecoder::default(),
            samples: VecDeque::new(),
            pending: [0x80, 0, 0x80, 0],
            volume: [0x80, 0, 0x80, 0],
            xa_mute: false,
            is_xa: false,
        }
    }
}

impl CdAudio {
    pub fn xa_active(&self) -> bool {
        self.is_xa && !self.samples.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.decoder = XaDecoder::default();
        self.is_xa = false;
    }
    pub fn write(&mut self, bank: u8, register: u32, value: u8) {
        match (bank, register) {
            (2, 2) => self.pending[0] = value,
            (2, 3) => self.pending[1] = value,
            (3, 1) => self.pending[2] = value,
            (3, 2) => self.pending[3] = value,
            (3, 3) => {
                self.xa_mute = value & 1 != 0;
                if value & 0x20 != 0 {
                    self.volume = self.pending;
                }
            }
            _ => {}
        }
    }
    fn push(&mut self, samples: impl IntoIterator<Item = StereoFrame>) {
        for sample in samples {
            push_sample(&mut self.samples, sample);
        }
    }
    pub fn xa(&mut self, raw: &[u8]) {
        self.is_xa = true;
        // Decode directly into the bounded queue. Filter history belongs to the
        // decoder; no temporary 9408-frame PCM allocation is needed per sector.
        let samples = &mut self.samples;
        self.decoder
            .decode(raw, |sample| push_sample(samples, sample));
    }
    pub fn cdda(&mut self, raw: &[u8]) {
        self.is_xa = false;
        self.push(raw.as_chunks::<4>().0.iter().map(|b| {
            [
                i16::from_le_bytes([b[0], b[1]]),
                i16::from_le_bytes([b[2], b[3]]),
            ]
        }));
    }
    pub fn silence_sector(&mut self) {
        self.is_xa = false;
        self.push(std::iter::repeat_n([0; 2], 588));
    }
    pub fn sample(&mut self, muted: bool) -> StereoFrame {
        let [left, right] = self.samples.pop_front().unwrap_or([0; 2]).map(i32::from);
        if muted || (self.is_xa && self.xa_mute) {
            return [0; 2];
        }
        let [ll, lr, rr, rl] = self.volume.map(i32::from);
        [
            ((left * ll + right * rl) >> 7).clamp(-32768, 32767) as i16,
            ((left * lr + right * rr) >> 7).clamp(-32768, 32767) as i16,
        ]
    }
}

fn push_sample(queue: &mut VecDeque<StereoFrame>, sample: StereoFrame) {
    if queue.len() == 16_384 {
        queue.pop_front();
    }
    queue.push_back(sample);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_xa_keeps_the_newest_bounded_pcm_and_clear_resets_history() {
        let mut raw = [0; 2352];
        raw[19] = 4; // Mono, 18.9 kHz: largest packet, 9408 stereo output frames.
        for group in raw[24..2328].as_chunks_mut::<128>().0 {
            group[16..].fill(0x21);
        }
        let mut reference = Vec::new();
        let mut decoder = XaDecoder::default();
        let mut audio = CdAudio::default();
        for _ in 0..3 {
            decoder.decode(&raw, |sample| reference.push(sample));
            audio.xa(&raw);
        }
        assert_eq!(audio.samples.len(), 16_384);
        for &sample in &reference[reference.len() - 16_384..] {
            assert_eq!(audio.sample(false), sample);
        }
        assert!(!audio.xa_active());
        audio.clear();
        audio.xa(&raw);
        for &sample in &reference[..9408] {
            assert_eq!(audio.sample(false), sample);
        }
    }

    #[test]
    fn cdda_matrix_applies_atomically_and_mute_consumes_samples() {
        let mut audio = CdAudio::default();
        let pcm = [1000_i16, -2000]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        audio.cdda(&pcm);
        assert_eq!(audio.sample(false), [1000, -2000]);
        for (bank, register) in [(2, 2), (2, 3), (3, 1), (3, 2)] {
            audio.write(bank, register, 0x40);
        }
        audio.cdda(&pcm);
        assert_eq!(audio.sample(false), [1000, -2000]);
        audio.write(3, 3, 0x20);
        audio.cdda(&pcm);
        assert_eq!(audio.sample(false), [-500; 2]);
        audio.cdda(&pcm);
        assert_eq!(audio.sample(true), [0; 2]);
        assert_eq!(audio.sample(false), [0; 2]);
    }
}

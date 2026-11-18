//! Frozen V1 sound hardware state.
use super::{Spu, envelope::EnvelopeV1, reverb::ReverbV1, voice::VoiceV1};
use std::collections::VecDeque;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Spu")]
pub(crate) struct SpuV1 {
    #[serde(with = "crate::state::array")]
    registers: [u16; 256],
    ram: Vec<u8>,
    fifo: VecDeque<u16>,
    cursor: usize,
    applied: u16,
    control_delay: u32,
    transfer_clock: u32,
    irq: bool,
    voices: [VoiceV1; 24],
    main_volume: [EnvelopeV1; 2],
    reverb: ReverbV1,
    sample_clock: u32,
    capture_cursor: usize,
    noise_timer: i32,
    noise_level: u16,
    end_flags: u32,
    output: VecDeque<[i16; 2]>,
    generated_samples: u64,
    nonzero_samples: u64,
    transferred_halfwords: u64,
}

impl From<&Spu> for SpuV1 {
    fn from(state: &Spu) -> Self {
        Self {
            registers: state.registers,
            ram: state.ram.clone(),
            fifo: state.fifo.clone(),
            cursor: state.cursor,
            applied: state.applied,
            control_delay: state.control_delay,
            transfer_clock: state.transfer_clock,
            irq: state.irq,
            voices: state.voices.each_ref().map(Into::into),
            main_volume: state.main_volume.each_ref().map(Into::into),
            reverb: (&state.reverb).into(),
            sample_clock: state.sample_clock,
            capture_cursor: state.capture_cursor,
            noise_timer: state.noise_timer,
            noise_level: state.noise_level,
            end_flags: state.end_flags,
            output: state.output.clone(),
            generated_samples: state.generated_samples,
            nonzero_samples: state.nonzero_samples,
            transferred_halfwords: state.transferred_halfwords,
        }
    }
}

impl TryFrom<SpuV1> for Spu {
    type Error = String;

    fn try_from(state: SpuV1) -> Result<Self, Self::Error> {
        let device = Self {
            registers: state.registers,
            ram: state.ram,
            fifo: state.fifo,
            cursor: state.cursor,
            applied: state.applied,
            control_delay: state.control_delay,
            transfer_clock: state.transfer_clock,
            irq: state.irq,
            voices: state.voices.map(Into::into),
            main_volume: state.main_volume.map(Into::into),
            reverb: state.reverb.into(),
            sample_clock: state.sample_clock,
            capture_cursor: state.capture_cursor,
            noise_timer: state.noise_timer,
            noise_level: state.noise_level,
            end_flags: state.end_flags,
            output: state.output,
            generated_samples: state.generated_samples,
            nonzero_samples: state.nonzero_samples,
            transferred_halfwords: state.transferred_halfwords,
        };
        device.validate_state()?;
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::Spu;

    #[test]
    fn direct_spu_decode_checks_nested_envelopes() {
        let mut state = Spu::default();
        state.voices[23].envelope.level = i32::MAX;
        let bytes = bincode::serialize(&state).unwrap();
        assert!(bincode::deserialize::<Spu>(&bytes).is_err());
    }
}

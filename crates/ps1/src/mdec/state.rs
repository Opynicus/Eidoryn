//! Frozen V1 representation of Mdec.
use super::{Mdec, Operation};
use std::collections::VecDeque;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Operation")]
enum OperationV1 {
    Idle,
    Quant,
    Scale,
    Decode,
}

impl From<Operation> for OperationV1 {
    fn from(value: Operation) -> Self {
        match value {
            Operation::Idle => Self::Idle,
            Operation::Quant => Self::Quant,
            Operation::Scale => Self::Scale,
            Operation::Decode => Self::Decode,
        }
    }
}

impl From<OperationV1> for Operation {
    fn from(value: OperationV1) -> Self {
        match value {
            OperationV1::Idle => Self::Idle,
            OperationV1::Quant => Self::Quant,
            OperationV1::Scale => Self::Scale,
            OperationV1::Decode => Self::Decode,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Mdec")]
pub(crate) struct MdecV1 {
    operation: OperationV1,
    command: u32,
    remaining: u32,
    status_count: u16,
    table_index: usize,
    dma_control: u32,
    #[serde(with = "crate::state::matrix")]
    quant: [[u8; 64]; 2],
    #[serde(with = "crate::state::array")]
    scale: [i16; 64],
    input: VecDeque<u16>,
    output: VecDeque<u32>,
    #[serde(with = "crate::state::array")]
    coefficients: [i32; 64],
    coefficient_index: Option<usize>,
    quant_scale: i32,
    #[serde(with = "crate::state::matrix")]
    chroma: [[i32; 64]; 2],
    block: usize,
    output_block: usize,
    output_depth: u32,
    output_position: usize,
    commands: u64,
    blocks_decoded: u64,
}

impl From<&Mdec> for MdecV1 {
    fn from(value: &Mdec) -> Self {
        Self {
            operation: value.operation.into(),
            command: value.command,
            remaining: value.remaining,
            status_count: value.status_count,
            table_index: value.table_index,
            dma_control: value.dma_control,
            quant: value.quant,
            scale: value.scale,
            input: value.input.clone(),
            output: value.output.clone(),
            coefficients: value.coefficients,
            coefficient_index: value.coefficient_index,
            quant_scale: value.quant_scale,
            chroma: value.chroma,
            block: value.block,
            output_block: value.output_block,
            output_depth: value.output_depth as u32,
            output_position: value.output_position,
            commands: value.commands,
            blocks_decoded: value.blocks_decoded,
        }
    }
}

impl TryFrom<MdecV1> for Mdec {
    type Error = String;

    fn try_from(value: MdecV1) -> Result<Self, Self::Error> {
        let device = Self {
            operation: value.operation.into(),
            command: value.command,
            remaining: value.remaining,
            status_count: value.status_count,
            table_index: value.table_index,
            dma_control: value.dma_control,
            quant: value.quant,
            scale: value.scale,
            input: value.input,
            output: value.output,
            coefficients: value.coefficients,
            coefficient_index: value.coefficient_index,
            quant_scale: value.quant_scale,
            chroma: value.chroma,
            block: value.block,
            output_block: value.output_block,
            output_depth: value.output_depth.try_into()?,
            output_position: value.output_position,
            commands: value.commands,
            blocks_decoded: value.blocks_decoded,
        };
        device.validate_state()?;
        Ok(device)
    }
}

crate::state::v1::impl_serde_v1!(Mdec, MdecV1);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdec::OutputDepth;

    #[test]
    fn v1_preserves_output_depth_codes() {
        for (code, depth) in [
            (0, OutputDepth::Mono4),
            (1, OutputDepth::Mono8),
            (2, OutputDepth::Rgb24),
            (3, OutputDepth::Rgb15),
        ] {
            let mut state = MdecV1::from(&Mdec::default());
            state.output_depth = code;
            let restored = Mdec::try_from(state).unwrap();
            assert_eq!(restored.output_depth, depth);
            assert_eq!(MdecV1::from(&restored).output_depth, code);
        }
        let mut state = MdecV1::from(&Mdec::default());
        state.output_depth = 4;
        assert!(Mdec::try_from(state).is_err());
    }

    #[test]
    fn direct_deserialization_bounds_the_entire_table_transfer() {
        for (command, limit, stride) in [
            (0x4000_0000, 64, 4),
            (0x4000_0001, 128, 4),
            (0x6000_0000, 64, 2),
        ] {
            let mut device = Mdec::default();
            device.write_data(command).unwrap();
            for (index, remaining) in [(1, 1), (limit - stride, 2), (usize::MAX, 1)] {
                let mut state = MdecV1::from(&device);
                state.table_index = index;
                state.remaining = remaining;
                let bytes = bincode::serialize(&state).unwrap();
                assert!(bincode::deserialize::<Mdec>(&bytes).is_err());
            }
            let mut partial = MdecV1::from(&device);
            partial.table_index = limit - stride;
            partial.remaining = 1;
            let bytes = bincode::serialize(&partial).unwrap();
            let mut restored: Mdec = bincode::deserialize(&bytes).unwrap();
            restored.write_data(0x1234_5678).unwrap();
            assert!(restored.operation == Operation::Idle);
        }
    }
}

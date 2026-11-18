//! Frozen V1 disc controller state.
use super::{CdRom, Command, Response};
use crate::{
    cd_audio::CdAudioV1,
    disc::{DiscReferenceV1, DiscRegionV1},
};
use smallvec::SmallVec;
use std::collections::VecDeque;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Command")]
struct CommandV1 {
    opcode: u8,
    parameters: Vec<u8>,
    clocks: u32,
}

impl From<&Command> for CommandV1 {
    fn from(state: &Command) -> Self {
        Self {
            opcode: state.opcode,
            parameters: state.parameters.to_vec(),
            clocks: state.clocks,
        }
    }
}

impl From<CommandV1> for Command {
    fn from(state: CommandV1) -> Self {
        Self {
            opcode: state.opcode,
            parameters: SmallVec::from_vec(state.parameters),
            clocks: state.clocks,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Response")]
struct ResponseV1 {
    interrupt: u8,
    bytes: Vec<u8>,
}

impl From<&Response> for ResponseV1 {
    fn from(state: &Response) -> Self {
        Self {
            interrupt: state.interrupt,
            bytes: state.bytes.to_vec(),
        }
    }
}

impl From<ResponseV1> for Response {
    fn from(state: ResponseV1) -> Self {
        Self {
            interrupt: state.interrupt,
            bytes: SmallVec::from_vec(state.bytes),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "CdRom")]
#[allow(clippy::struct_excessive_bools)] // Independent V1 drive/status bits.
pub(crate) struct CdromV1 {
    disc: Option<DiscReferenceV1>,
    region: Option<DiscRegionV1>,
    index: u8,
    parameters: Vec<u8>,
    response: VecDeque<u8>,
    irq_enable: u8,
    irq_flags: u8,
    command: Option<CommandV1>,
    completion: Option<(u32, ResponseV1)>,
    play_completion: Option<ResponseV1>,
    mode: u8,
    filter: [u8; 2],
    location: u32,
    reported_location: u32,
    target: Option<u32>,
    motor: bool,
    reading: bool,
    playing: bool,
    play_track: u8,
    audio: CdAudioV1,
    muted: bool,
    read_clocks: u32,
    sector: Vec<u8>,
    data: VecDeque<u8>,
    last_header: Option<[u8; 8]>,
    trace: Option<VecDeque<String>>,
    sectors_read: u64,
    cdda_sectors: u64,
    xa_sectors: u64,
    filtered_sectors: u64,
    commands: u64,
    command_errors: u64,
    last_error_code: Option<u8>,
    last_command: Option<u8>,
    last_error: Option<String>,
}

impl From<&CdRom> for CdromV1 {
    fn from(state: &CdRom) -> Self {
        Self {
            disc: state.disc.as_ref().map(Into::into),
            region: state.region.as_ref().map(Into::into),
            index: state.index,
            parameters: state.parameters.to_vec(),
            response: state.response.clone(),
            irq_enable: state.irq_enable,
            irq_flags: state.irq_flags,
            command: state.command.as_ref().map(Into::into),
            completion: state
                .completion
                .as_ref()
                .map(|(clocks, response)| (*clocks, response.into())),
            play_completion: state.play_completion.as_ref().map(Into::into),
            mode: state.mode,
            filter: state.filter,
            location: state.location,
            reported_location: state.reported_location,
            target: state.target,
            motor: state.motor,
            reading: state.reading,
            playing: state.playing,
            play_track: state.play_track,
            audio: (&state.audio).into(),
            muted: state.muted,
            read_clocks: state.read_clocks,
            sector: state.sector.clone(),
            data: state.data.clone(),
            last_header: state.last_header,
            trace: state.trace.clone(),
            sectors_read: state.sectors_read,
            cdda_sectors: state.cdda_sectors,
            xa_sectors: state.xa_sectors,
            filtered_sectors: state.filtered_sectors,
            commands: state.commands,
            command_errors: state.command_errors,
            last_error_code: state.last_error_code,
            last_command: state.last_command,
            last_error: state.last_error.clone(),
        }
    }
}

impl TryFrom<CdromV1> for CdRom {
    type Error = String;

    fn try_from(state: CdromV1) -> Result<Self, Self::Error> {
        let device = Self {
            disc: state.disc.map(TryInto::try_into).transpose()?,
            region: state.region.map(Into::into),
            index: state.index,
            parameters: SmallVec::from_vec(state.parameters),
            response: state.response,
            irq_enable: state.irq_enable,
            irq_flags: state.irq_flags,
            command: state.command.map(Into::into),
            completion: state
                .completion
                .map(|(clocks, response)| (clocks, response.into())),
            play_completion: state.play_completion.map(Into::into),
            mode: state.mode,
            filter: state.filter,
            location: state.location,
            reported_location: state.reported_location,
            target: state.target,
            motor: state.motor,
            reading: state.reading,
            playing: state.playing,
            play_track: state.play_track,
            audio: state.audio.into(),
            muted: state.muted,
            read_clocks: state.read_clocks,
            sector: state.sector,
            data: state.data,
            last_header: state.last_header,
            trace: state.trace,
            sectors_read: state.sectors_read,
            cdda_sectors: state.cdda_sectors,
            xa_sectors: state.xa_sectors,
            filtered_sectors: state.filtered_sectors,
            commands: state.commands,
            command_errors: state.command_errors,
            last_error_code: state.last_error_code,
            last_command: state.last_command,
            last_error: state.last_error,
        };
        device.validate_state()?;
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::CdRom;

    #[test]
    fn direct_cdrom_decode_checks_drive_state() {
        let state = CdRom {
            reading: true,
            ..CdRom::default()
        };
        let bytes = bincode::serialize(&state).unwrap();
        assert!(bincode::deserialize::<CdRom>(&bytes).is_err());
    }
}

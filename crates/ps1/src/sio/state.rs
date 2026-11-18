//! Frozen V1 representation of Sio.
use super::{PadPhase, Sio};
use crate::memory_card::MemoryCardV1;
use std::collections::VecDeque;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "PadPhase")]
enum PadPhaseV1 {
    Address,
    Command,
    Id,
    LowButtons,
    HighButtons,
    Card,
    Finished,
}

impl From<PadPhase> for PadPhaseV1 {
    fn from(value: PadPhase) -> Self {
        match value {
            PadPhase::Address => Self::Address,
            PadPhase::Command => Self::Command,
            PadPhase::Id => Self::Id,
            PadPhase::LowButtons => Self::LowButtons,
            PadPhase::HighButtons => Self::HighButtons,
            PadPhase::Card => Self::Card,
            PadPhase::Finished => Self::Finished,
        }
    }
}

impl From<PadPhaseV1> for PadPhase {
    fn from(value: PadPhaseV1) -> Self {
        match value {
            PadPhaseV1::Address => Self::Address,
            PadPhaseV1::Command => Self::Command,
            PadPhaseV1::Id => Self::Id,
            PadPhaseV1::LowButtons => Self::LowButtons,
            PadPhaseV1::HighButtons => Self::HighButtons,
            PadPhaseV1::Card => Self::Card,
            PadPhaseV1::Finished => Self::Finished,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Sio")]
pub(crate) struct SioV1 {
    mode: u16,
    control: u16,
    baud: u16,
    holding: Option<(u8, bool)>,
    transmitting: Option<(u8, u32)>,
    received: VecDeque<u8>,
    last_received: u8,
    phase: PadPhaseV1,
    buttons: u16,
    sampled_buttons: u16,
    ack_delay: u32,
    ack_pulse: u32,
    interrupt: bool,
    interrupt_edge: bool,
    transfers: u64,
    polls: u64,
    cards: [Option<MemoryCardV1>; 2],
}

impl From<&Sio> for SioV1 {
    fn from(value: &Sio) -> Self {
        Self {
            mode: value.mode,
            control: value.control,
            baud: value.baud,
            holding: value.holding,
            transmitting: value.transmitting,
            received: value.received.clone(),
            last_received: value.last_received,
            phase: value.phase.into(),
            buttons: value.buttons,
            sampled_buttons: value.sampled_buttons,
            ack_delay: value.ack_delay,
            ack_pulse: value.ack_pulse,
            interrupt: value.interrupt,
            interrupt_edge: value.interrupt_edge,
            transfers: value.transfers,
            polls: value.polls,
            cards: value
                .cards
                .each_ref()
                .map(|card| card.as_ref().map(MemoryCardV1::from)),
        }
    }
}

impl TryFrom<SioV1> for Sio {
    type Error = String;

    fn try_from(value: SioV1) -> Result<Self, Self::Error> {
        let [first, second] = value.cards;
        let device = Self {
            mode: value.mode,
            control: value.control,
            baud: value.baud,
            holding: value.holding,
            transmitting: value.transmitting,
            received: value.received,
            last_received: value.last_received,
            phase: value.phase.into(),
            buttons: value.buttons,
            sampled_buttons: value.sampled_buttons,
            ack_delay: value.ack_delay,
            ack_pulse: value.ack_pulse,
            interrupt: value.interrupt,
            interrupt_edge: value.interrupt_edge,
            transfers: value.transfers,
            polls: value.polls,
            cards: [
                first.map(TryInto::try_into).transpose()?,
                second.map(TryInto::try_into).transpose()?,
            ],
        };
        device.validate_state()?;
        Ok(device)
    }
}

crate::state::v1::impl_serde_v1!(Sio, SioV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_flight_countdown_survives_baud_change() {
        let mut sio = Sio::default();
        sio.write(8, 2, 0x0f);
        sio.write(14, 2, 0xffff);
        sio.write(10, 2, 3);
        sio.write(0, 1, 1);
        sio.tick(17);
        sio.write(14, 2, 1);
        let bytes = bincode::serialize(&sio).unwrap();
        let mut restored: Sio = bincode::deserialize(&bytes).unwrap();
        let clocks = sio.transmitting.unwrap().1;
        restored.tick(clocks);
        sio.tick(clocks);
        assert_eq!(
            bincode::serialize(&restored).unwrap(),
            bincode::serialize(&sio).unwrap()
        );

        let mut invalid = SioV1::from(&sio);
        invalid.transmitting = Some((1, 0));
        assert!(bincode::deserialize::<Sio>(&bincode::serialize(&invalid).unwrap()).is_err());
    }
}

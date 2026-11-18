//! Frozen V1 representation of MemoryCard.
use super::MemoryCard;
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "MemoryCard")]
pub(crate) struct MemoryCardV1 {
    bytes: Arc<[u8]>,
    flag: u8,
    position: usize,
    command: u8,
    address: u16,
    checksum: u8,
    previous: u8,
    #[serde(with = "crate::state::array")]
    buffer: [u8; 128],
    result: u8,
    active: bool,
    revision: u64,
    reads: u64,
    writes: u64,
}

impl From<&MemoryCard> for MemoryCardV1 {
    fn from(value: &MemoryCard) -> Self {
        Self {
            bytes: Arc::clone(&value.bytes),
            flag: value.flag,
            position: value.position,
            command: value.command,
            address: value.address,
            checksum: value.checksum,
            previous: value.previous,
            buffer: value.buffer,
            result: value.result,
            active: value.active,
            revision: value.revision,
            reads: value.reads,
            writes: value.writes,
        }
    }
}

impl TryFrom<MemoryCardV1> for MemoryCard {
    type Error = String;

    fn try_from(value: MemoryCardV1) -> Result<Self, Self::Error> {
        let device = Self {
            bytes: value.bytes,
            flag: value.flag,
            position: value.position,
            command: value.command,
            address: value.address,
            checksum: value.checksum,
            previous: value.previous,
            buffer: value.buffer,
            result: value.result,
            active: value.active,
            revision: value.revision,
            reads: value.reads,
            writes: value.writes,
        };
        device.validate_state()?;
        Ok(device)
    }
}

crate::state::v1::impl_serde_v1!(MemoryCard, MemoryCardV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_sector_read_can_resume_until_the_card_rejects_the_address() {
        let mut card = MemoryCard::formatted();
        card.select();
        for byte in [0x52, 0, 0, 0xff, 0xff, 0, 0, 0] {
            card.exchange(byte);
        }
        let bytes = bincode::serialize(&card).unwrap();
        let mut restored: MemoryCard = bincode::deserialize(&bytes).unwrap();
        assert_eq!(restored.exchange(0), (0xff, None));
        let mut invalid = MemoryCardV1::from(&card);
        invalid.position = 9;
        assert!(
            bincode::deserialize::<MemoryCard>(&bincode::serialize(&invalid).unwrap()).is_err()
        );
    }
}

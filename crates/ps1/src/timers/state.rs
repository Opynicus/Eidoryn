//! Frozen V1 representation of Timers.
use super::{Counter, Timers};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Counter")]
#[allow(clippy::struct_excessive_bools)] // Independent V1 hardware latches.
struct CounterV1 {
    count: u16,
    mode: u16,
    target: u16,
    divider: u8,
    hold: u8,
    fired: bool,
    pulse: bool,
    reset_next: bool,
    blank: bool,
    released: bool,
}

impl From<&Counter> for CounterV1 {
    fn from(value: &Counter) -> Self {
        Self {
            count: value.count,
            mode: value.mode,
            target: value.target,
            divider: value.divider,
            hold: value.hold,
            fired: value.fired,
            pulse: value.pulse,
            reset_next: value.reset_next,
            blank: value.blank,
            released: value.released,
        }
    }
}

impl From<CounterV1> for Counter {
    fn from(value: CounterV1) -> Self {
        Self {
            count: value.count,
            mode: value.mode,
            target: value.target,
            divider: value.divider,
            hold: value.hold,
            fired: value.fired,
            pulse: value.pulse,
            reset_next: value.reset_next,
            blank: value.blank,
            released: value.released,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Timers")]
pub(crate) struct TimersV1 {
    counters: [CounterV1; 3],
}

impl From<&Timers> for TimersV1 {
    fn from(value: &Timers) -> Self {
        Self {
            counters: value.counters.each_ref().map(CounterV1::from),
        }
    }
}

impl TryFrom<TimersV1> for Timers {
    type Error = String;

    fn try_from(value: TimersV1) -> Result<Self, Self::Error> {
        let device = Self {
            counters: value.counters.map(Into::into),
        };
        if device
            .counters
            .iter()
            .any(|counter| counter.mode & !0x1fff != 0 || counter.hold > 2 || counter.divider >= 8)
        {
            return Err("Invalid saved timer state".into());
        }
        Ok(device)
    }
}

crate::state::v1::impl_serde_v1!(Timers, TimersV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_deserialization_rejects_invalid_divider_phase() {
        let mut state = TimersV1::from(&Timers::default());
        state.counters[2].divider = 8;
        let bytes = bincode::serialize(&state).unwrap();
        assert!(bincode::deserialize::<Timers>(&bytes).is_err());
    }
}

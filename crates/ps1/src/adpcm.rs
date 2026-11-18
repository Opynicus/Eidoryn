//! Shared arithmetic for the two PS1 ADPCM formats; packet layouts live in their devices.
#[derive(Clone, Copy, Default)]
pub(crate) struct Predictor {
    history: [i32; 2],
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Predictor")]
pub(crate) struct PredictorV1 {
    history: [i32; 2],
}

impl From<&Predictor> for PredictorV1 {
    fn from(state: &Predictor) -> Self {
        Self {
            history: state.history,
        }
    }
}

impl From<PredictorV1> for Predictor {
    fn from(state: PredictorV1) -> Self {
        Self {
            history: state.history,
        }
    }
}

impl Predictor {
    pub(crate) fn valid_history(self) -> bool {
        self.history
            .iter()
            .all(|&sample| i16::try_from(sample).is_ok())
    }

    pub fn decode(&mut self, signed: i32, shift: u8, filter: usize) -> i16 {
        const COEFFICIENTS: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];
        let (recent, older) = COEFFICIENTS.get(filter).copied().unwrap_or((0, 0));
        let shift = if shift <= 12 { shift } else { 9 };
        let prediction = (self.history[0] * recent + self.history[1] * older + 32) >> 6;
        let sample = ((signed >> shift) + prediction).clamp(-32768, 32767);
        self.history = [sample, self.history[0]];
        sample as i16
    }
}

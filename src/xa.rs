//! Original XA packet decoder and 37.8/18.9 kHz to 44.1 kHz reconstruction.
use crate::{adpcm::Predictor, spu::StereoFrame};

#[derive(Clone, Default)]
pub(crate) struct XaDecoder {
    predictors: [Predictor; 2],
    history: [[i32; 2]; 32],
    cursor: usize,
    sixstep: usize,
}

impl XaDecoder {
    pub fn decode(&mut self, raw: &[u8], mut output: impl FnMut(StereoFrame)) {
        if raw.len() < 24 + 18 * 128 {
            return;
        }
        let coding = raw[19];
        let stereo = coding & 1 != 0;
        let eight_bit = coding & 0x10 != 0;
        let half_rate = coding & 4 != 0;
        let blocks = if eight_bit { 4 } else { 8 };
        for group in raw[24..24 + 18 * 128].as_chunks::<128>().0 {
            let mut left = [0; 28];
            for block in 0..blocks {
                let channel = if stereo { block & 1 } else { 0 };
                let header = group[4 + block];
                let mut decoded = [0; 28];
                for (index, sample) in decoded.iter_mut().enumerate() {
                    let signed = if eight_bit {
                        i32::from(group[16 + index * 4 + block] as i8) << 8
                    } else {
                        let nibble = (group[16 + index * 4 + block / 2] >> ((block & 1) * 4)) & 15;
                        i32::from((u16::from(nibble) << 12) as i16)
                    };
                    *sample = self.predictors[channel].decode(
                        signed,
                        header & 15,
                        usize::from((header >> 4) & 3),
                    );
                }
                if stereo && channel == 0 {
                    left = decoded;
                    continue;
                }
                for index in 0..28 {
                    let frame = [
                        if stereo { left[index] } else { decoded[index] },
                        decoded[index],
                    ];
                    self.resample(frame, &mut output);
                    if half_rate {
                        self.resample(frame, &mut output);
                    }
                }
            }
        }
    }

    fn resample(&mut self, frame: StereoFrame, output: &mut impl FnMut(StereoFrame)) {
        self.history[self.cursor] = frame.map(i32::from);
        self.cursor = (self.cursor + 1) & 31;
        self.sixstep += 1;
        if self.sixstep != 6 {
            return;
        }
        self.sixstep = 0;
        for weights in ZIGZAG {
            let mut sum = [0_i32; 2];
            for (index, weight) in weights.iter().enumerate() {
                let sample = self.history[self.cursor.wrapping_sub(index + 1) & 31];
                for side in 0..2 {
                    sum[side] += (sample[side] * weight) >> 15;
                }
            }
            output(sum.map(|sample| sample.clamp(-32768, 32767) as i16));
        }
    }
}

// Hardware filter coefficients from PSX-SPX (not an emulator implementation).
// https://psx-spx.consoledev.net/cdromformat/#25-point-zigzag-interpolation
const ZIGZAG: [[i32; 29]; 7] = [
    [
        0, 0, 0, 0, 0, -2, 10, -34, 65, -84, 52, 9, -266, 1024, -2680, 9036, 26516, -6016, 3021,
        -1571, 848, -365, 107, 10, -16, 17, -8, 3, -1,
    ],
    [
        0, 0, 0, -2, 0, 3, -19, 60, -75, 162, -227, 306, -67, -615, 3229, 29883, -4532, 2488,
        -1471, 882, -424, 166, -27, 5, 6, -8, 3, -1, 0,
    ],
    [
        0, 0, -1, 3, -2, -5, 31, -74, 179, -402, 689, -926, 1272, -1446, 31033, -1446, 1272, -926,
        689, -402, 179, -74, 31, -5, -2, 3, -1, 0, 0,
    ],
    [
        0, -1, 3, -8, 6, 5, -27, 166, -424, 882, -1471, 2488, -4532, 29883, 3229, -615, -67, 306,
        -227, 162, -75, 60, -19, 3, 0, -2, 0, 0, 0,
    ],
    [
        -1, 3, -8, 17, -16, 10, 107, -365, 848, -1571, 3021, -6016, 26516, 9036, -2680, 1024, -266,
        9, 52, -84, 65, -34, 10, -1, 0, 1, 0, 0, 0,
    ],
    [
        2, -8, 16, -35, 43, 26, -235, 635, -1352, 2810, -5882, 21472, 15367, -4681, 2062, -839,
        347, -68, -23, 70, -35, 17, -5, 0, 0, 0, 0, 0, 0,
    ],
    [
        -5, 17, -35, 70, -23, -68, 347, -839, 2062, -4681, 15367, 21472, -5882, 2810, -1352, 635,
        -235, 26, 43, -35, 16, -8, 2, 0, 0, 0, 0, 0, 0,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xa_layout_rates_channels_and_filter_history_are_preserved() {
        for (coding, count) in [
            (1, 2352),
            (0, 4704),
            (5, 4704),
            (4, 9408),
            (0x11, 1176),
            (0x10, 2352),
        ] {
            let mut raw = [0_u8; 2352];
            raw[19] = coding;
            for group in raw[24..2328].as_chunks_mut::<128>().0 {
                group[16..].fill(if coding & 0x10 == 0 { 0x21 } else { 0x40 });
            }
            let mut decoder = XaDecoder::default();
            let mut samples = Vec::new();
            decoder.decode(&raw, |sample| samples.push(sample));
            assert_eq!(samples.len(), count);
            assert!(samples[100..].iter().all(|s| s[0] > 3000 && s[1] > 3000));
            if coding & 1 == 0 || coding & 0x10 != 0 {
                assert!(samples.iter().all(|s| s[0] == s[1]));
            } else {
                assert!(
                    samples[100..]
                        .iter()
                        .all(|s| (i32::from(s[0]) * 2 - i32::from(s[1])).abs() < 40)
                );
            }
            let mut continuation = Vec::new();
            decoder.decode(&raw, |sample| continuation.push(sample));
            assert!(continuation[0][0] > 3000); // Filter ring survives sector boundaries.
        }
    }
}

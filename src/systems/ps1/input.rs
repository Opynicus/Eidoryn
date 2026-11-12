//! Host-side digital pad turbo, clocked by emulated CPU time rather than key repeat.
use super::timing::CPU_CLOCK_HZ;

/// Only action and shoulder buttons can repeat; navigation remains direct.
pub const TURBO_BUTTONS: u16 = 0xff00;
pub const TURBO_RATES: [u8; 4] = [5, 10, 15, 20];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurboConfig {
    buttons: u16,
    hz: u8,
}

impl Default for TurboConfig {
    fn default() -> Self {
        Self { buttons: 0, hz: 10 }
    }
}

impl TurboConfig {
    pub fn new(buttons: u16, hz: u8) -> Result<Self, String> {
        if buttons & !TURBO_BUTTONS != 0 || !TURBO_RATES.contains(&hz) {
            return Err(
                "Turbo requires action/shoulder buttons and a rate of 5, 10, 15 or 20 Hz".into(),
            );
        }
        Ok(Self { buttons, hz })
    }

    pub const fn buttons(self) -> u16 {
        self.buttons
    }

    pub const fn hz(self) -> u8 {
        self.hz
    }
}

#[derive(Default)]
pub struct PadInput {
    config: TurboConfig,
    held: u16,
    started: [u64; 8],
}

impl PadInput {
    pub fn configure(&mut self, config: TurboConfig, cycles: u64) {
        if self.config != config {
            self.config = config;
            self.started.fill(cycles);
        }
    }

    /// A new press starts down immediately. Unrelated/repeated events keep its phase.
    pub fn set_buttons(&mut self, held: u16, cycles: u64) {
        let pressed = held & !self.held;
        for (index, start) in self.started.iter_mut().enumerate() {
            if pressed & (1 << (index + 8)) != 0 {
                *start = cycles;
            }
        }
        self.held = held & !6; // Digital pads have no L3/R3.
    }

    pub fn buttons(&self, cycles: u64) -> u16 {
        let mut buttons = self.held;
        let repeating = self.held & self.config.buttons;
        if repeating != 0 {
            let half_period = u64::from(CPU_CLOCK_HZ) / (2 * u64::from(self.config.hz));
            for (index, start) in self.started.iter().enumerate() {
                let bit = 1 << (index + 8);
                if repeating & bit != 0 && (cycles.wrapping_sub(*start) / half_period) % 2 != 0 {
                    buttons &= !bit;
                }
            }
        }
        buttons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: u16 = 1 << 15;
    const CROSS: u16 = 1 << 14;
    const RIGHT: u16 = 1 << 5;

    #[test]
    fn disabled_turbo_passes_through_digital_buttons() {
        let mut input = PadInput::default();
        input.set_buttons(u16::MAX, 0);
        assert_eq!(input.buttons(u64::from(CPU_CLOCK_HZ)), 0xfff9);
    }

    #[test]
    fn supported_rates_pulse_only_held_selected_buttons() {
        for hz in TURBO_RATES {
            let mut input = PadInput::default();
            input.configure(TurboConfig::new(SQUARE, hz).unwrap(), 0);
            input.set_buttons(SQUARE | CROSS | RIGHT, 0);
            let half = u64::from(CPU_CLOCK_HZ) / (2 * u64::from(hz));
            for pulse in 0..u64::from(hz) {
                let start = 2 * pulse * half;
                assert_eq!(input.buttons(start), SQUARE | CROSS | RIGHT);
                assert_eq!(input.buttons(start + half - 1), SQUARE | CROSS | RIGHT);
                assert_eq!(input.buttons(start + half), CROSS | RIGHT);
                assert_eq!(input.buttons(start + 2 * half - 1), CROSS | RIGHT);
            }
            input.set_buttons(0, half);
            assert_eq!(input.buttons(2 * half), 0);
        }
    }

    #[test]
    fn buttons_have_independent_phases_and_repress_starts_down() {
        let mut input = PadInput::default();
        input.configure(TurboConfig::new(SQUARE | CROSS, 10).unwrap(), 0);
        let half = u64::from(CPU_CLOCK_HZ) / 20;
        input.set_buttons(SQUARE, 0);
        input.set_buttons(SQUARE | CROSS | RIGHT, half);
        assert_eq!(input.buttons(half), CROSS | RIGHT);
        input.set_buttons(SQUARE | CROSS | RIGHT, half + 1);
        assert_eq!(input.buttons(half + 1), CROSS | RIGHT);
        assert_eq!(input.buttons(2 * half), SQUARE | RIGHT);
        input.set_buttons(RIGHT, 2 * half);
        input.set_buttons(SQUARE | RIGHT, 3 * half);
        assert_eq!(input.buttons(3 * half), SQUARE | RIGHT);
    }

    #[test]
    fn reconfiguration_and_cycle_wrap_do_not_leave_buttons_stuck() {
        let mut input = PadInput::default();
        let half = u64::from(CPU_CLOCK_HZ) / 20;
        let start = u64::MAX - half / 2;
        input.configure(TurboConfig::new(SQUARE, 10).unwrap(), start);
        input.set_buttons(SQUARE, start);
        let off = start.wrapping_add(half);
        assert_eq!(input.buttons(off), 0);
        input.configure(TurboConfig::default(), off);
        assert_eq!(input.buttons(off), SQUARE);
        input.configure(TurboConfig::new(SQUARE, 20).unwrap(), off);
        assert_eq!(input.buttons(off), SQUARE);
        assert_eq!(input.buttons(off + half / 2), 0);
        input.set_buttons(0, off + half / 2);
        assert_eq!(input.buttons(off + half), 0);
    }

    #[test]
    fn invalid_settings_are_rejected() {
        for buttons in [1, 6, 8, RIGHT, u16::MAX] {
            assert!(TurboConfig::new(buttons, 10).is_err());
        }
        for hz in [0, 1, 21, u8::MAX] {
            assert!(TurboConfig::new(SQUARE, hz).is_err());
        }
    }
}

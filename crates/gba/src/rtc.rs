//! S-3511 cartridge clock connected to SCK, SIO and chip select on the ROM GPIO.
use crate::{Error, RTC_BATTERY_LEN};
use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

const DAY: i64 = 86_400;
const CENTURY: i64 = 36_525 * DAY; // Calendar years 2000..2099, including 2000.
const EPOCH_2000: i64 = 946_684_800;
const MODE_24H: u8 = 0x40;
const CONTROL_MASK: u8 = MODE_24H | 0x2a;

#[derive(Clone, Default, Serialize, Deserialize)]
enum Transfer {
    #[default]
    Idle,
    Command {
        byte: u8,
        bits: u8,
    },
    Read {
        data: [u8; 7],
        bit: u8,
        len: u8,
    },
    Write {
        register: u8,
        data: [u8; 7],
        bit: u8,
        len: u8,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Rtc {
    control: u8,
    offset: i64,
    weekday_offset: u8,
    enabled: bool,
    pins: u8,
    direction: u8,
    output: u8,
    transfer: Transfer,
}

impl Default for Rtc {
    fn default() -> Self {
        Self {
            control: MODE_24H,
            offset: 0,
            weekday_offset: 0,
            enabled: false,
            pins: 0,
            direction: 0,
            output: 0,
            transfer: Transfer::Idle,
        }
    }
}

impl Rtc {
    pub(super) fn detect(rom: &[u8]) -> Option<Self> {
        memchr::memmem::find(rom, b"SIIRTC_V").map(|_| Self::default())
    }
    pub(super) fn reset_bus(&mut self) {
        self.enabled = false;
        self.direction = 0;
        self.pins = 0;
        self.output = 0;
        self.transfer = Transfer::Idle;
    }
    pub(super) fn valid(&self) -> bool {
        self.control & !CONTROL_MASK == 0
            && self.weekday_offset < 7
            && self.offset.unsigned_abs() <= CENTURY as u64
            && self.direction <= 15
            && self.pins <= 15
            && self.output <= 1
            && match self.transfer {
                Transfer::Idle => true,
                Transfer::Command { bits, .. } => bits < 8,
                Transfer::Read { bit, len, .. } => matches!(len, 1 | 3 | 7) && bit < len * 8,
                Transfer::Write {
                    register, bit, len, ..
                } => matches!((register, len), (1, 1) | (2, 7) | (3, 3)) && bit < len * 8,
            }
    }
    pub(super) fn battery(&self) -> [u8; RTC_BATTERY_LEN] {
        let mut bytes = [0; RTC_BATTERY_LEN];
        bytes[..8].copy_from_slice(&self.offset.to_le_bytes());
        bytes[8] = self.control;
        bytes[9] = self.weekday_offset;
        bytes
    }
    pub(super) fn restore_battery(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let [lo @ .., control, weekday]: &[u8; RTC_BATTERY_LEN] =
            bytes.try_into().map_err(|_| Error::InvalidRtcBattery)?;
        let offset = i64::from_le_bytes(*lo);
        if control & !CONTROL_MASK != 0 || *weekday >= 7 || offset.unsigned_abs() > CENTURY as u64 {
            return Err(Error::InvalidRtcBattery);
        }
        self.offset = offset;
        self.control = *control;
        self.weekday_offset = *weekday;
        Ok(())
    }
    pub(super) fn read_enabled(&self) -> bool {
        self.enabled
    }
    pub(super) fn read(&self, addr: usize) -> u8 {
        match addr {
            0xc4 => (self.pins & self.direction) | ((self.output << 1) & !self.direction),
            0xc6 => self.direction,
            0xc8 => u8::from(self.enabled),
            _ => 0,
        }
    }
    pub(super) fn write(&mut self, addr: usize, value: u8) {
        let previous = self.pins & self.direction;
        match addr {
            0xc4 => self.pins = value & 15,
            0xc6 => self.direction = value & 15,
            0xc8 => {
                self.enabled = value & 1 != 0;
                return;
            }
            _ => return,
        }
        let levels = self.pins & self.direction;
        if levels & 4 == 0 {
            self.transfer = Transfer::Idle;
            self.output = 0;
        } else if previous & 4 == 0 {
            self.transfer = Transfer::Command { byte: 0, bits: 0 };
        } else if previous & 1 == 0 && levels & 1 != 0 {
            self.clock((levels >> 1) & 1);
        }
    }
    fn clock(&mut self, input: u8) {
        match &mut self.transfer {
            Transfer::Command { byte, bits } if self.direction & 2 != 0 => {
                *byte = (*byte << 1) | input;
                *bits += 1;
                if *bits == 8 {
                    let command = *byte;
                    self.transfer = self.command(command);
                }
            }
            Transfer::Read { data, bit, len } if self.direction & 2 == 0 => {
                self.output = (data[usize::from(*bit / 8)] >> (*bit % 8)) & 1;
                *bit += 1;
                if *bit == *len * 8 {
                    self.transfer = Transfer::Idle;
                }
            }
            Transfer::Write {
                data,
                bit,
                len,
                register,
            } if self.direction & 2 != 0 => {
                data[usize::from(*bit / 8)] |= input << (*bit % 8);
                *bit += 1;
                if *bit == *len * 8 {
                    let (r, d) = (*register, *data);
                    self.program(r, d, now());
                    self.transfer = Transfer::Idle;
                }
            }
            _ => {}
        }
    }
    fn command(&mut self, cmd: u8) -> Transfer {
        let cmd = if cmd >> 4 == 6 {
            cmd
        } else {
            cmd.reverse_bits()
        };
        if cmd >> 4 != 6 {
            return Transfer::Idle;
        }
        let register = cmd >> 1 & 7;
        let len = match register {
            0 => {
                self.control = 0;
                self.set_timestamp(EPOCH_2000, now());
                self.weekday_offset = 1; // 2000-01-01 is Saturday; the reset counter reads zero.
                return Transfer::Idle;
            }
            1 => 1,
            2 => 7,
            3 => 3,
            _ => return Transfer::Idle,
        };
        if cmd & 1 == 0 {
            return Transfer::Write {
                register,
                data: [0; 7],
                bit: 0,
                len,
            };
        }
        let mut data = [0; 7];
        match register {
            1 => data[0] = self.control,
            2 => data = self.datetime(now()),
            3 => data[..3].copy_from_slice(&self.datetime(now())[4..]),
            _ => unreachable!(),
        }
        Transfer::Read { data, bit: 0, len }
    }
    fn datetime(&self, host: i64) -> [u8; 7] {
        let seconds = (host - EPOCH_2000 + self.offset).rem_euclid(CENTURY);
        let date =
            OffsetDateTime::from_unix_timestamp(EPOCH_2000 + seconds).expect("date in 2000..2100");
        let hour = if self.control & MODE_24H == 0 {
            bcd(date.hour() % 12) | if date.hour() >= 12 { 0x80 } else { 0 }
        } else {
            bcd(date.hour())
        };
        [
            bcd((date.year() - 2000) as u8),
            bcd(date.month() as u8),
            bcd(date.day()),
            (date.weekday().number_days_from_sunday() + self.weekday_offset) % 7,
            hour,
            bcd(date.minute()),
            bcd(date.second()),
        ]
    }
    fn set_timestamp(&mut self, absolute: i64, host: i64) {
        self.offset = absolute - EPOCH_2000 - (host - EPOCH_2000).rem_euclid(CENTURY);
    }
    fn program(&mut self, register: u8, mut payload: [u8; 7], host: i64) {
        if register == 1 {
            self.control = payload[0] & CONTROL_MASK;
            return;
        }
        if register == 3 {
            payload.copy_within(..3, 4);
            payload[..4].copy_from_slice(&self.datetime(host)[..4]);
        }
        let Some((date, time)) = decode_datetime(payload, self.control & MODE_24H != 0) else {
            return;
        };
        self.weekday_offset = (payload[3] + 7 - date.weekday().number_days_from_sunday()) % 7;
        self.set_timestamp(
            PrimitiveDateTime::new(date, time)
                .assume_utc()
                .unix_timestamp(),
            host,
        );
    }
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
fn bcd(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}
fn decimal(b: u8) -> Option<u8> {
    (b >> 4 < 10 && b & 15 < 10).then_some((b >> 4) * 10 + (b & 15))
}
fn decode_datetime(b: [u8; 7], mode_24h: bool) -> Option<(Date, Time)> {
    if b[3] >= 7 {
        return None;
    }
    let date = Date::from_calendar_date(
        2000 + i32::from(decimal(b[0])?),
        Month::try_from(decimal(b[1])?).ok()?,
        decimal(b[2])?,
    )
    .ok()?;
    let raw = decimal(b[4] & 0x7f)?;
    let hour = if mode_24h {
        raw
    } else if raw <= 12 {
        raw % 12 + if b[4] & 0x80 != 0 { 12 } else { 0 }
    } else {
        return None;
    };
    Some((
        date,
        Time::from_hms(hour, decimal(b[5])?, decimal(b[6])?).ok()?,
    ))
}

#[cfg(test)]
mod tests;

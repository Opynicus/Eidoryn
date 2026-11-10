//! Original SIO0, port-1 digital pad and two optional memory-card slots.
//! https://psx-spx.consoledev.net/serialinterfacessio/
//! https://psx-spx.consoledev.net/controllersandmemorycards/
use crate::memory_card::Cards;
use std::collections::VecDeque;

pub const ACK_DELAY: u32 = 300;
// A controller must assert /ACK for at least 2 us. Model that minimum at 33.8688 MHz;
// the host can acknowledge the previous interrupt once this pulse has ended.
pub const ACK_PULSE: u32 = 68;

#[derive(Clone, Copy, Default)]
enum PadPhase {
    #[default]
    Address,
    Command,
    Id,
    LowButtons,
    HighButtons,
    Card,
    Finished,
}

#[derive(Clone, Default)]
pub struct Sio {
    mode: u16,
    control: u16,
    baud: u16,
    holding: Option<(u8, bool)>,
    transmitting: Option<(u8, u32)>,
    received: VecDeque<u8>,
    last_received: u8,
    phase: PadPhase,
    buttons: u16,
    sampled_buttons: u16,
    ack_delay: u32,
    ack_pulse: u32,
    interrupt: bool,
    interrupt_edge: bool,
    pub transfers: u64,
    pub polls: u64,
    pub cards: Cards,
}

impl Sio {
    /// Host input is active-high; the serial pad response is active-low.
    pub fn set_buttons(&mut self, pressed: u16) {
        self.buttons = pressed & !6; // A digital pad has no L3/R3 switches.
    }

    pub fn buttons(&self) -> u16 {
        self.buttons
    }

    pub fn take_irq_edge(&mut self) -> bool {
        std::mem::take(&mut self.interrupt_edge)
    }

    fn check_interrupt(&mut self) {
        let threshold = 1 << ((self.control >> 8) & 3);
        let active = (self.control & 0x400 != 0 && self.holding.is_none())
            || (self.control & 0x800 != 0 && self.received.len() >= threshold)
            || (self.control & 0x1000 != 0 && self.ack_pulse != 0);
        if active && !self.interrupt {
            self.interrupt = true;
            self.interrupt_edge = true;
        }
    }

    fn bit_clocks(&self) -> u32 {
        let factor = match self.mode & 3 {
            2 => 16,
            3 => 64,
            _ => 1,
        };
        ((u32::from(self.baud) * factor) & !1).max(1)
    }

    fn start_transmit(&mut self) {
        if self.transmitting.is_none()
            && self
                .holding
                .is_some_and(|(_, enabled)| enabled || self.control & 1 != 0)
        {
            let (byte, _) = self.holding.take().expect("pending transmit byte");
            let bits = 5 + u32::from((self.mode >> 2) & 3);
            self.transmitting = Some((byte, self.bit_clocks() * bits));
        }
    }

    fn exchange(&mut self, byte: u8) -> (u8, Option<u32>) {
        // A deselected device, absent port/card or unsupported wire format floats high.
        if self.control & 2 == 0 || self.mode & 0x11c != 0x0c {
            return (0xff, None);
        }
        let port = usize::from((self.control >> 13) & 1);
        let (reply, acknowledge, next) = match self.phase {
            PadPhase::Address if byte == 0x81 && self.cards[port].is_some() => {
                self.cards[port].as_mut().expect("inserted card").select();
                (0xff, true, PadPhase::Card)
            }
            PadPhase::Card => {
                return self.cards[port]
                    .as_mut()
                    .map_or((0xff, None), |card| card.exchange(byte));
            }
            PadPhase::Address if byte == 1 && port == 0 => (0xff, true, PadPhase::Command),
            PadPhase::Command if byte == 0x42 => {
                self.sampled_buttons = !self.buttons;
                self.polls += 1;
                (0x41, true, PadPhase::Id)
            }
            PadPhase::Id => (0x5a, true, PadPhase::LowButtons),
            PadPhase::LowButtons => (self.sampled_buttons as u8, true, PadPhase::HighButtons),
            PadPhase::HighButtons => ((self.sampled_buttons >> 8) as u8, false, PadPhase::Finished),
            _ => (0xff, false, PadPhase::Finished),
        };
        self.phase = next;
        (reply, acknowledge.then_some(ACK_DELAY))
    }

    /// Baud-derived byte timing; ACK delay/pulse are approximate device timings.
    #[inline]
    pub fn tick(&mut self, clocks: u32) {
        // All interrupt-producing changes are handled by register writes or
        // transfer/ACK edges. An idle controller has nothing to advance.
        if self.holding.is_none()
            && self.transmitting.is_none()
            && self.ack_delay == 0
            && self.ack_pulse == 0
        {
            return;
        }
        self.tick_active(clocks);
    }

    fn tick_active(&mut self, mut clocks: u32) {
        self.start_transmit();
        while clocks != 0 {
            let next = [
                self.transmitting.map_or(0, |(_, n)| n),
                self.ack_delay,
                self.ack_pulse,
            ]
            .into_iter()
            .filter(|n| *n != 0)
            .min();
            let Some(next) = next else {
                break;
            };
            let elapsed = clocks.min(next);
            clocks -= elapsed;
            self.ack_pulse = self.ack_pulse.saturating_sub(elapsed);
            if self.ack_delay != 0 {
                self.ack_delay -= elapsed;
                if self.ack_delay == 0 {
                    self.ack_pulse = ACK_PULSE;
                }
            }
            if let Some((byte, remaining)) = self.transmitting {
                if remaining > elapsed {
                    self.transmitting = Some((byte, remaining - elapsed));
                } else {
                    self.transmitting = None;
                    let (reply, ack) = self.exchange(byte);
                    self.transfers += 1;
                    if self.control & 6 != 0 {
                        if self.received.len() == 8 {
                            self.received.pop_back();
                        }
                        self.received.push_back(reply);
                        self.last_received = reply;
                        self.control &= !4; // Forced receive is single-byte on SIO0.
                    }
                    if let Some(delay) = ack {
                        self.ack_delay = delay;
                    }
                    self.start_transmit();
                }
            }
            self.check_interrupt();
        }
        self.check_interrupt();
    }

    pub fn status(&self) -> u32 {
        let half_period = (self.bit_clocks() / 2).max(1);
        let baud_timer = self.transmitting.map_or(0, |(_, n)| n % half_period);
        u32::from(self.holding.is_none())
            | (u32::from(!self.received.is_empty()) << 1)
            | (u32::from(self.holding.is_none() && self.transmitting.is_none()) << 2)
            | (u32::from(self.ack_pulse != 0) << 7)
            | (u32::from(self.interrupt) << 9)
            | (baud_timer << 11)
    }

    pub fn read(&mut self, address: u32, width: u32) -> u32 {
        let offset = address & 15;
        if offset == 0 {
            let mut value = 0;
            for n in 0..width {
                value |= u32::from(
                    self.received
                        .get(n as usize)
                        .copied()
                        .unwrap_or(self.last_received),
                ) << (n * 8);
            }
            // Halfword previews two bytes but consumes only one; word consumes four.
            for _ in 0..if width == 4 { 4 } else { 1 } {
                self.received.pop_front();
            }
            return value;
        }
        let registers = [
            self.status(),
            u32::from(self.mode) | (u32::from(self.control) << 16),
            u32::from(self.baud) << 16,
        ];
        if (4..16).contains(&offset) {
            let value = registers[(offset / 4 - 1) as usize] >> ((offset & 3) * 8);
            return value
                & match width {
                    1 => 0xff,
                    2 => 0xffff,
                    _ => u32::MAX,
                };
        }
        0
    }

    pub fn write(&mut self, address: u32, width: u32, value: u32) {
        let offset = address & 15;
        if offset == 0 {
            self.holding = Some((value as u8, self.control & 1 != 0));
            self.start_transmit();
            self.check_interrupt();
            return;
        }
        if width == 4 {
            self.write(address, 2, value & 0xffff);
            self.write(address + 2, 2, value >> 16);
            return;
        }
        let value = if width == 1 {
            let old = self.read(address & !1, 2) as u16;
            let shift = (offset & 1) * 8;
            (old & !(0xff << shift)) | ((value as u16 & 255) << shift)
        } else {
            value as u16
        };
        match offset & !1 {
            8 => self.mode = value & 0x13f,
            10 => {
                if value & 0x40 != 0 {
                    *self = Self {
                        buttons: self.buttons,
                        transfers: self.transfers,
                        polls: self.polls,
                        cards: std::mem::take(&mut self.cards),
                        ..Self::default()
                    };
                    self.deselect_cards();
                    return;
                }
                if value & 2 == 0 || (self.control ^ value) & 0x2000 != 0 {
                    self.phase = PadPhase::Address;
                    self.ack_delay = 0;
                    self.ack_pulse = 0;
                    self.deselect_cards();
                }
                self.control = value & 0x3f07;
                if value & 0x10 != 0 {
                    self.interrupt = false;
                }
            }
            14 => self.baud = value,
            _ => {} // Status and unused registers are read-only.
        }
        self.start_transmit();
        self.check_interrupt();
    }

    pub fn deselect_cards(&mut self) {
        for card in self.cards.iter_mut().flatten() {
            card.deselect();
        }
    }
}

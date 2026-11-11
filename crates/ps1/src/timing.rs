//! Shared system clock used by the CPU and timed PS1 devices.
/// Nominal PS1 CPU clock in cycles per second; devices derive their clocks from it.
pub const CPU_CLOCK_HZ: u32 = 33_868_800;

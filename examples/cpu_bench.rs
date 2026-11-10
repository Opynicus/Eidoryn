//! Original ALU loop; measures native execution together with normal device ticks.
#[allow(dead_code)]
#[path = "support/cpu.rs"]
mod cpu;
#[allow(dead_code)]
#[path = "support/renderer.rs"]
mod renderer;

fn main() -> Result<(), String> {
    use eidoryn_psx::{cpu::Cpu, demo, machine::Machine};
    let mut machine = Machine::from_exe(&demo::executable(), "Original integer loop")
        .map_err(|e| e.to_string())?;
    machine.bus.ram.fill(0);
    machine.cpu = Cpu::new(demo::ENTRY);
    for n in 0..64 {
        let word = match n % 4 {
            0 => 0x2421_0001, // addiu r1,r1,1
            1 => 0x0021_1026, // xor r2,r1,r1
            2 => 0x0022_1821, // addu r3,r1,r2
            _ => 0x0003_2040, // sll r4,r3,1
        };
        machine
            .bus
            .write32(demo::ENTRY + n * 4, word)
            .map_err(|e| e.to_string())?;
    }
    machine
        .bus
        .write32(demo::ENTRY + 256, 0x0800_4000)
        .map_err(|e| e.to_string())?;
    cpu::benchmark(&machine, 30_000_000)
}

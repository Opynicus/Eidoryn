//! Observe guest SIO0 accesses after a bounded BIOS boot, without changing guest code.
use eidoryn_psx::{bus::physical, cpu::disassemble, media::load_disc};

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let bios = std::path::PathBuf::from(args.next().ok_or("BIOS path required")?);
    let disc = std::path::PathBuf::from(args.next().ok_or("Disc path required")?);
    let boot_steps: u64 = args
        .next()
        .ok_or("boot step count required")?
        .to_str()
        .ok_or("invalid step count")?
        .parse::<u64>()
        .map_err(|e| e.to_string())?;
    let mut machine = load_disc(&disc, Some(&bios), false)?;
    for _ in 0..boot_steps {
        machine.step().map_err(|e| e.to_string())?;
    }
    // One reference run and one Start press from exactly the same machine state.
    let mut neutral = machine.clone();
    machine.bus.sio.set_buttons(1 << 3);
    for step in 0..2_000_000 {
        let pc = machine.cpu.pc;
        let word = machine.bus.peek32(pc).ok_or("unmapped PC")?;
        let opcode = word >> 26;
        let base = machine.cpu.registers[((word >> 21) & 31) as usize];
        let address = physical(base.wrapping_add_signed(i32::from(word as i16)));
        if matches!(opcode, 0x20..=0x25 | 0x28 | 0x29 | 0x2b)
            && (0x1f80_1040..=0x1f80_104f).contains(&address)
        {
            let width = match opcode & 3 {
                0 => 1,
                1 => 2,
                _ => 4,
            };
            let value = if opcode < 0x28 {
                machine.bus.sio.clone().read(address, width)
            } else {
                machine.cpu.registers[((word >> 16) & 31) as usize]
            };
            // Busy-loop status reads are less useful than data/control traffic.
            if address != 0x1f80_1044 {
                println!(
                    "{step:7} PC={pc:08X} {} address={address:08X} value={value:08X}",
                    disassemble(pc, word)
                );
            }
        }
        machine.step().map_err(|e| e.to_string())?;
        neutral.step().map_err(|e| e.to_string())?;
    }
    let mut differences = 0;
    for (address, (pressed, released)) in machine.bus.ram.iter().zip(&neutral.bus.ram).enumerate() {
        if pressed != released {
            if differences < 256 {
                println!("RAM {address:08X}: Start={pressed:02X} neutral={released:02X}");
            }
            differences += 1;
        }
    }
    println!(
        "RAM differences={differences}; Start polls={} neutral polls={}",
        machine.bus.sio.polls, neutral.bus.sio.polls
    );
    Ok(())
}

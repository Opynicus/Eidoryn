//! Bounded diagnostic runner. Only paths are supplied; no firmware/disc is bundled.
use eidoryn_psx::{card_file::CardStore, cpu::disassemble, media::load_disc};

#[allow(clippy::too_many_lines)] // Keep the bounded trace and final device report together.
fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let bios = std::path::PathBuf::from(args.next().ok_or("BIOS path required")?);
    let disc = std::path::PathBuf::from(args.next().ok_or("Disc path required")?);
    let limit: u64 = args
        .next()
        .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
        .unwrap_or(50_000_000);
    let frame_path = args.next().map(std::path::PathBuf::from);
    let inputs = args
        .next()
        .map(|path| {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            text.lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                .map(|line| {
                    let mut fields = line.split_whitespace();
                    let step = fields
                        .next()
                        .ok_or("input event needs step and hex button mask")?
                        .parse::<u64>()
                        .map_err(|e| e.to_string())?;
                    let mask = u16::from_str_radix(
                        fields
                            .next()
                            .ok_or("input event needs mask")?
                            .trim_start_matches("0x"),
                        16,
                    )
                    .map_err(|e| e.to_string())?;
                    if fields.next().is_some() {
                        return Err("input event has extra fields".into());
                    }
                    Ok((step, mask))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?
        .unwrap_or_default();
    if !inputs.windows(2).all(|pair| pair[0].0 < pair[1].0)
        || inputs.last().is_some_and(|event| event.0 >= limit)
    {
        return Err("Input event steps must increase and be below the step limit".into());
    }
    let mut capture_inputs = false;
    let mut card_paths = [None, None];
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--capture-inputs") if frame_path.is_some() && !inputs.is_empty() => {
                capture_inputs = true;
            }
            Some("--card1" | "--card2") => {
                let slot = usize::from(flag == "--card2");
                card_paths[slot] = Some(std::path::PathBuf::from(
                    args.next().ok_or("Missing card path")?,
                ));
            }
            _ => {
                return Err(
                    "Expected --capture-inputs, --card1 PATH or --card2 PATH after input script"
                        .into(),
                );
            }
        }
    }
    let mut inputs = inputs.into_iter().peekable();
    let mut machine = load_disc(&disc, Some(&bios), false)?;
    let (mut card_store, cards) = CardStore::open(card_paths)?;
    machine.bus.sio.cards = cards;
    let mut history = std::collections::VecDeque::with_capacity(32);
    let mut failure = None;
    let mut cd_errors = 0;
    for n in 0..limit {
        if inputs.peek().is_some_and(|event| event.0 == n) {
            if capture_inputs {
                let path = frame_path.as_ref().expect("validated capture path");
                write_frame(&machine, &path.with_extension(format!("{n}.ppm")))?;
            }
            let (_, mask) = inputs.next().expect("pending input");
            machine.bus.sio.set_buttons(mask);
            println!("input step={n} mask={mask:04X}");
        }
        if history.len() == 32 {
            history.pop_front();
        }
        history.push_back((machine.cpu.pc, machine.bus.peek32(machine.cpu.pc)));
        if let Err(error) = machine.step() {
            failure = Some(error.to_string());
            break;
        }
        if machine.bus.cdrom.command_errors != cd_errors {
            cd_errors = machine.bus.cdrom.command_errors;
            println!(
                "CD error step={n} command={:?} code={:?} detail={:?}",
                machine.bus.cdrom.last_command,
                machine.bus.cdrom.last_error_code,
                machine.bus.cdrom.last_error
            );
        }
        if (n + 1) % 5_000_000 == 0 {
            card_store.flush(&machine.bus.sio.cards)?;
            println!(
                "step={} PC={:08X} GP0={} CDcommands={} sectors={} POST={:X} card R/W={:?}",
                n + 1,
                machine.cpu.pc,
                machine.bus.gpu.commands,
                machine.bus.cdrom.commands,
                machine.bus.cdrom.sectors_read,
                machine.bus.post,
                machine
                    .bus
                    .sio
                    .cards
                    .each_ref()
                    .map(|card| card.as_ref().map(|card| (card.reads, card.writes)))
            );
        }
    }
    card_store.flush(&machine.bus.sio.cards)?;
    if let Some(path) = frame_path {
        write_frame(&machine, &path)?;
    }
    println!("Last instructions:");
    for (pc, word) in history {
        if let Some(word) = word {
            println!("{pc:08X} {word:08X} {}", disassemble(pc, word));
        }
    }
    for (name, value) in eidoryn_psx::cpu::REGISTER_NAMES
        .iter()
        .zip(machine.cpu.registers)
    {
        println!("{name:>4}={value:08X}");
    }
    let irq_status = machine.bus.read32(0x1f80_1070).map_err(|e| e.to_string())?;
    let irq_mask = machine.bus.read32(0x1f80_1074).map_err(|e| e.to_string())?;
    println!(
        "instructions={} PC={:08X} faultPC={:08X} GP0={} GTE={} MDECblocks={} CDcommands={} lastCD={:?} sectors={} SPUhalfwords={} SIOtransfers={} padPolls={} I_STAT={:08X} I_MASK={:08X}",
        machine.cpu.instructions,
        machine.cpu.pc,
        machine.cpu.instruction_address(),
        machine.bus.gpu.commands,
        machine.cpu.gte.commands,
        machine.bus.mdec.blocks_decoded,
        machine.bus.cdrom.commands,
        machine.bus.cdrom.last_command,
        machine.bus.cdrom.sectors_read,
        machine.bus.spu.transferred_halfwords,
        machine.bus.sio.transfers,
        machine.bus.sio.polls,
        irq_status,
        irq_mask
    );
    failure.map_or(Ok(()), Err)
}

fn write_frame(
    machine: &eidoryn_psx::machine::Machine,
    path: &std::path::Path,
) -> Result<(), String> {
    use std::io::Write;
    let (width, height) = machine.bus.gpu.dimensions();
    let mut file =
        std::fs::File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    write!(file, "P6\n{width} {height}\n255\n").map_err(|e| e.to_string())?;
    file.write_all(&machine.bus.gpu.display_rgb())
        .map_err(|e| e.to_string())
}

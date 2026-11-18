//! Conversion between execution state and the V1 pipeline layout.
use super::Cpu;
use crate::gte::GteV1;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Cpu")]
pub(crate) struct CpuV1 {
    registers: [u32; 32],
    pc: u32,
    hi: u32,
    lo: u32,
    cop0: [u32; 32],
    gte: GteV1,
    instructions: u64,
    exceptions: u64,
    next_pc: u32,
    instruction_pc: u32,
    branch_pending: bool,
    in_delay_slot: bool,
    load: Option<(usize, u32)>,
    last_step_cycles: u32,
    muldiv_ready: Option<u64>,
}

impl From<&Cpu> for CpuV1 {
    fn from(value: &Cpu) -> Self {
        Self {
            registers: value.registers,
            pc: value.pc,
            hi: value.hi,
            lo: value.lo,
            cop0: value.cop0,
            gte: (&value.gte).into(),
            instructions: value.instructions,
            exceptions: value.exceptions,
            next_pc: value.next_pc,
            instruction_pc: value.instruction_pc,
            branch_pending: value.branch_pending,
            in_delay_slot: value.in_delay_slot,
            load: value.load,
            last_step_cycles: value.last_step_cycles,
            muldiv_ready: value.muldiv_ready,
        }
    }
}

impl TryFrom<CpuV1> for Cpu {
    type Error = String;

    fn try_from(value: CpuV1) -> Result<Self, Self::Error> {
        if value.load.is_some_and(|(register, _)| register >= 32) {
            return Err("Invalid saved CPU load register".into());
        }
        if value.registers[0] != 0 {
            return Err("Invalid saved CPU zero register".into());
        }
        Ok(Self {
            registers: value.registers,
            pc: value.pc,
            hi: value.hi,
            lo: value.lo,
            cop0: value.cop0,
            gte: value.gte.try_into()?,
            instructions: value.instructions,
            exceptions: value.exceptions,
            next_pc: value.next_pc,
            instruction_pc: value.instruction_pc,
            branch_pending: value.branch_pending,
            in_delay_slot: value.in_delay_slot,
            load: value.load,
            last_step_cycles: value.last_step_cycles,
            muldiv_ready: value.muldiv_ready,
        })
    }
}

crate::state::v1::impl_serde_v1!(Cpu, CpuV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gte_roundtrips_extended_registers_and_rejects_out_of_range_depth() {
        let mut cpu = Cpu::new(0);
        cpu.gte.write_control(26, 0xffff);
        cpu.gte.write_data(9, 0x8000);
        cpu.gte.execute(0x28);
        let bytes = bincode::serialize(&cpu).unwrap();
        let restored: Cpu = bincode::deserialize(&bytes).unwrap();
        assert_eq!(bincode::serialize(&restored).unwrap(), bytes);

        // V1 stores all 32 data registers first, including zero-extended OTZ.
        let mut bytes = bincode::serialize(&cpu.gte).unwrap();
        bytes[7 * 4..8 * 4].copy_from_slice(&0x1_0000_u32.to_le_bytes());
        assert!(bincode::deserialize::<crate::gte::Gte>(&bytes).is_err());
    }

    #[test]
    fn direct_cpu_decode_checks_delayed_load_and_allows_pending_address_fault() {
        let mut cpu = Cpu::new(0x8000_0003);
        cpu.load = Some((31, 0x1234));
        let bytes = bincode::serialize(&cpu).unwrap();
        let restored: Cpu = bincode::deserialize(&bytes).unwrap();
        assert_eq!(restored.pc, cpu.pc);
        assert_eq!(restored.load, cpu.load);

        cpu.load = Some((32, 0x1234));
        let bytes = bincode::serialize(&cpu).unwrap();
        assert!(bincode::deserialize::<Cpu>(&bytes).is_err());
    }
}

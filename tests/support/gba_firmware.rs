/// Original ARM firmware for tests. Its reset, SWI and IRQ vectors leave markers in SRAM.
pub fn firmware() -> Vec<u8> {
    let words = [
        0xea00_0006_u32, // reset: B 0x20
        0xeaff_fffe,     // undefined: B .
        0xea00_000c,     // SWI: B 0x40
        0xeaff_fffe,
        0xeaff_fffe,
        0xeaff_fffe,
        0xea00_0010, // IRQ: B 0x60
        0xeaff_fffe,
        0xe10f_2000, // reset: MRS r2, CPSR
        0xe3a0_040e, // MOV r0, #SRAM
        0xe5c0_2083, // STRB r2, [r0, #0x83]
        0xe5d0_1080, // LDRB r1, [r0, #0x80]
        0xe281_1001, // ADD r1, r1, #1
        0xe5c0_1080, // STRB r1, [r0, #0x80]
        0xe321_f01f, // MSR CPSR_c, #0x1f (system mode)
        0xe3a0_f302, // MOV pc, #ROM
        0xe10f_1000, // swi: MRS r1, CPSR
        0xe3a0_040e, // MOV r0, #SRAM
        0xe5c0_1084, // STRB r1, [r0, #0x84]
        0xe14f_1000, // MRS r1, SPSR
        0xe311_0020, // TST r1, #T (caller was Thumb?)
        0x03a0_00a1, // MOVEQ r0, #0xa1
        0x13a0_0072, // MOVNE r0, #0x72
        0xe1b0_f00e, // MOVS pc, lr (restore CPSR)
        0xe3a0_1301, // irq: MOV r1, #IO
        0xe281_1c02, // ADD r1, r1, #0x200
        0xe3a0_0001, // MOV r0, #VBlank
        0xe1c1_00b2, // STRH r0, [r1, #2] (acknowledge IF)
        0xe3a0_140e, // MOV r1, #SRAM
        0xe5d1_0081, // LDRB r0, [r1, #0x81]
        0xe280_0001, // ADD r0, r0, #1
        0xe5c1_0081, // STRB r0, [r1, #0x81]
        0xe14f_0000, // MRS r0, SPSR
        0xe5c1_0082, // STRB r0, [r1, #0x82]
        0xe25e_f004, // SUBS pc, lr, #4
    ];
    let mut image = vec![0; eidoryn_gba::BIOS_LEN];
    for (dst, op) in image.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *dst = op.to_le_bytes();
    }
    image
}

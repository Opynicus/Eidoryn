# Native execution boundary

This crate contains the original integer x86-64 emitter and the only unsafe
native-call boundary in Eidoryn. The emulator crate keeps `unsafe_code = forbid`.
The backend currently accepts only Linux x86-64, using the SysV AMD64 ABI.

- Callers provide typed integer operations, never code bytes or executable addresses.
  Blocks contain 1–64 operations. Every register and shift index is checked below 32.
- Generated memory operations access only a supplied exclusive `[u32; 32]`, with
  fixed offsets 0–124. Register zero is never written. Guest values cannot become
  host addresses, jump targets, callback addresses or instruction lengths.
- Code is built in anonymous RW memory and converted to RX before publication.
  The mutable mapping is consumed. No interface exposes or modifies published code.
- The three arguments are register storage, a live callback context and the
  monomorphized trampoline matching that context. They remain valid for the entire
  synchronous call. No pointer is retained after return. Immutable code can be
  shared, but each execution has separate register/context borrows.
- r12–r14 are saved and restored on every exit. Three pushes change entry
  `rsp % 16 == 8` to `rsp % 16 == 0` before each CALL. Other callee-saved registers,
  SIMD registers, the direction flag and FP control state are untouched. The
  return address and stack are restored on every bounded exit path.
- The gate sees boundary zero and each completed operation, including the last.
  It can stop execution before the next operation. A panic is caught entirely
  inside Rust; generated frames have no unwinding metadata. After the native
  function returns, Rust resumes the captured panic.
- Core code must maintain guest PC/delay/interrupt/clock semantics in the gate;
  this low-level API itself promises only the documented integer transformations.

References: [SysV AMD64 ABI](https://gitlab.com/x86-psABIs/x86-64-ABI),
[Intel instruction encodings](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[memmap2 anonymous mappings and make_exec](https://docs.rs/memmap2/0.9.12/memmap2/struct.MmapMut.html).
No emulator implementation is copied or linked.

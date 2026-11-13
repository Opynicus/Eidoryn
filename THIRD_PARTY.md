# Eidoryn dependencies and test artifacts

The emulator CPU, bus, software/Vulkan GPU algorithms and shaders, DMA, disc reader, CD-ROM controller, EXE
loader, CUE parser, GTE, MDEC, SIO/digital pad, memory-card protocol/persistence,
SPU voices/ADSR/reverb, XA/CDDA decoding, timers and MIPS test programs are implemented locally. No third-party emulator
core, emulator adapter, or vendored emulator source is used.
The application now depends on local `eidoryn-ps1` (hardware) and `eidoryn-host`
(audio, filters, themes) crates. With optional features disabled, `eidoryn-host`
uses only the Rust standard library. `eidoryn-ps1` uses smallvec 1.16.4
(MIT/Apache-2.0, `union` feature) for private CD-ROM command parameters and delayed
responses. The inline capacities are 16 and 8 bytes; larger values can still use
heap storage. Existing sector and FIFO buffers retain their reusable capacities.

The optional `vulkan` feature uses wgpu 29.0.4 for safe Vulkan resource and compute
submission APIs, pollster 0.4.0 for initialization, and bytemuck 1.25.2 for safe
integer-slice byte views. These are general-purpose libraries; PS1 command
encoding, dependency tracking, rasterization and scanout are implemented here.
API reference: https://docs.rs/wgpu/29.0.4/wgpu/ . The default build includes `desktop`, `vulkan` and `jit`;
runtime CPU/renderer selections are independent.

The desktop frontend uses GPUI 0.2.2, image, serde and serde_json (theme manifests),
and CPAL 0.16.0 for host audio-device access (not PS1 sound emulation),
with libc pinned for GPUI's older xattr dependency. Their transitive dependencies and exact versions
are recorded in Cargo.lock and retain their own licenses. This project keeps
its existing GPL-3.0-only package license; LICENSE-MIT preserves the original
prototype's notice.

GPUI's transitive `proc-macro-error2` 2.0.1 dependency and its attribute crate are
pinned to commit `53ff94b61c678593158db1a3d50269c6f0de0db7` via Cargo's patch table.
The source change makes the `proc_macro` re-export public to fix Rust's future
incompatibility warning; see [upstream PR #14](https://github.com/GnomedDev/proc-macro-error-2/pull/14).
Both crates retain their MIT/Apache-2.0 licenses.

The optional `jit` feature uses the local `eidoryn-jit-runtime` crate, an original
MIPS-integer-to-x86-64 emitter and safe native execution API. Its only external
library is memmap2 0.9.12 for anonymous RW-to-RX executable mappings (MIT/Apache-2.0).
No emulator recompiler or third-party code generator is copied or linked.
The main emulator keeps `unsafe_code = forbid`; the isolated native-call boundary
is documented and tested in crates/jit-runtime/SAFETY.md.

## Hardware references

The implementation refers to the PSX-SPX hardware research documentation:

- https://psx-spx.consoledev.net/cdromdrive/
- https://psx-spx.consoledev.net/cdromformat/
- https://psx-spx.consoledev.net/dmachannels/
- https://psx-spx.consoledev.net/cdromfileformats/
- https://psx-spx.consoledev.net/graphicsprocessingunitgpu/
- https://psx-spx.consoledev.net/soundprocessingunitspu/
- https://psx-spx.consoledev.net/timers/
- https://psx-spx.consoledev.net/expansionportpio/
- https://psx-spx.consoledev.net/geometrytransformationenginegte/
- https://psx-spx.consoledev.net/macroblockdecodermdec/
- https://psx-spx.consoledev.net/serialinterfacessio/
- https://psx-spx.consoledev.net/controllersandmemorycards/

These are behavior and register references, not source dependencies.
The 512-entry SPU Gaussian ROM table and seven XA zigzag filter tables are
hardware coefficients transcribed from these documents. The surrounding
decoder, interpolation, mixer and device state machines are implemented here;
no CoffeePSX or other emulator implementation was copied or linked.

## Distributed assets

`assets/eidoryn.png` is an AI-generated icon created for this project. It is embedded
in the GPUI header. Test programs and disc generation scripts are original.
No game images, proprietary BIOS, save files, gameplay captures, or local testing
artifacts are distributed with the application.

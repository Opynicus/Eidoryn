# Eidoryn dependencies and test artifacts

The emulator CPU, bus, software/wgpu GPU algorithms and shaders, DMA, disc reader, CD-ROM controller, EXE
loader, CUE parser, GTE, MDEC, SIO/digital pad, memory-card protocol/persistence,
SPU voices/ADSR/reverb, XA/CDDA decoding, timers and MIPS test programs are implemented locally. No third-party emulator
core, emulator adapter, or vendored emulator source is used.
The application now depends on local `eidoryn-ps1` (hardware) and `eidoryn-host`
(audio, filters, themes) crates. With optional features disabled, `eidoryn-host`
uses only the Rust standard library. `eidoryn-ps1` uses smallvec 1.16.5
(MIT/Apache-2.0, `union` feature) for private CD-ROM command parameters and delayed
responses. The inline capacities are 16 and 8 bytes; larger values can still use
heap storage. Existing sector and FIFO buffers retain their reusable capacities.

The optional `dx12`, `metal`, `vulkan` and `opengl` features use wgpu 30.0.2 for safe resource and compute
submission APIs, pollster 1.0.1 for initialization, and bytemuck 1.25.2 for safe
integer-slice byte views. These are general-purpose libraries; PS1 command
encoding, dependency tracking, rasterization and scanout are implemented here.
OpenGL uses wgpu's `gles` backend (native WGL on Windows), including glow and
glutin_wgl_sys bindings; the emulator introduces no unsafe OpenGL calls.
API reference: https://docs.rs/wgpu/30.0.2/wgpu/ . The default build includes `desktop`, `hardware` (all four backends) and `jit`;
runtime CPU/renderer selections are independent.

Cargo.lock resolves gpu-allocator 0.28.0's Windows API dependency to windows 0.62.2,
matching wgpu-hal's DirectX 12 interface types. Other dependencies may retain older
Windows API versions; these cannot be substituted across the allocator boundary.

The desktop frontend uses GPUI, image, serde and serde_json (theme manifests),
and CPAL 0.18.2 for host audio-device access (not PS1 sound emulation),
with libc pinned for GPUI's older xattr dependency. Their transitive dependencies and exact versions
are recorded in Cargo.lock and retain their own licenses. This project keeps
its existing GPL-3.0-only package license; LICENSE-MIT preserves the original
prototype's notice.

GPUI (Apache-2.0) is fetched directly from the official Zed repository at commit
[`a66dac7b3a065014e9729e25ea5ac4322014b037`](https://github.com/zed-industries/zed/commit/a66dac7b3a065014e9729e25ea5ac4322014b037).
This source revision is newer than the crates.io 0.2.2 release and contains
[upstream PR #42227](https://github.com/zed-industries/zed/pull/42227), which fixes
Windows file-drop crashes by passing `STGMEDIUM.hGlobal` directly as an `HDROP`.
The complete Git revision and Cargo.lock fix the dependency source; its package
version label alone does not identify this fix. No GPUI source is vendored or
patched locally. The upstream license and source notice are preserved under
`licenses/gpui` and included in release packages.

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

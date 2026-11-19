# Eidoryn

[![Platform: PlayStation](https://img.shields.io/badge/Platform-PlayStation-6750A4?style=flat-square)](#current-status)
[![Rust: 1.95+](https://img.shields.io/badge/Rust-1.95%2B-DEA584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![UI: GPUI](https://img.shields.io/badge/UI-GPUI-7C4DFF?style=flat-square)](https://www.gpui.rs/)
[![Status: Experimental](https://img.shields.io/badge/Status-Experimental-E9A23B?style=flat-square)](#current-status)
[![License: GPL--3.0](https://img.shields.io/badge/License-GPL--3.0--only-4C8B64?style=flat-square)](LICENSE)

Experimental project developed with extensive AI assistance. Hardware accuracy and game compatibility are still under active development.

**English** | [简体中文](README.zh-CN.md)

[Project Overview](#project-overview) · [Features](#features) · [Current Status](#current-status) · [Getting Started](#getting-started) · [Controls](#controls) · [Architecture](#architecture) · [Documentation](#documentation) · [Roadmap](#roadmap) · [License & Acknowledgments](#license--acknowledgments)

## Project Overview

Eidoryn is an experimental emulator written in Rust with a native GPUI interface. Its current PlayStation (PS1) core provides game execution, graphics settings, and hardware debugging. The CPU, GPU, audio, disc drive, and other emulated components are implemented in this project; no third-party emulator core is integrated.

The architecture leaves room for additional systems, but **GBA and PS2 are not currently supported**. No game images, BIOS files, or saves are included.

## Features

- **PS1 emulation:** MIPS CPU, GTE, GPU, MDEC, SPU, disc drive, DMA, timers, a shared CPU/device cycle timeline, and a digital controller.
- **Graphics and performance options:** serial and multithreaded software rendering, optional wgpu hardware acceleration with runtime backend detection, enlarged and fullscreen views, and a CRT display filter.
- **Desktop experience:** English and Simplified Chinese, keyboard navigation, four built-in themes, JSON theme plugins, saved preferences, a volume slider, and system audio output.
- **Media and saves:** ISO, BIN, multi-track CUE/BIN, and CHD CD images; two memory-card slots with automatic persistence, plus timestamped save states for returning to any saved moment.
- **Debugging tools:** An optional debugger with CPU registers, disassembly, single stepping, and execution slices; an experimental JIT and a headless CLI. Enable the debugger in its workspace or start with `--debugger`.

## Current Status

| Area | Status |
| --- | --- |
| Emulated systems | PlayStation (PS1) is in development; other systems are not implemented. |
| Host platforms | Linux x86-64 has been used for development and testing. Windows and macOS headless CLI builds have passed CI smoke tests; their desktop interfaces still need hands-on runtime validation. |
| Rendering | Software rendering is the default. **Settings → Graphics / F8** offers optional hardware acceleration and detects usable backends. Windows automatic selection prefers DirectX 12, then Vulkan, then OpenGL. |
| CPU JIT | Experimental and disabled by default; supports a subset of integer instructions on Linux x86-64 only. |
| Accuracy and compatibility | Device timing, graphics, and audio are not cycle-accurate. Compatibility and performance vary by game, scene, and host. |

Disc swapping is not supported. Memory-card saves use the game's own save points or menus; save states can be created while running or paused.

## Getting Started

### Requirements

- Rust **1.95 or newer** and Cargo. A release build is recommended for running games.
- The desktop build requires GPUI's system graphics, font, and audio development libraries, plus a working graphics driver. See the [release workflow](https://github.com/Opynicus/Eidoryn/blob/main/.github/workflows/release.yml) for the Linux build dependencies.
- Hardware rendering requires a physical GPU and driver with compute shader support. Windows supports DirectX 12, Vulkan and OpenGL 4.3+; older devices without a compatible hardware backend can use software rendering.
- Bring your own BIOS and game images; they are not supplied by this project.

This UI redesign branch produces temporary test releases for Linux x86-64 Wayland, Linux x86-64 X11, and Windows x86-64, with SHA-256 checksums. These prereleases are built from the test branch and do not replace the latest release. On Linux, choose the archive matching your desktop session; Ubuntu 24.04 or a compatible newer system is required. On Windows, extract the full ZIP and run `eidoryn.exe` from its directory. Windows desktop display, input, and audio still need hands-on validation; native CI covers compilation, tests, and the headless demo. See [GitHub Releases](https://github.com/Opynicus/Eidoryn/releases) for available test packages.

### Build and launch

```bash
git clone https://github.com/Opynicus/Eidoryn.git
cd Eidoryn
cargo run --locked --release
```

Without media, Eidoryn opens its original built-in demo without requiring a BIOS. After building, the executable is at `target/release/eidoryn` on Linux and macOS, or `target/release/eidoryn.exe` on Windows. `--locked` fixes dependency versions; `--release` enables optimizations.

The default Linux build includes Wayland and X11. To reproduce either release variant, select its window backend explicitly:

```bash
cargo build --locked --release --bin eidoryn --no-default-features --features wayland,vulkan,jit
cargo build --locked --release --bin eidoryn --no-default-features --features x11,vulkan,jit
```

The Windows test package uses a native MSVC build with Windows SDK `fxc.exe` available through `GPUI_FXC_PATH`:

```powershell
cargo build --locked --release --bin eidoryn --no-default-features --features desktop,dx12,vulkan,opengl
```

Windows packages do not enable the Linux-only experimental JIT.

Enable **Settings → Graphics / F8 → Hardware acceleration**, or pass `--renderer hardware` for automatic selection. The settings panel probes devices and shader initialization in the background; unavailable backends remain disabled with a reason. `--renderer dx12`, `vulkan`, `opengl`, or `metal` requests a specific backend. Windows prefers DirectX 12, Linux prefers Vulkan, and macOS prefers Metal; unavailable automatic candidates are tried in order. Explicit selections fail without silently changing to another API, preserving the active session.

Under **Display options → Internal resolution**, select 1×, 2×, or 4× with hardware acceleration enabled. All three fit the game to the same 4:3 viewport. The desktop remembers your hardware resolution when you switch renderers.

Under **Display options → Rendering color depth**, choose **True color (RGB888)** for smoother shading and blending at any hardware resolution. **Native (RGB555)** is the default. Changes apply to subsequent drawing. The game still reads RGB555 from VRAM and uses it for texture feedback. Software rendering uses native color and keeps your hardware preference for the next switch.

Use `./target/release/eidoryn.exe --list-renderers | Out-Host` in PowerShell to list detected backends and any unavailability reasons.

The `hardware` build feature includes all four wgpu backends; individual `dx12`, `vulkan`, `opengl`, and `metal` features remain available. Backends also require platform and driver support. All share the integer compute rasterizer: drawing, texture lookup, blending and display conversion run on the GPU, then frames are read back for the desktop UI. Performance depends on the game and driver. wgpu 30 has no native DirectX 11 backend; older Windows GPUs can use OpenGL if compute support is available, or software rendering. True color has been tested on Linux with Vulkan and OpenGL. [Pixel tests and frame timings](tests/true-color-validation.md)

On Windows, the native title bar provides window controls. The workspace bar shows the current page, playback status and settings. Release desktop builds start without a console window; debug builds and builds without `desktop` retain the console. For interactive CLI output, use a headless build (`--no-default-features`). To capture output from the desktop release in PowerShell and wait for it to finish, pipe it to `Out-Host`, for example `./target/release/eidoryn.exe --help | Out-Host`.

### Interface language

Choose **Settings → Appearance → Language** to switch immediately and save your choice. On first launch, Eidoryn matches your system locale to an available language and falls back to English. Use `--language zh-CN` to override the choice for one session. `--help` lists the language tags included in your build.

To add a language, create a file named for its language tag, such as `locales/fr.toml`. Set `name` to the native language name and add translations under `[messages]`. An optional top-level `aliases` list adds system locale matches. Rebuild to include the language in settings and the CLI. Omitted keys use English. The build rejects unknown keys and empty translations. See the [English catalog](locales/en.toml) for the full list of keys.

Theme, CRT, volume, mute state, preferred renderer, and fullscreen controls are also saved. CLI options such as `--theme ocean --volume 60 --crt soft` override saved preferences for the current session. **Settings → Graphics → Clean fullscreen** hides the playback controls when the game picture is fullscreen. The equivalent launch options are `--fullscreen --clean-fullscreen`.

### Load a game

1. Select **Choose BIOS** and load your own 512 KiB BIOS file.
2. Select **Open disc image** and load an ISO, CUE, or CHD file. For a multi-track BIN image, select the `.cue` that references all tracks.
3. Select **Run** to start the game.

Choose **Enlarge game view** (**F6**) to put the emulator window in fullscreen. The Game view workspace fills the screen, including the sidebar, playback controls, quick slot, rewind, and memory cards. Choose **Restore window** or press **Esc** to return to the page and window you started from.

Choose **Fullscreen** (**F11**) to use the entire screen for the game picture, centered at 4:3. **F12** shows or hides playback controls over the picture. If you enter fullscreen from the enlarged Game view, **Esc** returns there first.

CHD support covers standalone, single-session CD-ROM images. Images requiring a parent CHD and GD-ROM images are not supported.

Choosing a BIOS on the console sets the firmware for the next disc you open. The current game continues playing while the selected file is checked. Open **Full paths and default BIOS** and select **Set as default** to use it automatically on the next launch. **Clear default BIOS** removes that setting without deleting the BIOS file or changing the current session's selection. Startup and disc opening check that the selected BIOS is still available; a missing default is removed from the configuration and reported so you can choose another file. An unreadable or invalid BIOS is reported without discarding the saved path.

The home page keeps the ten most recently opened disc images, newest first. Select a recent game to reopen it with the current BIOS and its saved fast-boot setting. When the disc loads, the workspace switches to **Game view**. **Clear recent games** removes only the history. Startup selects the default BIOS but does not automatically launch a recent game. Explicit command-line BIOS choices apply to that session without replacing the saved default.

You can also start from the command line:

```bash
cargo run --locked --release -- \
  --bios /path/to/bios.bin --disc /path/to/game.cue --run
```

Memory cards default to `saves/card1.mcd` and `saves/card2.mcd` relative to the current working directory. Use `--card1 PATH` and `--card2 PATH` to choose other files. `--fast-boot` skips BIOS initialization and is intended only for homebrew that does not rely on BIOS services. Run `cargo run --locked --release -- --help` for more options.

### Save states

Select **New snapshot** on the **Save states** page or press **Ctrl+S** to capture the current emulated machine. Each capture creates a new timestamped file in `saves/<game>/` relative to the working directory. Existing captures are retained.

In **Game view**, open **Snapshots** and select one of the current game’s three most recent snapshots to load it. Select **Manage snapshots** at the bottom of the menu, or **Save states** in the sidebar, to see the full history. Each row shows the snapshot name and time and offers load, rename, and delete actions. To permanently remove a snapshot file, select **Delete snapshot**, check its name and time, and confirm. Loading preserves the previous execution state. A running game continues, and a paused game stays paused. This restores CPU, memory, graphics, audio, device timing, disc-drive state, and connected memory-card contents together; it also works with memory cards disabled. Original disc images must remain available when restoring disc sessions. Save states use a versioned format and are not guaranteed to work across incompatible emulator versions.

Use **Quick save** (**Ctrl+Shift+S**) and **Quick load** (**Ctrl+Shift+L**) alongside the snapshot controls for a dedicated slot at `saves/<game>/quick/slot.ps1state`. Each game has one quick-save slot: saving replaces that slot without creating additional files or overwriting timestamped history. Quick loading follows the same rule: running games continue, while paused or stopped games remain paused. **Ctrl+S** still creates a separate timestamped snapshot. The Quick save button asks before overwriting an occupied slot and shows its timestamp. The keyboard shortcut saves immediately.

To rename a snapshot, use the rename button on its history row. The new name must be unique within the current game's save directory; existing files are never overwritten. Renaming keeps the `.ps1state` extension and the captured state unchanged. Newly created snapshots still receive timestamped names.

### Rewind

Open **Settings (F8) → Rewind** to enable recording and set the maximum history duration from **5 to 120 seconds**. Rewind is disabled by default, with a default maximum duration of **30 seconds**. Both settings are remembered. Confirming the switch off clears the rewind history. Cancelling keeps recording.

Once recording is enabled, hold **Ctrl+Z** or the **Rewind** button to move back through recent gameplay, then release to stop. Audio is muted while rewinding. Releasing resumes a previously running game or keeps a previously paused game paused, using the current controller buttons. The display shows how much recorded history remains.

Complete machine snapshots stay in memory, sampled every 250 ms of emulated time, within the configured duration and an estimated **256 MiB** memory budget. Older snapshots are discarded as either limit is reached, so available history may be shorter than the selected maximum. Rewinding restores one snapshot every 100 ms and stops at the oldest recorded point until released. Continuing from a rewound point replaces the abandoned future history. Changing games, resetting, manually loading a snapshot, or closing the application clears this history. Rewind does not create save files or overwrite normal and quick-save snapshots.

<details>
<summary>Headless build and tests</summary>

```bash
cargo run --locked --release --no-default-features -- --headless --no-memory-cards --frames 120
cargo test --locked --workspace
```

With default features disabled, the core and CLI build without desktop, audio-output, or GPU dependencies. `--frames` counts execution slices, not video frames.

GPUI follows the official `main` branch; `Cargo.lock` pins the tested revision.
See [dependency provenance](THIRD_PARTY.md).

</details>

## Controls

| Default keyboard key | PS1 controller |
| --- | --- |
| Arrow keys | D-pad |
| `X` / `Z` / `S` / `D` | Cross / Square / Triangle / Circle |
| `Enter` / `Backspace` | Start / Select |
| `Q` / `E` | L1 / R1 |
| `A` / `F` | L2 / R2 |

Open **Settings (F8) → Controller → Keyboard bindings**, select a controller button, then press its new keyboard key. All 14 digital controller buttons can be remapped. Duplicate keys are allowed while editing and are highlighted, so you can swap bindings directly. Click **Confirm changes** to validate, apply, and save the completed mapping; unresolved duplicates prevent confirmation. The game keeps using the last confirmed mapping until then. **Esc** cancels capture; **Reset keyboard bindings** stages the defaults above for confirmation without changing turbo settings. **Discard changes** or closing settings abandons unconfirmed edits. Tab, Esc, F5–F12, and modifier combinations remain reserved for interface shortcuts. Space can be bound to a controller button.

Open **Settings (F8) → Controller** to enable turbo for individual action or shoulder buttons. Hold the corresponding keyboard key to repeat; release to stop. Choose 5, 10, 15, or 20 presses per second. Turbo is off by default, and button/rate choices are saved. Directions, Start, and Select keep their normal behavior.

| Shortcut | Action |
| --- | --- |
| `F5` | Run / pause |
| `F6` | Enlarge the Game view workspace to fill the screen or restore the window |
| `F7` / `F8` | Cycle CRT filter / open graphics settings |
| `F11` / `Esc` | Toggle fullscreen for the game picture / return to the previous view |
| `F12` | Show or hide fullscreen controls |
| `F9` / `F10` | Run one slice / step one instruction |
| `Ctrl+R` | Reset the current session |
| `Ctrl+S` | Create a save state |
| `Ctrl+Shift+S` | Quick save to the current game's dedicated slot |
| `Ctrl+Shift+L` | Quick load the current game's dedicated slot |
| Hold `Ctrl+Z` | Rewind recent gameplay; release to stop |

Use **Tab / Shift+Tab** to navigate controls and **Enter / Space** to activate them. Arrow keys adjust a focused volume slider; **Esc** closes settings or returns keyboard input to the game. Icon buttons show descriptions on hover.

## Architecture

```text
CLI / GPUI
    │
    ├── systems::ps1 ──→ eidoryn-ps1 ──→ optional JIT runtime
    │     media, cards,      PS1 hardware core
    │     and worker
    └── eidoryn-host
          audio, CRT filters, and themes
```

The emulation worker owns the PS1 machine; the interface communicates through commands and snapshots. `crates/ps1` contains the independent hardware core, `crates/host` provides reusable host services, and `src/systems/ps1` adapts PS1 sessions to the application. See [architecture](docs/ARCHITECTURE.md) for boundaries and data flow.

## Documentation

The architecture document is currently in Chinese.

| Document | Contents |
| --- | --- |
| [Architecture](docs/ARCHITECTURE.md) | Modules, threads, and data flow |
| [Third-party information](THIRD_PARTY.md) | Dependencies and hardware references |

## Roadmap

- Improve PS1 device timing, audio accuracy, and game compatibility; expand regression coverage.
- Continue measuring frame time, audio latency, and Vulkan readback costs before optimizing further.
- Validate and improve Windows and macOS desktop execution and release workflows.
- Explore additional system cores within the existing module boundaries; there is no implementation commitment or timeline for GBA or PS2.

## License & Acknowledgments

The project is licensed under [GPL-3.0-only](LICENSE). [LICENSE-MIT](LICENSE-MIT) preserves the license notice for the original prototype. See [THIRD_PARTY.md](THIRD_PARTY.md) for dependencies and hardware references.

Hardware behavior is informed by [PSX-SPX](https://psx-spx.consoledev.net/); the desktop interface uses [GPUI](https://www.gpui.rs/). Development makes extensive use of AI assistance, and hardware behavior and compatibility remain under active validation.

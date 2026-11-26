# Eidoryn

[![Platforms: PS1, GBA and NDS](https://img.shields.io/badge/Platforms-PS1%20%7C%20GBA%20%7C%20NDS-6750A4?style=flat-square)](#current-status)
[![Rust: 1.95+](https://img.shields.io/badge/Rust-1.95%2B-DEA584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![UI: GPUI](https://img.shields.io/badge/UI-GPUI-7C4DFF?style=flat-square)](https://www.gpui.rs/)
[![Status: Experimental](https://img.shields.io/badge/Status-Experimental-E9A23B?style=flat-square)](#current-status)
[![License: GPL--3.0](https://img.shields.io/badge/License-GPL--3.0--only-4C8B64?style=flat-square)](LICENSE)

Experimental project developed with extensive AI assistance. Hardware accuracy and game compatibility are still under active development.

**English** | [简体中文](README.zh-CN.md)

[Project Overview](#project-overview) · [Features](#features) · [Current Status](#current-status) · [Getting Started](#getting-started) · [Controls](#controls) · [Architecture](#architecture) · [Roadmap](#roadmap) · [License & Acknowledgments](#license--acknowledgments)

## Project Overview

Eidoryn is an experimental Rust emulator with a native GPUI interface. Its PlayStation (PS1), Game Boy Advance (GBA) and Nintendo DS (NDS) cores use the same desktop game library. Supply your own game images. GBA and NDS include high-level emulation (HLE) of BIOS services.

## Features

- **PS1 emulation:** MIPS CPU, GTE, GPU, MDEC, SPU, disc drive, DMA, timers, a shared CPU/device cycle timeline, and a digital controller.
- **GBA emulation** includes an ARM/Thumb interpreter, scanline graphics, keypad input and SRAM, Flash and EEPROM cartridge saves.
- **PS1 graphics and performance options:** serial and multithreaded software rendering, optional wgpu hardware acceleration with runtime backend detection, enlarged and fullscreen views, and a CRT display filter.
- **Desktop experience:** English and Simplified Chinese, keyboard navigation, five built-in themes, JSON theme plugins, saved preferences, a volume slider, and system audio output.
- **Media and saves:** ISO, BIN, multi-track CUE/BIN, and CHD CD images; two memory-card slots with automatic persistence, plus timestamped save states for returning to any saved moment.
- **Debugging tools:** An optional debugger with CPU registers, disassembly, single stepping, and execution slices; an experimental JIT and a headless CLI. Enable the debugger in its workspace or start with `--debugger`.

## Current Status

| Area | Status |
| --- | --- |
| Emulated systems | PlayStation, Game Boy Advance and an experimental Nintendo DS core. |
| Host platforms | Linux x86-64 has been used for development and testing. Windows and macOS headless CLI builds have passed CI smoke tests; their desktop interfaces still need hands-on runtime validation. |
| PS1 rendering | Software rendering is the default. **Settings → Graphics / F8** offers optional hardware acceleration and detects usable backends. Windows automatic selection prefers DirectX 12, then Vulkan, then OpenGL. |
| PS1 CPU JIT | Experimental and disabled by default; supports a subset of integer instructions on Linux x86-64 only. |
| Accuracy and compatibility | Device timing, graphics, and audio are not cycle-accurate. Compatibility and performance vary by game, scene, and host. |

Disc swapping is not supported. Memory-card saves use the game's own save points or menus; save states can be created while running or paused.

## Getting Started

### Requirements

- Rust **1.95 or newer** and Cargo. A release build is recommended for running games.
- The desktop build requires GPUI's system graphics, font, and audio development libraries, plus a working graphics driver. See the [release workflow](https://github.com/Opynicus/Eidoryn/blob/main/.github/workflows/release.yml) for the Linux build dependencies.
- Hardware rendering requires a physical GPU and driver with compute shader support. Windows supports DirectX 12, Vulkan and OpenGL 4.3+; older devices without a compatible hardware backend can use software rendering.
- Supply your own game images. Normal PS1 disc boot also requires a 512 KiB PlayStation BIOS.

This UI redesign branch produces temporary test releases for Linux x86-64 Wayland, Linux x86-64 X11, and Windows x86-64, with SHA-256 checksums. These prereleases are built from the test branch and do not replace the latest release. On Linux, choose the archive matching your desktop session; Ubuntu 24.04 or a compatible newer system is required. On Windows, extract the full ZIP and run `eidoryn.exe` from its directory. Windows desktop display, input, and audio still need hands-on validation; native CI covers compilation, tests, and the headless demo. See [GitHub Releases](https://github.com/Opynicus/Eidoryn/releases) for available test packages.

### Desktop game library

Open **Game library** in the sidebar and choose **Add game** to import images or scan a folder. You can also drop images directly into the library. Pick the CUE file for a multi-track game. Select a game and click **Start game**, double-click its card, or press Enter. The game view opens once the media has loaded. For PS1 discs, the library prompts for a PlayStation BIOS when needed.

Drag a game onto either side of another card, or above or below a list row, to change its position. The library switches to **Manual order** and remembers the arrangement. Right-click a game to scrape its metadata, open its details, or remove it. Check the box in the upper-right corner of a card or at the start of a list row to select games. You can then check more games or select all current search results and confirm their removal. Clearing the last check returns to browsing. Removing entries keeps the disc images, memory cards, snapshots and play history.

New discs appear immediately using their filenames or a release already cached on this device. For PS1, Eidoryn looks up release titles, aliases and dates on Wikidata by PlayStation serial and downloads covers in the background. If Wikidata has no matching edition or regional date, Eidoryn also checks GameDB-PSX by disc serial. GBA uses the cartridge header code and region to find editions in Libretro’s No-Intro database and artwork in its GBA box-art collection. When several GBA editions share a code, the scraper checks the local ROM size and CRC32 against the catalog. If the match is still ambiguous, select the card and click **Choose edition**. Use **Scrape metadata** on an existing game to retry a previous miss. Hover over the circular arrow in the library toolbar to see its action. Click it to refresh the entire library, including games outside the current search. In **Details**, you can choose a matching edition, group discs, relocate an image or edit individual fields and the cover. Refreshes preserve manual edits. New imports and their pending jobs are saved together. After a restart, the library resumes outstanding work. Retry an individual game from Details or retry failed games together under **View task history** on the library overview.

**Get cover** downloads artwork for the selected game. Libretro is the default source. On first use, Eidoryn downloads its PlayStation Redump DAT to match disc serials with regional box art. Under **Scraping settings**, choose separate limits for release lookups and cover downloads (1–20 each, initially 6 and 3). The next refresh or import uses these limits. Providers can impose lower request limits. For PS1, you can also switch the cover source to ScreenScraper, IGDB or a custom HTTPS template. For ScreenScraper and IGDB, set the credential environment variables shown there before starting Eidoryn. To keep imports and browsing entirely offline, turn off **Scrape on import** and **Fill missing covers while browsing**.

Under **Scraping settings → Metadata cache**, **Clear metadata cache** defaults to the entire library. This deletes downloaded covers, release details and cached candidate editions, including details for editions you chose. Each card shows a placeholder and its local image name. The release information in Details stays empty after restarting. Use **Scrape metadata** on a game or refresh the whole library to fetch release details and artwork again, even when automatic cover downloads are off. Eidoryn remembers your edition choice for the next scrape. Choose **Removed games only** to keep the data for games still in your library. **Clear metadata when removing a game** is off by default. Cleanup preserves manual edits, custom covers, disc images, saves and play history.

The user data directory contains `library.sqlite3` with the library index, cached release fields and the source records for each matched edition. The `covers/` folder shares original images and PNG thumbnails by content hash. On Linux it is `$XDG_DATA_HOME/eidoryn` (default `~/.local/share/eidoryn`), on Windows `%LOCALAPPDATA%\eidoryn`, and on macOS `~/Library/Application Support/eidoryn`. Playtime counts time spent running forward, including cutscenes and input-free menus. A normal exit saves the final interval.

Before upgrading an existing library, Eidoryn saves a copy as `library.sqlite3.vN.bak`, where N is the old schema version. To use it with the earlier release, close all Eidoryn instances and move the current database and any `library.sqlite3-wal` and `library.sqlite3-shm` files together to a backup folder. Copy the matching backup into the user data directory as `library.sqlite3` and keep the existing `covers/` folder.

### Play a GBA cartridge

Add a local `.gba` cartridge to **Game library** and click **Start game**. PS1 and GBA share a collection and a recent games list. Opening a game selects its core automatically. You can also select **GBA** in the top bar to open a cartridge directly from the console. The GBA core starts at the cartridge entry point and handles BIOS calls in Rust, so the ROM is all you need on first launch.

The built-in BIOS services include interrupt waits, memory copies, integer math, affine transforms and decompression. If a game calls an unimplemented service, the error identifies the call number and the cartridge instruction address.

In Game view, use the arrow keys for the D-pad, **X / Z** for A / B, **Q / E** for L / R, **Enter** for Start and **Backspace** for Select.

Press **F8** and open **Controller** to edit keyboard bindings for the selected core. Under **Graphics**, enable GBA hardware acceleration to composite layers and apply LCD color effects on the GPU. The debugger shows ARM/Thumb disassembly, registers, the stack and LCD status. **F10** steps one instruction and **F9** advances one frame.

Save from the game's own menu to update its raw `.sav` file. Eidoryn supports SRAM (32 KiB), Flash (64 or 128 KiB) and EEPROM (512 bytes or 8 KiB). Each save lives under `saves/gba/<ROM SHA-256>.sav` in the user data directory, so moving or renaming the ROM keeps its progress. Games such as *Pokémon Emerald* use the cartridge real-time clock. It follows the host UTC clock, including while the emulator is closed. Game-written clock adjustments are stored beside the save in a separate `.rtc` file. The **Cartridge save** panel in Game view can sync, export or import a save. Import restarts the cartridge and clears rewind history.

GBA uses the same **Snapshots** menu, quick slot, save-state manager and rewind controls as PS1. GBA snapshots are stored in `saves/gba/states/<ROM SHA-256>/` under the user data directory. Loading a snapshot or releasing rewind keeps the previous running or paused state. The restored timeline includes the cartridge save memory. Use **Export .sav** to keep a separate copy of in-game progress before loading an older timeline.

The first version renders at 240 × 160 and supports tiled backgrounds, bitmap modes 3–5, sprites, mosaic, display windows, color blending and VBlank interrupts. CPU and DMA timing are approximate. The GBA core mixes its two square waves, wave channel, noise and both timer-driven Direct Sound FIFOs into stereo. The Game view volume and mute controls also apply to cartridges. Link cable support is pending. Compatibility testing covers original homebrew cartridges, including an ARM/Thumb keypad and color test and an interrupt-driven frame counter. Run `cargo test --locked -p eidoryn-gba` to reproduce them.

Run `cargo test --release --locked -p eidoryn-gba --test local_boot -- --ignored --nocapture` to start each local cartridge with HLE and run 1,800 frames without input. The tests read ROMs from the repository’s `tests/` folder by default. Set `EIDORYN_GBA_TEST_ROMS` to select a specific ROM or directory and `EIDORYN_GBA_BOOT_FRAMES` to change the duration.

For regional editions of *Castlevania: Aria of Sorrow*, run `cargo test --release --locked -p eidoryn-gba --test aria_of_sorrow -- --ignored --nocapture` with the same folder, or use `EIDORYN_ARIA_ROM` to select a single image. The verified Japanese and US retail images have reference checks for their title screens, save slots and the Castle Corridor. Other editions are checked for visible output during boot and compared against an idle run of the same image to detect keypad response. Each run uses a fresh in-memory save.

For English *Pokémon Emerald* and US *Castlevania: Harmony of Dissonance*, point `EIDORYN_GBA_TEST_ROMS` at the folder containing both cartridges and run `cargo test --release --locked -p eidoryn-gba --test emerald --test harmony_of_dissonance -- --ignored --nocapture`. Emerald starts a new game, saves in Littleroot Town and continues after a cold boot. Harmony checks the title, playable character and castle drawbridge against mGBA.

### Play a Nintendo DS cartridge

Add a local `.nds` file to **Game library** and select **Start game**, or choose **NDS** in the top bar and open it from the console. The core loads the cartridge's ARM9 and ARM7 programs directly. A first launch needs only the cartridge.

Game view stacks two 256 × 192 LCDs. Click or drag on the lower screen to use the stylus. Keyboard bindings include X and Y and can be changed in **Settings → Controller**. The debugger shows both program counters. **F10** advances one dual-CPU scheduling step and **F9** advances one frame. The quick slot is stored per cartridge under `saves/nds/states/<ROM SHA-256>/`.

The DS core renders tiles, bitmaps, sprites and textured 3D polygons in software with approximate CPU and device timing. Two-byte-address EEPROM cartridges save automatically to `saves/nds/<ROM SHA-256>.sav` in the user data directory. The emulated wireless controller passes local startup checks and scans with no peers. The ARM7 sound unit mixes 16 channels of PCM, IMA-ADPCM, square waves or noise with per-channel volume and panning. Game view uses the same volume and mute controls as the other cores. Multiplayer and other cartridge backup types are outside its current coverage. Run `cargo test --locked -p eidoryn-nds` for core tests. To test another homebrew that draws during startup, set `EIDORYN_NDS_TEST_ROM` and run `cargo test --release --locked -p eidoryn-nds --test local_boot -- --ignored --nocapture`.

The US *Castlevania: Order of Ecclesia* regression starts the cartridge with erased save memory, operates the language selector, starts a new game, and verifies that pressing Right moves Shanoa in the first room. Put your copy in the repository’s `tests/` directory or set `EIDORYN_ECCLESIA_ROM`, then run `cargo test --release --locked -p eidoryn-nds --test order_of_ecclesia -- --ignored`.

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

Under **Display options → Anti-aliasing**, choose **FXAA** to smooth edges at the selected hardware resolution. It defaults to **Off**. In-game text and thin lines may look softer. Changes appear on the next frame, including after a debugger step. PS1 RGB24 movies keep their original scanout.

Use `./target/release/eidoryn.exe --list-renderers | Out-Host` in PowerShell to list detected backends and any unavailability reasons.

The `hardware` build feature includes all four wgpu backends; individual `dx12`, `vulkan`, `opengl`, and `metal` features remain available. Backends also require platform and driver support. All share the integer compute rasterizer: drawing, texture lookup, blending and display conversion run on the GPU, then frames are read back for the desktop UI. Performance depends on the game and driver. wgpu 30 has no native DirectX 11 backend; older Windows GPUs can use OpenGL if compute support is available, or software rendering. True color has been tested on Linux with Vulkan and OpenGL.

On Windows, the native title bar provides window controls. The workspace bar shows the current page, playback status and settings. Release desktop builds start without a console window; debug builds and builds without `desktop` retain the console. For interactive CLI output, use a headless build (`--no-default-features`). To capture output from the desktop release in PowerShell and wait for it to finish, pipe it to `Out-Host`, for example `./target/release/eidoryn.exe --help | Out-Host`.

### Interface language

Choose **Settings → Appearance → Language** to switch immediately and save your choice. On first launch, Eidoryn matches your system locale to an available language and falls back to English. Use `--language zh-CN` to override the choice for one session. `--help` lists the language tags included in your build.

To add a language, create a file named for its language tag, such as `locales/fr.toml`. Set `name` to the native language name and add translations under `[messages]`. An optional top-level `aliases` list adds system locale matches. Rebuild to include the language in settings and the CLI. Omitted keys use English. The build rejects unknown keys and empty translations. See the [English catalog](locales/en.toml) for the full list of keys.

Select **Settings → Appearance → Interface theme → Lemon Green** for a charcoal interface with lime accents. Eidoryn saves the selected theme, CRT, volume, mute state, preferred renderer, and fullscreen controls. CLI options such as `--theme lemon-green --volume 60 --crt soft` override saved preferences for the current session. **Settings → Graphics → Clean fullscreen** hides the playback controls when the game picture is fullscreen. The equivalent launch options are `--fullscreen --clean-fullscreen`.

### Load a PS1 game

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

Select **New snapshot** on the **Save states** page or press **Ctrl+S** to capture the current emulated machine. Each PS1 capture creates a new timestamped file in `saves/<game>/` relative to the working directory. Existing captures are retained.

In **Game view**, open **Snapshots** and select one of the current game’s three most recent snapshots to load it. Select **Manage snapshots** at the bottom of the menu, or **Save states** in the sidebar, to see the full history. Each row shows the snapshot name and time and offers load, rename, and delete actions. To permanently remove a snapshot file, select **Delete snapshot**, check its name and time, and confirm. Loading preserves the previous execution state. A running game continues, and a paused game stays paused. This restores CPU, memory, graphics, audio, device timing, disc-drive state, and connected memory-card contents together; it also works with memory cards disabled. Original disc images must remain available when restoring disc sessions. Save states use a versioned format and are not guaranteed to work across incompatible emulator versions.

Use **Quick save** (**Ctrl+Shift+S**) and **Quick load** (**Ctrl+Shift+L**) alongside the snapshot controls for a dedicated slot at `saves/<game>/quick/slot.ps1state`. Each game has one quick-save slot: saving replaces that slot without creating additional files or overwriting timestamped history. Quick loading follows the same rule: running games continue, while paused or stopped games remain paused. **Ctrl+S** still creates a separate timestamped snapshot. Both the Quick save button and the keyboard shortcut save immediately.

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

Run `cargo test --locked --all-features --lib library::` to check indexing, metadata, artwork and recovery. To exercise regional scraping against the live providers, run `cargo test --locked --all-features --lib live_regional_scrape_matrix -- --ignored`.

On a physical Vulkan GPU, run `cargo test --release --locked --no-default-features --features vulkan --test vulkan_gpu -- --ignored --test-threads=1` for the PS1 rendering tests, including FXAA and true color. The frame-time benchmarks replay a local PS1 save state with the same input for both settings. [FXAA benchmark](examples/fxaa_bench.rs) · [True-color benchmark](examples/true_color_bench.rs)

GPUI follows the official `main` branch, and `Cargo.lock` pins the tested revision. [Dependency sources and hardware references](docs/ARCHITECTURE.md#依赖与参考资料)

</details>

## Controls

| Default keyboard key | PS1 controller |
| --- | --- |
| Arrow keys | D-pad |
| `X` / `Z` / `S` / `D` | Cross / Square / Triangle / Circle |
| `Enter` / `Backspace` | Start / Select |
| `Q` / `E` | L1 / R1 |
| `A` / `F` | L2 / R2 |

Open **Settings (F8) → Controller → Keyboard bindings**, select a controller button, then press its new keyboard key. All 14 digital controller buttons can be remapped. Duplicate keys are allowed while editing and are highlighted, so you can swap bindings directly. Click **Confirm changes** to validate, apply, and save the completed mapping; unresolved duplicates prevent confirmation. The game keeps using the last confirmed mapping until then. **Esc** cancels capture; **Reset keyboard bindings** stages the defaults above for confirmation without changing turbo settings. **Discard changes** or closing settings abandons unconfirmed edits. Controller buttons accept single keys, including Space and function keys. Reassign or clear a conflicting application shortcut before using its key for the controller. Tab and Esc are reserved for navigation.

Open **Settings (F8) → Controller** to enable turbo for individual action or shoulder buttons. Hold the corresponding keyboard key to repeat; release to stop. Choose 5, 10, 15, or 20 presses per second. Turbo is off by default, and button/rate choices are saved. Directions, Start, and Select keep their normal behavior.

Open **Settings → Shortcuts**, select an action, then press the key or key combination you want to use. You can clear an individual binding or restore the defaults for all actions. Resolve any conflicts with other shortcuts or controller keys, then select **Confirm changes**. Buttons and the status bar show the saved bindings.

| Default shortcut | Action |
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
    ├── systems::gba ──→ eidoryn-gba ──→ eidoryn-arm
    │     cartridge, SRAM,   GBA hardware core
    │     and worker
    ├── systems::nds ──→ eidoryn-nds ──→ eidoryn-arm
    └── eidoryn-host
          audio, CRT filters, and themes
```

Each emulation worker owns its machine and exchanges commands and snapshots with the interface. The hardware cores live in `crates/ps1`, `crates/gba` and `crates/nds`. GBA and NDS share the ARM interpreter in `crates/arm`. Desktop adapters in `src/systems` connect them to the application, and `crates/host` supplies reusable host services. The architecture guide describes the module boundaries and data flow. [Architecture (Chinese)](docs/ARCHITECTURE.md)

## Roadmap

- Improve PS1 device timing, audio accuracy, and game compatibility; expand regression coverage.
- Continue measuring frame time, audio latency, and Vulkan readback costs before optimizing further.
- Validate and improve Windows and macOS desktop execution and release workflows.
- Extend GBA compatibility with additional cartridge save types and more accurate hardware timing.

## License & Acknowledgments

The project is licensed under [GPL-3.0-only](LICENSE). [LICENSE-MIT](LICENSE-MIT) preserves the license notice for the original prototype. Third-party license notices are included with each release. [Third-party notices](LICENSE-THIRD-PARTY)

Hardware behavior is informed by [PSX-SPX](https://psx-spx.consoledev.net/); the desktop interface uses [GPUI](https://www.gpui.rs/). Development makes extensive use of AI assistance, and hardware behavior and compatibility remain under active validation.

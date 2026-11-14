# Eidoryn

[![Platform: PlayStation](https://img.shields.io/badge/Platform-PlayStation-6750A4?style=flat-square)](#当前状态)
[![Rust: 1.95+](https://img.shields.io/badge/Rust-1.95%2B-DEA584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![UI: GPUI](https://img.shields.io/badge/UI-GPUI-7C4DFF?style=flat-square)](https://www.gpui.rs/)
[![Status: Experimental](https://img.shields.io/badge/Status-Experimental-E9A23B?style=flat-square)](#当前状态)
[![License: GPL--3.0](https://img.shields.io/badge/License-GPL--3.0--only-4C8B64?style=flat-square)](LICENSE)

这是一个广泛使用 AI 辅助开发的实验性项目。硬件精度与游戏兼容性仍在持续开发中。

[English](README.md) | **简体中文**

[项目概览](#项目概览) · [功能](#功能) · [当前状态](#当前状态) · [快速开始](#快速开始) · [按键](#按键) · [架构](#架构) · [文档](#文档) · [路线图](#路线图) · [许可证与致谢](#许可证与致谢)

## 项目概览

Eidoryn 是一个使用 Rust 编写的实验性模拟器，配备原生 GPUI 界面。目前实现了 PlayStation（PS1）核心，提供游戏运行、图形设置和硬件调试功能。项目自主实现了 CPU、GPU、音频、光驱等模拟组件，没有集成其他模拟器核心。

架构为未来增加其他主机预留了空间；**目前不支持 GBA 或 PS2**。项目不包含游戏镜像、BIOS 或存档。

## 功能

- **PS1 模拟**：MIPS CPU、GTE、GPU、MDEC、SPU、光驱、DMA、计时器、统一 CPU/设备周期时间线和数字手柄。
- **画面与性能选项**：串行软件绘图、多线程软件绘图和可选 wgpu 硬件加速，运行时检测可用后端；支持放大、全屏与 CRT 显示滤镜。
- **桌面体验**：中英文即时切换、键盘导航、四套内置主题、JSON 主题插件、设置持久化、音量滑条与系统音频输出。
- **游戏媒体与存档**：读取 ISO、BIN 和多轨 CUE/BIN；支持双槽记忆卡及自动保存。
- **调试工具**：默认关闭的可选调试器，包含 CPU 寄存器、反汇编、单步和执行切片；可在调试页开启，或使用 `--debugger` 启动。另有实验性 JIT 与无窗口命令行模式。

## 当前状态

| 项目 | 状态 |
| --- | --- |
| 模拟主机 | PlayStation（PS1）进行中；其他主机尚未实现 |
| 宿主平台 | Linux x86-64 已用于开发和测试；Windows、macOS 无窗口 CLI 已通过 CI 冒烟测试，桌面版仍需实际运行验证 |
| 渲染 | 默认软件绘图；在 **设置 → 图形设置 / F8** 中启用硬件加速并检测可用后端。Windows 自动选择优先 DirectX 12，其次 Vulkan、OpenGL |
| CPU JIT | 实验性功能，默认关闭，仅支持 Linux x86-64 的部分整数指令 |
| 精度与兼容性 | 设备时序、图形和音频并非周期精确；游戏兼容性和性能因场景与设备而异 |

目前不支持即时存档、CHD 或运行中换盘。记忆卡存档需要在游戏自身的存档点或菜单中完成。

## 快速开始

### 环境要求

- Rust **1.95 或更新版本**及 Cargo；建议使用 release 构建运行游戏。
- 桌面版需要 GPUI 所需的系统图形、字体与音频开发库，以及可用图形驱动。Linux 构建依赖见[发布工作流](https://github.com/Opynicus/Eidoryn/blob/main/.github/workflows/release.yml)。
- 硬件绘图需要支持计算着色器的物理显卡和驱动。Windows 支持 DirectX 12、Vulkan 和 OpenGL 4.3+；没有兼容硬件后端的老旧设备仍可使用软件绘图。
- 启动游戏需要自行准备相应 BIOS 和镜像。项目不提供这些文件。

此 UI 改版分支提供 Linux x86-64 Wayland、Linux x86-64 X11 和 Windows x86-64 临时测试发行包，均附 SHA-256 校验文件。这些预发布版本从测试分支构建，不替换最新发行版。Linux 请按桌面会话选择对应压缩包，需要 Ubuntu 24.04 或兼容的更新系统；Windows 请完整解压 ZIP，再从解压目录运行 `eidoryn.exe`。Windows 桌面显示、输入及声音仍需人工实机验证，原生 CI 覆盖编译、测试和无窗口演示。可用测试包见 [GitHub Releases](https://github.com/Opynicus/Eidoryn/releases)。

### 构建与启动

```bash
git clone https://github.com/Opynicus/Eidoryn.git
cd Eidoryn
cargo run --locked --release
```

未指定媒体时会打开内置原创演示，无需 BIOS。构建后可运行 Linux/macOS 的 `target/release/eidoryn` 或 Windows 的 `target/release/eidoryn.exe`。`--locked` 固定依赖版本，`--release` 才会启用优化。

Linux 默认构建同时包含 Wayland 和 X11。复现发布版本时，可明确选择窗口后端：

```bash
cargo build --locked --release --bin eidoryn --no-default-features --features wayland,vulkan,jit
cargo build --locked --release --bin eidoryn --no-default-features --features x11,vulkan,jit
```

Windows 测试包采用原生 MSVC 构建，通过 `GPUI_FXC_PATH` 指定 Windows SDK 的 `fxc.exe`：

```powershell
cargo build --locked --release --bin eidoryn --no-default-features --features desktop,dx12,vulkan,opengl
```

Windows 包不启用仅支持 Linux 的实验性 JIT。

在 **设置 → 图形设置 / F8** 中启用 **硬件加速**，或通过 `--renderer hardware` 自动选择。设置页在后台检测设备并验证着色器初始化，不可用的后端会禁用并说明原因。`--renderer dx12`、`vulkan`、`opengl`、`metal` 可明确指定后端。Windows 优先 DirectX 12，Linux 优先 Vulkan，macOS 优先 Metal；自动模式依次尝试可用后端，明确指定时则报告失败并保留原会话，不静默切换图形接口。

在 PowerShell 中运行 `./target/release/eidoryn.exe --list-renderers | Out-Host` 可查看检测到的后端及不可用原因。

`hardware` 构建特性包含四种 wgpu 后端，也可单独启用 `dx12`、`vulkan`、`opengl`、`metal`；运行时仍受平台和驱动能力限制。它们共用整数计算光栅器，绘图、纹理采样、混合及显示转换在 GPU 上执行，再读回画面供桌面显示。性能取决于游戏和驱动。wgpu 30 不提供原生 DirectX 11 后端；老旧 Windows 显卡可尝试支持计算着色器的 OpenGL，或使用软件绘图。Metal 及非 Windows 平台的运行行为仍需在对应设备上验证。

Windows 桌面使用系统原生标题栏，并隐藏额外的工作区顶部栏。桌面 release 构建启动时不再弹出控制台；调试构建和未启用 `desktop` 的构建仍保留控制台。需要交互式命令行输出时，请使用无窗口构建（`--no-default-features`）。在 PowerShell 中读取桌面发行版的输出并等待其结束，可通过管道连接 `Out-Host`，例如 `./target/release/eidoryn.exe --help | Out-Host`。

### 界面语言

在 **设置 → 外观主题 → 语言** 中选择 **English** 或 **简体中文**，界面立即切换，并保存供下次启动使用。首次启动时，中文系统语言选择简体中文，其余语言默认使用英文。

可通过 `--language en` 或 `--language zh-CN` 临时指定本次启动语言。

主题、CRT、音量、静音状态、首选渲染器和全屏控制栏也会保存。启动参数优先于已保存设置，例如 `--theme ocean --volume 60 --crt soft`。在 **设置 → 图形设置 → 纯画面全屏** 中隐藏全屏控制栏，也可用 `--fullscreen --clean-fullscreen` 启动。

### 加载游戏

1. 在界面中选择 **选择 BIOS**，加载自己的 512 KiB BIOS。
2. 选择 **打开光盘镜像**，加载 ISO 或 CUE；多轨 BIN 镜像请选择关联所有轨道的 `.cue` 文件。
3. 点击 **运行** 开始运行。使用 **F11** 进入或退出全屏。

也可以从命令行启动：

```bash
cargo run --locked --release -- \
  --bios /path/to/bios.bin --disc /path/to/game.cue --run
```

记忆卡默认位于当前工作目录的 `saves/card1.mcd` 和 `saves/card2.mcd`；可用 `--card1 PATH`、`--card2 PATH` 指定其他文件。`--fast-boot` 会跳过 BIOS 初始化，仅适用于不依赖 BIOS 服务的自制程序。更多选项可运行 `cargo run --locked --release -- --help` 查看。

<details>
<summary>无窗口构建与测试</summary>

```bash
cargo run --locked --release --no-default-features -- --headless --no-memory-cards --frames 120
cargo test --locked --workspace
```

无默认特性的核心与命令行仅使用 Rust 标准库。`--frames` 表示执行切片数，而不是视频帧数。

GPUI 跟随官方 `main` 分支，由 `Cargo.lock` 锁定已验证的提交，详见[依赖来源说明](THIRD_PARTY.md)。

</details>

## 按键

| 默认键盘按键 | PS1 手柄 |
| --- | --- |
| 方向键 | 方向键 |
| `X` / `Z` / `S` / `D` | Cross / Square / Triangle / Circle |
| `Enter` / `Backspace` | Start / Select |
| `Q` / `E` | L1 / R1 |
| `A` / `F` | L2 / R2 |

在 **设置（F8）→ 手柄 → 自定义按键** 中，点击手柄按键后按下新的键盘键，即可修改全部 14 个数字手柄按键。编辑时允许暂时重复绑定并显示提示，因此可以直接交换两个键位。点击 **确认修改** 时统一校验，通过后才生效并保存；若仍有重复绑定，则保留编辑内容供继续调整。确认前，游戏继续使用上次确认的键位。**Esc** 取消本次按键录入；**恢复默认按键** 将编辑内容改为上表映射，仍需确认，连发设置保持不变。**取消修改** 或关闭设置会放弃未确认的修改。Tab、Esc、F5–F12 和修饰键组合保留给界面快捷操作，空格可绑定为手柄按键。

在 **设置（F8）→ 手柄** 中选中需要连发的动作键或肩键，再长按对应键盘键即可连发，松开即停止。可选 5、10、15、20 次/秒，默认全部关闭；按键与速度会自动保存。方向键、Start 和 Select 保持普通输入。

| 快捷键 | 操作 |
| --- | --- |
| `F5` | 运行 / 暂停 |
| `F6` / `F7` / `F8` | 切换游戏视图 / CRT 滤镜 / 图形设置 |
| `F11` / `Esc` | 切换全屏 / 退出全屏 |
| `F12` | 显示或隐藏全屏控制栏 |
| `F9` / `F10` | 执行一个切片 / 单条指令 |
| `Ctrl+R` | 重置当前会话 |

使用 **Tab / Shift+Tab** 移动控件焦点，**Enter / Space** 激活；音量滑条聚焦后可用方向键调整。**Esc** 收起设置或将键盘输入交还游戏，鼠标悬停图标按钮可查看说明。

## 架构

```text
CLI / GPUI
    │
    ├── systems::ps1 ──→ eidoryn-ps1 ──→ 可选 JIT runtime
    │     媒体、存档、运行线程       PS1 硬件核心
    │
    └── eidoryn-host
          音频、CRT 滤镜与主题
```

PS1 机器状态由运行线程持有；界面通过命令和快照与其交互。`crates/ps1` 是独立硬件核心，`crates/host` 提供可复用宿主功能，`src/systems/ps1` 负责 PS1 会话适配。设计边界与数据流见[架构文档](docs/ARCHITECTURE.md)。

## 文档

| 文档 | 内容 |
| --- | --- |
| [架构](docs/ARCHITECTURE.md) | 模块边界、线程与数据流 |
| [第三方资料](THIRD_PARTY.md) | 依赖与硬件参考 |

## 路线图

- 提升 PS1 设备时序、音频精度和游戏兼容性，扩大回归测试范围。
- 继续测量帧时间、音频延迟和 Vulkan 读回开销，按证据优化性能。
- 验证并完善 Windows、macOS 的构建、运行及发布流程。
- 在已有模块边界的基础上探索其他主机核心；GBA 和 PS2 尚无实现承诺或时间表。

## 许可证与致谢

项目采用 [GPL-3.0-only](LICENSE)；[LICENSE-MIT](LICENSE-MIT) 保留原始自研原型的许可声明。第三方依赖及其说明见 [THIRD_PARTY.md](THIRD_PARTY.md)。

硬件行为参考 [PSX-SPX](https://psx-spx.consoledev.net/)；桌面界面使用 [GPUI](https://www.gpui.rs/)。本项目在开发过程中广泛使用 AI 辅助，硬件实现和兼容性仍在持续验证。

# Eidoryn

用 Rust 自主实现的模拟器应用，搭配原生 GPUI 调试界面。当前支持范围仅为 PlayStation；代码按独立核心与公共宿主服务分层，为未来 GBA、PS2 等核心留出结构空间，尚未实现这些主机。CPU、软件/Vulkan GPU、总线、DMA、CD-ROM 控制器、ISO 读取器和测试程序都在本项目实现；没有集成外部模拟器核心。使用 `--no-default-features` 可构建仅依赖 Rust 标准库的软件核心与命令行。

**Pure vibe-coded project. Use at your own risk.**

这是一个纯 vibe coding 实验项目，仍在开发中，兼容性与硬件精度有限。支持双槽记忆卡、音频、可选 Vulkan 绘图、多线程软件绘图和实验性 JIT。仓库只分发自主实现代码与原创测试程序，不包含游戏、BIOS、存档或实际游戏测试素材。

<img src="assets/eidoryn.png" alt="Eidoryn icon" width="128" />

## 启动

日常运行请使用 `--release`。`--locked` 只锁定依赖版本，不启用优化；默认 debug 构建用于开发，会明显拖慢解释器和软件绘图。

在本目录执行，未指定媒体时运行原创 MIPS 绘图演示，不需要 BIOS：

```bash
cargo run --locked --release
```

程序名为 `eidoryn`，库名为 `eidoryn_psx`。完成构建后也可直接执行 `./target/release/eidoryn`，参数与 `cargo run -- ...` 相同。

**Graphics / F8** 提供 Vulkan 硬件加速、软件绘图多线程和实验性 CPU JIT 开关，可在运行中切换并保留游戏状态。Vulkan 使用独立提交线程，启用后软件多线程选项变灰；关闭硬件加速会恢复之前的软件模式。命令行也可选择 `--renderer vulkan`、`--renderer threaded` 或 `--renderer serial`（默认）。JIT 另用 `--cpu jit` 启用，默认 `--cpu interpreter`，目前支持 Linux x86-64 的普通整数运算块。设置与测试范围见 [渲染后端说明](docs/RENDERING.md)和 [JIT 说明](docs/JIT.md)。

界面采用 Material 3 风格的浅色 tonal surfaces、圆角卡片、紫色主按钮、状态标签和错误提示；保留视频、CPU 寄存器与指令调试。全部由 GPUI 原生绘制。

顶部 **Theme** 可即时切换 Lavender、Midnight、Ocean、Forest 四套内置主题；**Load theme…** 可加载自定义 JSON 主题插件。启动时自动扫描当前目录下的 `plugins/themes/*.json`，随项目附带 Ember 示例。主题覆盖按钮、卡片、文字、状态和全屏工具栏，不修改游戏本身的颜色。详见[主题插件格式](docs/THEMES.md)。

```bash
cargo run --locked --release -- --theme midnight
cargo run --locked --release -- --theme-plugin plugins/themes/ember.json
```

| 操作 | 行为 |
| --- | --- |
| Space / Run / Pause | 运行或暂停 |
| F10 / Step | 一条 CPU 指令 |
| F9 / Slice | 最多 20,000 次 CPU step |
| F6 / Enlarge / Inspector | 隐藏调试面板，放大画面；再次切换返回调试视图 |
| F7 / CRT | 在 Off、Soft、Classic 显示滤镜之间切换 |
| F8 / Graphics | 打开硬件加速、软件多线程和 JIT 开关 |
| F11 / Fullscreen | 进入或退出原生全屏；全屏时使用放大视图 |
| Esc | 退出全屏，恢复之前的窗口视图 |
| Maximize / Restore | 通过窗口管理器最大化或恢复窗口 |
| Ctrl+R / Reset | 恢复加载时的 CPU、RAM、设备和光盘状态，暂停 |
| Mute / Unmute；音量滑条 | 静音或恢复声音；点击或拖动滑条连续调整 0–100% 音量 |
| Flush saves | 将记忆卡中待保存的数据立即写入磁盘 |
| Demo | 运行内置原创程序 |
| Open EXE | 装入 PS-X EXE，暂停 |
| Open BIOS | 载入并记住有效的 512 KiB 固件，暂停 |
| Open Disc | 使用已选 BIOS 挂载 CUE/BIN 或 ISO，暂停供调试，点击 Run 开始 |
| Fast boot | 从光盘的 SYSTEM.CNF 查找 EXE，直接装入 RAM 并运行 |

窗口可拖动边框改变尺寸，视频按电视的 4:3 比例缩放并留边，不随 256/320/512/640 像素模式切换而拉伸变形。`--game-view` 直接启动放大视图，`--fullscreen` 直接启动全屏，`--run` 自动开始执行。全屏下保留运行和退出入口，错误仍会显示。

`--crt soft` 或 `--crt classic` 可启用扫描线、轻微单色栅格、边缘暗角和圆角；默认关闭。CRT 使用缓存遮罩由 GPUI 合成，只在尺寸、DPI、扫描行数或预设变化时重建，不逐帧在 CPU 上过滤图像。小窗口下自动减弱扫描线以降低摩尔纹。这是轻量显示效果，尚未模拟荧光余辉、RGB 子像素或信号解码；帧导出仍保留原始像素。

数字手柄连接在端口 1：方向键移动，`X/Z/S/D` 对应 Cross/Square/Triangle/Circle，`Enter/Backspace` 对应 Start/Select，`Q/E` 对应 L1/R1，`A/F` 对应 L2/R2。窗口失焦、暂停、重置或加载媒体时释放按键，避免卡键。端口 2 手柄未连接，两个端口均支持记忆卡。

Fast boot 跳过 BIOS 初始化，仅用于不依赖 BIOS 服务的自制程序。镜像以文件句柄按扇区读取，不将整个光盘装入内存。支持 BINARY CUE 的多文件/共用 BIN、Mode 1/2 数据轨、音轨边界和 INDEX 00/01，以及 XA 解码和 CDDA 播放。多轨镜像应选择 `.cue`，由它关联各个 `.bin`，不能将音轨 BIN 当作独立数据盘。暂不支持 CHD 和换盘。

`Image exceeds 524288 bytes` 是旧版将光盘送入 BIOS 加载器的提示。**BIOS 和 ISO 是不同文件：游戏镜像用 Open Disc / `--disc`，固件用 Open BIOS / `--bios`。** 新版明确报告误选入口；失败的选择不会替换当前机器和上次成功选择的 BIOS。

## 声音、存档与运行速度

桌面默认启用系统音频，初始音量 75%。`--mute` 可静音启动；找不到输出设备时显示错误，仍可运行游戏，**Enable sound** 可重试。暂停、重置和换媒体会清空播放缓冲。SPU 生成 44.1 kHz 双声道 PCM，由 CPAL 接入系统设备；音频队列与视频快照独立。实现范围和 WAV 导出见[音频说明](docs/AUDIO.md)。

默认记忆卡为当前工作目录下的 `saves/card1.mcd` 和 `saves/card2.mcd`，首次使用创建已格式化的 128 KiB 原始卡。游戏写入后每 250 ms 批量落盘，暂停、重置和关闭时同步；重置保留卡内容。请在游戏自身的存档点/菜单中保存；这不是即时存档。可用 `--card1 PATH --card2 PATH` 指定卡，`--no-memory-cards` 断开两个卡槽。格式、锁和失败重试见[存档说明](docs/SAVES.md)。

界面 FPS 统计实际模拟视频帧。工作线程完成一帧后按视频时钟等待，NTSC 目标约 59.82 FPS，PAL 约 49.75 FPS；不会把 PAL 强制加速到 60。不同场景和系统负载会影响速度，不能保证所有游戏稳定满速。性能测量的范围与局限见[性能审查](docs/PERFORMANCE.md)。

运行画面在 VBlank 开始时保存为完整快照，避免读取游戏清屏、换缓冲区期间的中间状态而闪黑；正常黑屏转场和淡入淡出仍会显示。调试 Step/Slice 仍可查看当前 VRAM。

## 自制光盘端到端测试

不需要固件、显卡或任何商业素材：

```bash
mkdir -p test-assets
cargo run --locked --no-default-features -- --export-disc-test test-assets/probe.exe
python3 scripts/make_test_disc.py test-assets/probe.exe test-assets/probe.iso
cargo run --locked --no-default-features -- \
  --disc test-assets/probe.iso --fast-boot --headless --frames 20 \
  --dump-frame test-assets/probe.ppm

# 在 GPUI 中观察同一程序
cargo run --locked -- --disc test-assets/probe.iso --fast-boot
```

导出命令不覆盖已有文件，重复运行请换输出名称。测试客体是原创 MIPS 机器码：设置光驱位置与模式，发出 ReadN，确认中断，通过 DMA3 将卷描述符搬到 RAM，检查签名，再提交 GPU 绘图命令。主机只负责初始 EXE 装载，后面的扇区传输由模拟的 CPU、CD-ROM 和 DMA 执行。

预期输出包含 `instructions=400000`、`GP0 commands=26`、`CD sectors=1`。图像内容是水晶与网格，测试同时比较完整 2048 字节扇区，不只是检查非空画面。详见 [验证记录](docs/TESTING.md)。

## 其他命令

```bash
# 无窗口原创绘图演示
cargo run --locked --no-default-features -- --headless --frames 120

# 导出并执行独立 PS-X EXE
cargo run --locked --no-default-features -- --export-demo demo.exe
cargo run --locked --no-default-features -- --exe demo.exe --headless --frames 1

# 调试真实 BIOS 的执行路径
cargo run --locked -- --bios /path/to/bios.bin --disc /path/to/game.iso

# 从 BIOS 自动启动并全屏显示
cargo run --locked --release -- \
  --bios /path/to/bios.bin \
  --disc /path/to/game.cue \
  --renderer vulkan --fullscreen --run

# 检查所有轨道、SYSTEM.CNF 和启动 EXE，不执行客体
cargo run --locked --no-default-features -- --inspect-disc /path/to/game.cue
```

`--frames` 为保留的 CLI 参数名，实际计数是有界宿主时间片，不是 VBlank 或硬件视频帧。PPM 为所选 GPU 后端的原始 RGB 输出。BIOS 启动不使用 Fast boot，不会偷偷替换成主机加载游戏。

## 当前硬件实现

| 模块 | 能力与边界 |
| --- | --- |
| CPU | MIPS I 基础整数运算、HI/LO 乘除、分支/加载延迟槽、字节/半字/字访存、LWL/LWR/SWL/SWR |
| CPU 执行 | 默认解释器；可选原创 Linux x86-64 整数 JIT，逐指令时钟/中断/代码修改校验，其余指令回到解释器 |
| COP0 | 基础 MFC0/MTC0/RFE、溢出/未对齐/SYSCALL/BREAK 异常、EPC/BD、外部中断入口 |
| GTE / COP2 | 22 条标准计算命令、矩阵/透视/光照/颜色运算、44-bit 累加溢出、饱和标志、UNR 除法、FIFO、MFC2/CFC2 延迟和 LWC2/SWC2 |
| 总线 | 2 MiB RAM 及镜像、KSEG 映射、1 KiB Scratchpad、512 KiB BIOS、部分控制寄存器 |
| GPU | 1024×512 16-bit VRAM；纯色/渐变/纹理三角形与四边形、线段/连续线段、矩形、填充和图像传输；4/8-bit CLUT、15-bit 纹理 |
| 绘图执行 | 串行/多线程软件与 Vulkan 整数计算后端；有界命令批次、读取与 VBlank 同步，保持绘图顺序 |
| 绘制控制 | 纹理窗口、颜色调制、透明 texel、四种混合模式、抖动、绘制区域、偏移、mask bit、显存回绕 |
| 视频与计时器 | NTSC/PAL 整数扫描线、VBlank IRQ、GPUSTAT 奇偶位、三个 root counter 的时钟源、同步与中断；非周期精确 |
| MDEC | 流式 RLE、量化/缩放表、整数逆 DCT、YUV 转换、4/8/15/24-bit 输出、DMA 背压与 8×8 到 16×16 重排；GPU 支持 RGB24 显示 |
| SIO0 | 数字手柄五字节应答、波特率传输、ACK 时序、接收 FIFO、IRQ7 边沿锁存和键盘输入 |
| 记忆卡 | 两槽 128 KiB、读/写/ID 串口协议、校验和/ACK、自动落盘、文件锁和失败重试 |
| SPU | 24 声道 ADPCM、Gaussian 音高插值、ADSR/音量扫变、噪声、音高调制、混响、CD 混音、捕获缓冲、512 KiB RAM、FIFO/DMA4/IRQ9 |
| DMA | 通道 0/1 MDEC 流式输入/输出、通道 2 GPU 块/链表、通道 3 光驱输入、通道 4 SPU 双向传输、通道 6 OTC；DICR 边沿中断、字节/半字访问 |
| 光盘 | 2048-byte ISO、2352-byte Mode 1 / Mode 2 Form 1 BIN、双轨及多轨 CUE；有界 ISO-9660 遍历和 SYSTEM.CNF 启动文件查找 |
| CD-ROM | 参数/响应/数据 FIFO、寄存器分页、INT1/2/3/4/5、Setloc、ReadN/ReadS、Play、Pause、Init、Seek、XA/CDDA、音量矩阵和查询命令 |
| 载入 | PS-X EXE 范围校验、GP/SP/FP、零填充、BIOS 长度校验；失败保留原会话 |
| 桌面 | Material 风格 GPUI、独立执行线程、文件选择器、运行控制、放大/全屏、可切换 CRT 和可见错误 |

CD-ROM 的命令延迟和寻道是近似值，扇区节拍按 75/150 Hz 的标称设备时钟计算，但目前每次 CPU step 仅推进一个设备时钟，并非周期精确模拟。控制器有单扇区暂存，未模拟真实溢出行为；DMA3 只支持一次可用扇区内的手动输入，不支持 chopping 或跨扇区突发传输。支持 Mute/Demute、XA 音频解码/过滤及交错视频数据传输。

镜像不包含实体光盘的 SCEx 摆动信息。GetID 根据 Mode 2 第 4 扇区的地区授权文本推断 SCEA/SCEE/SCEI，未知镜像仍报告未认证；这是模拟器的区域识别策略，不是真实光盘认证恢复，也不修改 BIOS。

ISO 可生成 Mode 2 Form 1 子头和 EDC，但无法恢复原光盘的 XA 元数据、音轨或认证信息，也不生成 ECC。原始 BIN 不验证 EDC/ECC。隔行半行、GPU busy/亚像素、GTE 指令延时、MDEC 舍入/延时仍未精确模拟，线段使用有界整数 DDA，线段与三角形的亚像素规则仍为近似，尚未与实机逐像素比对。SPU 仅支持标准 RAM 传输配置，混响重采样仍为近似；尚无即时存档。启动通过不等于完整游戏兼容性通过。

后台以约 4 ms 执行块检查输入，按实际视频帧调度，移除了旧版每 16.7 ms 只执行 12 ms 的限制。取指/RAM 字访问、GPU 时序和空闲设备路径已优化；时序优化与保留的逐时钟参考实现作随机对照。手动 Slice 仍是最多 20,000 次 step。建议用 release 构建运行 BIOS/游戏。

## 开发与验证

Rust 2024 edition，最低版本声明为 1.95；本次使用 Linux / Rust 1.98.1 / Wayland。桌面需要 GPUI 的 Linux 图形与字体开发库和可用图形驱动；无窗口核心不需要这些运行设施。保留 `Cargo.lock`：GPUI 0.2.2 的旧 `xattr` 依赖 `libc::ENOATTR`，当前固定 `libc = 0.2.186`。

```text
crates/ps1/src/                 独立 PS1 硬件核心、错误与原创客体程序
crates/host/src/audio/          系统音频与 WAV 导出，可显式指定源采样率
crates/host/src/theme/          通用语义配色与主题插件
crates/host/src/video_filter.rs 通用 CRT 预设与遮罩
src/systems/ps1/                PS1 加载、卡文件、会话调度和调试快照
src/desktop/                    GPUI 窗口、组件、显示与 PS1 调试面板
src/{main,app}.rs               CLI 与产品标识
src/lib.rs                      应用入口与历史库 API 兼容层
examples/boot_trace.rs           有界启动诊断、帧导出与定时手柄输入
examples/runner_probe.rs         执行线程吞吐量与暂停延迟测量
examples/display_probe.rs        帧缓冲转换微基准
examples/gpu_bench.rs            软件/多线程/Vulkan 绘图与读回对照
examples/cpu_bench.rs            原创整数循环的解释器/JIT 对照
crates/jit-runtime/              原生代码生成与隔离的可执行内存边界
examples/play_probe.rs           交互手柄脚本、声音录制和带音频帧率测量
examples/timing_probe.rs         核心设备时序微基准
examples/serial_trace.rs         对照执行并记录客体 SIO 读写
scripts/make_test_disc.py        原创测试光盘生成器
```

模块边界和数据流见[代码结构](docs/ARCHITECTURE.md)。GPUI 只读取快照并发送命令；工作线程独占机器状态。主题功能属于桌面特性，不给无窗口核心增加依赖。

```bash
cargo fmt --all --check
cargo test --locked --no-default-features
cargo test --locked --workspace --all-targets
cargo test --locked -p eidoryn-ps1 --no-default-features
cargo test --locked -p eidoryn-host --all-features
cargo test --locked --test vulkan_gpu -- --ignored --test-threads=1
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo build --locked
cargo tree --locked --no-default-features
```

测试方法与覆盖范围见[验证说明](docs/TESTING.md)，本次审查的改动与测量见[性能审查](docs/PERFORMANCE.md)。GPUI 的传递依赖 `proc-macro-error2` 有 future-incompatibility 提示。[LICENSE](LICENSE) 保持现有项目许可，[LICENSE-MIT](LICENSE-MIT) 保留原始自研原型的许可。第三方依赖及硬件参考资料见 [THIRD_PARTY.md](THIRD_PARTY.md)。

## 实现资料

实现参考硬件行为资料，没有引入其他模拟器的实现源码：

- [PSX-SPX：CD-ROM 寄存器与命令](https://psx-spx.consoledev.net/cdromdrive/)
- [PSX-SPX：光盘格式](https://psx-spx.consoledev.net/cdromformat/)
- [PSX-SPX：DMA](https://psx-spx.consoledev.net/dmachannels/)
- [PSX-SPX：CUE 格式](https://psx-spx.consoledev.net/cdromfileformats/)
- [PSX-SPX：GPU 绘制与视频时序](https://psx-spx.consoledev.net/graphicsprocessingunitgpu/)
- [PSX-SPX：SPU](https://psx-spx.consoledev.net/soundprocessingunitspu/)
- [PSX-SPX：计时器](https://psx-spx.consoledev.net/timers/)
- [PSX-SPX：GTE](https://psx-spx.consoledev.net/geometrytransformationenginegte/)
- [PSX-SPX：MDEC](https://psx-spx.consoledev.net/macroblockdecodermdec/)
- [PSX-SPX：SIO](https://psx-spx.consoledev.net/serialinterfacessio/)
- [PSX-SPX：手柄协议](https://psx-spx.consoledev.net/controllersandmemorycards/)
- [GPUI 0.2.2 API](https://docs.rs/gpui/0.2.2/gpui/)

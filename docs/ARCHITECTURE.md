# Eidoryn 代码结构

Eidoryn 是应用层，当前只实现 PlayStation。GBA、PS2 等是未来扩展方向，不是已经支持的平台。本次重构将可独立构建的硬件核心、可复用的宿主服务和具体主机的应用适配明确分开；不为尚不存在的核心定义统一 CPU、存档或渲染器协议。

```text
Eidoryn (CLI / GPUI)
├── systems::ps1                 PS1 媒体、记忆卡文件、运行线程和调试快照
│   ├── eidoryn-ps1              CPU / Bus / GPU / SPU / CD-ROM / SIO 等硬件
│   │   └── eidoryn-jit-runtime  可选原生执行边界
│   └── eidoryn-host::audio      有界 PCM 传输
└── eidoryn-host                 音频输出 / WAV / CRT / 主题插件
```

`eidoryn-ps1` 不依赖宿主模块、GPUI、CPAL 或主题；默认仅依赖标准库。`eidoryn-host` 不依赖任何主机核心或 GPUI，默认也仅依赖标准库。Vulkan/JIT 是核心的可选特性，`audio-output`/`themes` 是宿主模块的可选特性。应用的 `desktop`、`vulkan`、`jit` 特性转发到对应组件；`wayland` 和 `x11` 启用桌面及对应 GPUI 窗口后端。默认仍同时包含这两个 Linux 后端，发布构建使用 `--no-default-features --features wayland,vulkan,jit` 或 `x11,vulkan,jit` 分别构建；Windows/macOS 继续使用原生窗口后端。

| 位置 | 职责 |
| --- | --- |
| `crates/ps1/src/{cpu,bus,machine}.rs` | MIPS CPU、总线、机器状态与执行；DMA 位于 `bus/dma.rs` |
| `crates/ps1/src/cpu_engine.rs`、`cpu/jit.rs` | 解释器/JIT 选择、缓存及逐指令异常/中断/代码失效检查 |
| `crates/ps1/src/gpu/` | 自主软件光栅器、多线程绘图、Vulkan 管线与 WGSL；GP0/GP1 和显示时序位于 `gpu.rs` |
| `crates/ps1/src/{gte,mdec,timers,sio,memory_card}.rs` | PS1 几何运算、解码、时钟、手柄和记忆卡协议 |
| `crates/ps1/src/{disc,cue,cdrom}.rs` | 镜像读取与光驱控制器 |
| `crates/ps1/src/spu/`、`{adpcm,xa,cd_audio}.rs` | PS1 合成、混响与光盘音频解码 |
| `crates/ps1/src/demo.rs`、`error.rs` | 原创客体程序与 PS1 错误类型 |
| `crates/jit-runtime/` | 原创 x86-64 发射器及隔离的 RW→RX/原生调用安全边界 |
| `crates/host/src/audio.rs`、`audio/` | 与主机无关的 PCM 队列、显式源采样率、CPAL 输出和 WAV 导出 |
| `crates/host/src/video_filter.rs`、`theme/` | CRT 遮罩、语义配色与数据型主题插件，无 GPUI 类型 |
| `src/systems/ps1/input.rs` | 按键保持状态、独立连发相位与配置校验；以模拟 CPU 周期计时 |
| `src/systems/ps1/media.rs` | BIOS/EXE 读取、校验与 PS1 会话构建 |
| `src/systems/ps1/card_file.rs` | PS1 卡文件锁、批量同步和原子替换 |
| `src/systems/ps1/runner/` | PS1 命令、会话线程、持久化边界、快照与调试状态 |
| `src/desktop/` | GPUI 生命周期、窗口、Material 组件、音量、主题、CRT 合成和操作；PS1 调试面板明确放在 `ps1_inspector.rs` |
| `src/main.rs`、`src/app.rs` | 进程入口、退出码与共享产品标识 |
| `src/cli/args.rs` | 手写参数解析、默认配置和兼容性校验；不访问文件系统 |
| `src/cli/mod.rs`、`headless.rs` | 启动分派、独立检查/导出，以及无窗口执行与输出 |
| `src/lib.rs` | 应用模块入口和历史 `eidoryn_psx` API 兼容重导出 |

## API 兼容与后续核心接入

旧入口 `eidoryn_psx::machine::Machine`、`runner`、`media`、`audio` 等继续可用，类型仍是原类型，没有包装复制或动态分发。新应用代码明确使用 `eidoryn_psx::systems::ps1`；只需要硬件核心的工具可直接依赖 `eidoryn-ps1`。`Gpu::latch_display` 是给调试 Step/Slice 使用的显式快照接口，不推进视频时间；运行中仍使用正常 VBlank 锁存画面。

未来增加一个核心时：

1. 在独立 crate 中实现自身机器、时序、媒体与错误，不让核心反向依赖 GPUI 或应用。
2. 在 `src/systems/<system>` 中实现加载、持久化、输入映射和调试适配；PS1 的双记忆卡、光驱、MIPS 寄存器与渲染设置不作为共同要求。
3. 复用 `eidoryn-host` 的音频/主题/显示滤镜。用 `AudioBuffer::with_sample_rate(NonZeroU32)` 和 `WavWriter::with_sample_rate` 指定 PCM 源采样率。现有 `default()` / `create()` 仍保持 44.1 kHz；WAV 不可表示的字节率在创建文件前拒绝。
4. 有第二个真实核心后，再根据两个实现提炼运行、输入和视频接口。当前 GPUI 的画幅、按键映射与 Inspector 仍是 PS1 适配，需要在接入新核心时扩展；本次没有假装它们已经通用。

热路径保留静态调用，不引入逐指令虚调用或新的队列。依赖版本未升级，主核心和宿主 crate 共同继承工作区的 unsafe 禁止和 Clippy 规则；JIT crate 保留自己的安全边界约束。应用目录与窗口 application ID 保留原值，避免影响现有启动器和窗口管理规则。

## PS1 执行与同步

工作线程独占 `Machine` 和 `CardStore`，窗口通过通道发送命令，从邮箱取走最新完整快照。关闭请求先同步记忆卡，成功后发布最终快照再退出；失败则暂停并允许重试。`Runner::drop` 回收线程并尝试最终同步。媒体加载先校验，成功后再替换机器和重置基准；换媒体和重置均保留当前记忆卡。失败仍保留原会话。

手柄输入通过 `Command::SetButtons` 更新实际保持状态，`Command::SetTurbo` 配置动作/肩键连发。运行线程在每个有界执行片段前后采样连发状态并送入 SIO；按键各自以首次按下的 CPU 周期为起点，按下/松开各占半个周期。暂停和会话替换释放输入但保留连发配置。输入配置不清空音频、不重置帧节奏，也不改动硬件核心的手柄协议。

命令行与库中新建 `Gpu` 默认串行；`--renderer threaded`、`--renderer vulkan` 或 `Gpu::set_renderer` 选择其他后端。Graphics/F8 面板通过 `Command::SetRenderer` 在运行中切换，保留机器和记忆卡。多线程软件模式将原有光栅器和 VRAM 移入一个持久工作线程。解释器继续负责 GP0 解包、寄存器和设备时钟；绘图命令携带提交时的绘图状态，按原顺序成批发送。每批最多 128 条，通道最多排队 2 批；DMA2 结束时也提交不足一批的命令，使绘图能与后续 CPU 指令重叠。批次缓冲回收复用，生产速度过快时产生背压。绘图之间不并发，因此纹理反馈、透明混合、遮罩和重叠复制保持原有顺序。

显存读取、机器克隆和后端切换会先提交剩余命令并等待工作完成。VBlank 在向 CPU 发出中断前排入画面采集命令，携带该时刻的显示寄存器；GPU 按顺序在先前绘图之后、后续清屏之前生成不可变像素。解释器可继续执行，真正读取完成帧的调用方才等待采集结果，并通过 `OnceLock` 缓存。同一有界队列也限制在途采集；未消费的旧帧可丢弃，不会阻塞 GPU 回复。克隆得到独立的串行数据快照，避免共享可变显存；会话重置和换媒体会恢复已选后端，线程创建失败时保留原会话。调试接口 `vram()` 返回同步副本，`vram_mut()` 收回工作线程后提供串行编辑。退出时排空已接收任务并 join；设备或线程初始化失败会保留旧后端及显存。设备运行错误锁存后在宿主时间片边界停止执行，克隆也携带错误，不能把失败读回当成健康状态。主模拟器继续禁止 unsafe Rust；可选 JIT 的原生调用边界隔离在本地 `jit-runtime` crate，并有独立的安全约束与测试。


Vulkan 模式通过 wgpu 的安全接口把显存保存在 GPU storage buffer，每个 PS1 半字对应 u32，避免相邻半字写入竞争。GP0 图元编码为固定 32-word 记录，按 8×8 像素分块，每个 GPU invocation 独占一个像素，依次执行该块的命令并保留遮罩/混合顺序。根据 UV 范围、窗口、翻转和 CLUT 追踪读取区域；读写区域发生依赖时结束批次，自身反馈/重叠复制使用有序 GPU shader。上传合并为行段。批次最多 2048 图元、262144 个块引用、65536 上传半字；在途提交到 64 次时等待，防止无 VBlank 程序无限排队。原生分辨率和整数运算与软件算法一致，不要求 shader int64。

Vulkan 的 RGB15/RGB24 显示转换也在 GPU 执行，然后读回 RGB 供 GPUI 显示，尚未共享显示纹理。下载显存时缓存完整读回到下一次绘图；模式切换/克隆显式同步。Vulkan 编译特性和后端选择独立，默认构建提供 Vulkan，但默认运行软件模式；无物理显卡时报告错误，不选择 CPU Vulkan 适配器。后端实现位于 `crates/ps1/src/gpu/`；验证见 `tests/vulkan_gpu.rs`、`tests/threaded_gpu.rs` 和 `tests/display.rs`。

CPU 与设备共用 33,868,800 Hz 的累计周期时间线；指令按当前近似耗时推进设备，JIT 与解释器遵守相同的边界。周期定义与计数 API 位于 `crates/ps1/src/timing.rs` 和 `crates/ps1/src/bus.rs`，时序回归测试见 `tests/cpu_timing.rs`。

`Machine::run_steps` 和 `run_slice` 按所选 CPU 模式执行；`step` 保留解释器调试语义。JIT 仅编译非陷阱整数操作，逐指令回调负责设备时钟、异常边界和代码修改校验。缓存有上限，原生代码只读共享，选择失败不改变原机器。原生执行安全边界见 [JIT runtime 安全约束](../crates/jit-runtime/SAFETY.md)。

执行块以约 4 ms 宿主时间为限，便于响应命令；一帧完成后才等待相应 NTSC/PAL 截止时间。FPS 由 GPU 完成的视频帧数测量，有限吸收调度抖动，避免旧的固定工作预算降低模拟速度。取指/RAM 字访问走连续区域快路径，越界和镜像交界仍走完整访存；计时器和 GPU 时序优化有逐时钟对照测试。

显示输出与调试快照的采样时间分离。GPU 以可见区域结束、发出 VBlank 中断之前作为画面采集边界，将对应像素和尺寸一起保存为 `DisplayFrame`；`Snapshot` 通过 `Arc<[u8]>` 共享这份不可变画面。之后游戏在 VBlank 中交换缓冲区、清屏或改变显示寄存器，不会让宿主批次意外提交中间画面。暂停/状态更新也继续使用完成帧；明确的 Step/Slice 调试操作和内置演示的客体帧结束标记则可刷新当前 VRAM。`display_rgb()` 与命令行帧导出保留即时 VRAM 读取能力。这是按场锁存的近似扫描输出，尚非逐像素扫描线渲染；未改变中断周期，也没有按亮度丢弃黑帧。显示寄存器和 VBlank 范围参考 [PSX-SPX GPU 文档](https://psx-spx.consoledev.net/graphicsprocessingunitgpu/#gp107h-vertical-display-range-on-screen)。

PCM 按设备时钟在核心生成，工作线程独立排入 `AudioBuffer`，不附带在可被替换的快照中。CPAL 回调使用非阻塞锁和预分配有界队列，进行宿主采样率适配；失败输出静音并计数。窗口持有输出流，暂停/换媒体由工作线程清空声音。无窗口 CLI 使用相同核心，通过 `WavWriter` 导出，不链接 CPAL。

快照中的像素和尺寸未变化时，窗口复用已有纹理，运行中的调试/状态信息最多每 100 ms 触发一次重绘。新的视频帧、暂停和关闭立即刷新，避免静态全屏画面仍被状态计数强制每帧重绘。软件三角形边缘权重按整数增量推进；恒定颜色和原色纹理跳过无效插值，保留逐像素输出等价性。视频转换按行处理，RGB24 直接解包连续 VRAM 半字，RGB15 展开三个颜色通道；没有改动游戏指令、设备时序或绘图命令。相关像素等价性测试位于 `tests/display.rs`，转换基准位于 `examples/display_probe.rs`。

新增 UI 元素使用 `Palette` 语义字段；新增主题无需改硬件核心。`theme` 和 JSON 依赖随 `desktop` 特性启用，关闭所有默认特性后的软件核心仍只依赖标准库。原 `runner::load_path/load_disc` 入口继续重导出，新的调用方使用 `systems::ps1::media`。

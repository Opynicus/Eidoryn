# Eidoryn 代码结构

硬件核心、宿主运行控制和桌面界面分层。CPU、GPU、GTE、光驱等继续由本项目自主实现。

```text
CLI / GPUI actions  →  media：校验并构造 Machine
GPUI actions       →  Runner 命令通道
Runner worker      →  Machine → CPU / Bus / 硬件设备
GPU command parser →  有界绘图批次 → 软件/Vulkan 工作线程（独占 VRAM）
Runner worker      →  Snapshot → GPUI 视频、运行状态和调试面板
SPU / CD decoder   →  AudioBuffer → CPAL 设备回调
SIO / MemoryCard   →  CardStore → 原始记忆卡文件
主题 JSON          →  ThemeCatalog → GPUI 语义配色
```

| 模块 | 职责 |
| --- | --- |
| `app.rs` | 产品名、版本和原生窗口标识的统一入口 |
| `main.rs` | CLI 参数校验、启动分派、无窗口执行和帧导出 |
| `media.rs` | 有界文件读取、BIOS/EXE 校验和光盘会话构建 |
| `cpu_engine.rs` | 解释器/JIT 选择、热块缓存、MIPS 整数指令解码 |
| `cpu/jit.rs` | 原生执行边界的 PC、设备时钟、中断与指令失效检查 |
| `crates/jit-runtime/` | 原创 x86-64 发射器、RW→RX 映射和隔离的原生调用边界 |
| `runner/mod.rs` | 公共命令接口、线程创建/关闭和快照邮箱 |
| `runner/worker.rs` | 显式会话状态、命令转换、执行预算和快照发布 |
| `runner/snapshot.rs` | 与前端无关的显示/调试数据及帧比较 |
| `gpu.rs` / `DisplayFrame` | GP0/GP1、传输控制、显示时序及 VBlank 开始时锁存的完成画面 |
| `gpu/rasterizer.rs` | 原创软件光栅器、纹理、混合与 VRAM 像素操作；串行/多线程共用，也作为硬件后端的对照 |
| `gpu/renderer.rs` | 串行/独立线程调度、有界批次、读回屏障、错误锁存和线程回收 |
| `gpu/renderer_engine.rs` | 工作线程中的软件/Vulkan 执行接口 |
| `gpu/vulkan/` | 原创 GP0 图元编码、依赖分批、Vulkan 管线、整数 WGSL 光栅器与读回 |
| `memory_card.rs` | 128-byte 扇区、卡格式、串口事务/校验和/ACK；无文件访问 |
| `card_file.rs` | 卡文件生命周期、旁路锁、批量同步和原子替换 |
| `spu/` | 声音寄存器/RAM、声道、Gaussian 表、包络和混响 |
| `adpcm.rs`、`xa.rs`、`cd_audio.rs` | 共用预测器、XA 解码/重采样、CD 音量矩阵与 PCM 队列 |
| `audio.rs`、`audio/output.rs` | 有界 PCM 队列、系统采样率适配与 CPAL 输出 |
| `audio/wav.rs` | 无声卡依赖的流式诊断录音 |
| `desktop/mod.rs` | GPUI 生命周期、焦点和快照接收 |
| `desktop/actions.rs` | 键盘、文件选择和控制命令 |
| `desktop/components.rs` | 使用语义配色的 Material 通用组件 |
| `desktop/graphics.rs` | 运行时 Vulkan/软件多线程/JIT 开关、待切换状态和设备显示 |
| `desktop/audio_controls.rs` | 音频 UI 状态、可拖动的音量滑条及百分比 |
| `desktop/view.rs` | 顶部栏、工作区、显示卡片与游戏视图的组合 |
| `desktop/inspector.rs` | CPU、指令和光驱面板 |
| `desktop/video.rs` | RGB 到 BGRA、4:3 画布、CRT 遮罩缓存和 GPUI 合成 |
| `video_filter.rs` | 无依赖的 CRT 预设和有界扫描线/栅格/暗角遮罩 |
| `desktop/appearance.rs` | 主题选择和插件加载交互 |
| `theme/` | 内置调色板、主题目录、版本化插件校验 |

工作线程独占 `Machine` 和 `CardStore`，窗口通过通道发送命令，从邮箱取走最新完整快照。关闭请求先同步记忆卡，成功后发布最终快照再退出；失败则暂停并允许重试。`Runner::drop` 回收线程并尝试最终同步。媒体加载先校验，成功后再替换机器和重置基准；换媒体和重置均保留当前记忆卡。失败仍保留原会话。

命令行与库中新建 `Gpu` 默认串行；`--renderer threaded`、`--renderer vulkan` 或 `Gpu::set_renderer` 选择其他后端。Graphics/F8 面板通过 `Command::SetRenderer` 在运行中切换，保留机器和记忆卡。多线程软件模式将原有光栅器和 VRAM 移入一个持久工作线程。解释器继续负责 GP0 解包、寄存器和设备时钟；绘图命令携带提交时的绘图状态，按原顺序成批发送。每批最多 128 条，通道最多排队 2 批；DMA2 结束时也提交不足一批的命令，使绘图能与后续 CPU 指令重叠。批次缓冲回收复用，生产速度过快时产生背压。绘图之间不并发，因此纹理反馈、透明混合、遮罩和重叠复制保持原有顺序。

显存读取、机器克隆和后端切换会先提交剩余命令并等待工作完成。VBlank 在向 CPU 发出中断前排入画面采集命令，携带该时刻的显示寄存器；GPU 按顺序在先前绘图之后、后续清屏之前生成不可变像素。解释器可继续执行，真正读取完成帧的调用方才等待采集结果，并通过 `OnceLock` 缓存。同一有界队列也限制在途采集；未消费的旧帧可丢弃，不会阻塞 GPU 回复。克隆得到独立的串行数据快照，避免共享可变显存；会话重置和换媒体会恢复已选后端，线程创建失败时保留原会话。调试接口 `vram()` 返回同步副本，`vram_mut()` 收回工作线程后提供串行编辑。退出时排空已接收任务并 join；设备或线程初始化失败会保留旧后端及显存。设备运行错误锁存后在宿主时间片边界停止执行，克隆也携带错误，不能把失败读回当成健康状态。主模拟器继续禁止 unsafe Rust；可选 JIT 的原生调用边界隔离在本地 `jit-runtime` crate，并有独立的安全约束与测试。


Vulkan 模式通过 wgpu 的安全接口把显存保存在 GPU storage buffer，每个 PS1 半字对应 u32，避免相邻半字写入竞争。GP0 图元编码为固定 32-word 记录，按 8×8 像素分块，每个 GPU invocation 独占一个像素，依次执行该块的命令并保留遮罩/混合顺序。根据 UV 范围、窗口、翻转和 CLUT 追踪读取区域；读写区域发生依赖时结束批次，自身反馈/重叠复制使用有序 GPU shader。上传合并为行段。批次最多 2048 图元、262144 个块引用、65536 上传半字；在途提交到 64 次时等待，防止无 VBlank 程序无限排队。原生分辨率和整数运算与软件算法一致，不要求 shader int64。

Vulkan 的 RGB15/RGB24 显示转换也在 GPU 执行，然后读回 RGB 供 GPUI 显示，尚未共享显示纹理。下载显存时缓存完整读回到下一次绘图；模式切换/克隆显式同步。Vulkan 编译特性和后端选择独立，默认构建提供 Vulkan，但默认运行软件模式；无物理显卡时报告错误，不选择 CPU Vulkan 适配器。范围、开关和验证方法见 [RENDERING.md](RENDERING.md)。

`Machine::run_steps` 和 `run_slice` 按所选 CPU 模式执行；`step` 保留解释器调试语义。JIT 仅编译非陷阱整数操作，逐指令回调负责设备时钟、异常边界和代码修改校验。缓存有上限，原生代码只读共享，选择失败不改变原机器。具体范围和安全边界见 [JIT.md](JIT.md)。

执行块以约 4 ms 宿主时间为限，便于响应命令；一帧完成后才等待相应 NTSC/PAL 截止时间。FPS 由 GPU 完成的视频帧数测量，有限吸收调度抖动，避免旧的固定工作预算降低模拟速度。取指/RAM 字访问走连续区域快路径，越界和镜像交界仍走完整访存；计时器和 GPU 时序优化有逐时钟对照测试。

显示输出与调试快照的采样时间分离。GPU 以可见区域结束、发出 VBlank 中断之前作为画面采集边界，将对应像素和尺寸一起保存为 `DisplayFrame`；`Snapshot` 通过 `Arc<[u8]>` 共享这份不可变画面。之后游戏在 VBlank 中交换缓冲区、清屏或改变显示寄存器，不会让宿主批次意外提交中间画面。暂停/状态更新也继续使用完成帧；明确的 Step/Slice 调试操作和内置演示的客体帧结束标记则可刷新当前 VRAM。`display_rgb()` 与命令行帧导出保留即时 VRAM 读取能力。这是按场锁存的近似扫描输出，尚非逐像素扫描线渲染；未改变中断周期，也没有按亮度丢弃黑帧。显示寄存器和 VBlank 范围参考 [PSX-SPX GPU 文档](https://psx-spx.consoledev.net/graphicsprocessingunitgpu/#gp107h-vertical-display-range-on-screen)。

PCM 按设备时钟在核心生成，工作线程独立排入 `AudioBuffer`，不附带在可被替换的快照中。CPAL 回调使用非阻塞锁和预分配有界队列，进行宿主采样率适配；失败输出静音并计数。窗口持有输出流，暂停/换媒体由工作线程清空声音。无窗口 CLI 使用相同核心，通过 `WavWriter` 导出，不链接 CPAL。

快照中的像素和尺寸未变化时，窗口复用已有纹理，运行中的调试/状态信息最多每 100 ms 触发一次重绘。新的视频帧、暂停和关闭立即刷新，避免静态全屏画面仍被状态计数强制每帧重绘。软件三角形边缘权重按整数增量推进；恒定颜色和原色纹理跳过无效插值，保留逐像素输出等价性。视频转换按行处理，RGB24 直接解包连续 VRAM 半字，RGB15 展开三个颜色通道；没有改动游戏指令、设备时序或绘图命令。相关像素等价性测试位于 `tests/display.rs`，转换基准位于 `examples/display_probe.rs`。

新增 UI 元素使用 `Palette` 语义字段；新增主题无需改硬件核心。`theme` 和 JSON 依赖随 `desktop` 特性启用，关闭所有默认特性后的软件核心仍只依赖标准库。原 `runner::load_path/load_disc` 入口继续重导出，新的调用方使用 `media`。

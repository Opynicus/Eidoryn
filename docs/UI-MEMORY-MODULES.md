# 各模块内存占用与按需调试

本轮在 `perf/ui-memory` 上完成。原 `perf/ui-memory-baseline-analysis` 的基线分析已通过普通提交移入此分支，没有创建 merge commit；主分支仍由用户审查后通过 Pull Request 合并。

**主要收益来自图标资源，而不是关闭 CPU 调试信息。** 最终 Release 的英文 Console 存活堆从约 **23.05 MiB 降到 16.03 MiB**；默认分配器 RSS 从约 **88.37 MiB 降到 81.30 MiB**。Debugger 默认关闭，可在页面中启用和关闭，离开调试页会暂停反汇编采集。

## 条件与口径

2026-11-12，AMD Ryzen 9 8945H、Linux 7.3.1、Niri 26.04、Rust 1.98.1、GPUI 0.2.2，显示缩放 150%。全部使用内置原创演示、串行软件渲染、解释器、Midnight、CRT 关闭、静音、无记忆卡；窗口为可见且聚焦的 1280×900 逻辑像素。没有加载游戏、BIOS 或用户存档。

- 优化前为 `7b17106`，包含此前字体复用和侧栏缓存优化。UI 重构前版本的比较另见 [历史基线](UI-MEMORY-COMPARISON.md)。
- **RSS** 使用默认分配器独立运行，在约 5 秒、10 秒采样，下面取第二个采样点。Console 每种语言、每版各 3 次；最终 Debugger 页 2 次。
- **存活堆** 使用 gperftools，跳过启动转储，再取两份约相隔 5 秒的快照。英文 Console 每版 3 次，最终 Debugger 页 2 次。这个口径不含字体文件映射，也不等于 RSS。
- 一次中间版本的调试页采样失去焦点，已排除。保留上一帧 RGB 的中间实现也未计入最终结果；该引用已用更新标记替代。
- 数据及二进制 SHA-256 见 [逐次记录](benchmarks/ui-memory-modules-2026-11-12.json)。原始堆转储、系统路径和截图保留在本地，不提交。

本次分类脚本新增图像缓冲、仿真核心、宿主音频三类，并使用同一规则重新分析前后版本。像素分配优先按其所有者归类，避免把移到 `Render` 中的帧转换误算成布局增长。旧文档的“其他”分类不能直接与这里比较。优化内联、截断的调用栈、绘制阶段和共享资源都会影响细分结果。

## 实测结果

默认分配器的进程 RSS，单位 MiB：

| 视图 | 优化前中位数（范围） | 最终版中位数（范围） |
| --- | ---: | ---: |
| Console，英文，n=3 | 88.37（88.35–88.62） | 81.30（79.45–81.59） |
| Console，中文，n=3 | 93.30（93.22–93.38） | 86.49（86.05–86.49） |
| Debugger 已启用，英文，n=2 | 未单独采集 | 81.21（79.96–82.46） |

最终调试页与 Console 的 RSS 范围重叠，不能据此声称调试器没有成本，或关闭调试器会固定节省若干 MiB。本机采样中透明大页和分配器驻留行为带来约 2 MiB 波动；详情中的 `AnonHugePages` 已保留。中文界面还需要 CJK 字体的驻留页，不是调试器导致的增长。

英文进程在采样时存活的堆，单位 KiB，各分项独立取中位数：

| 归因 | 优化前 Console，n=3 | 最终 Console，n=3 | 最终 Debugger，n=2 |
| --- | ---: | ---: | ---: |
| 仿真核心与快照 | 8,449.53 | 8,449.14 | 8,674.97 |
| 字体与文本 | 857.26 | 857.20 | 1,602.83 |
| 布局与元素 | 2,089.96 | 2,089.71 | 2,287.51 |
| 图像缓冲 | 6,442.83 | 556.09 | 256.00 |
| 宿主音频及可识别后端分配 | 1,802.13 | 1,802.13 | 1,802.13 |
| 共享图集/场景缓冲的 CPU 堆 | 1,054.68 | 1,056.13 | 1,465.67 |
| SVG 解析/栅格化 | 0.59 | 0.59 | 0.59 |
| 其他 | 2,809.68 | 1,598.46 | 1,597.09 |
| 每次总量的中位数（MiB） | **23.047** | **16.025** | **17.272** |

分项中位数之和不一定等于总量中位数。调试页拥有不同的文字、布局和场景缓冲；与 Console 的整页差额约 1.25 MiB，不能全算作反汇编的成本。核心归因还包含在途 RGB 快照，采样相位和 UI 消费速度可带来约一帧（225 KiB）的差异，PS1 RAM 容量并未改变。

Console 中的图像累计分配率约 17.39 → 17.36 MiB/s，仍然需要逐帧转换可见画面；调试页为 **0 MiB/s**，不再转换隐藏的视频。SVG 预热后的新增分配为 0。调试页有更多动态文本：字体/文本约 7.31 MiB/s，布局约 159.38 MiB/s；Console 分别约 1.53、113.56 MiB/s。这些是已释放临时对象也计入的累计分配率，不是泄漏或游戏 FPS 数据。

另用前后各一个原生 Console 进程读取 DRM 客户端统计：`resident-gtt + resident-vram` 约 **198.94 → 183.10 MiB**，每个进程相隔 5 秒的两次读数一致。这只是本机图形驱动账户的补充观察，可能含共享资源，不能加到 RSS，也不能全部视作 PS1 Vulkan 渲染器独占显存；本次 PS1 后端使用软件渲染，GPUI 窗口仍使用 GPU。

## 核心设备各自需要多少内存

以下为源码中的确定容量和 x86_64 默认功能构建的 `size_of`，与上表的堆归因互相解释，**不要重复相加**。Worker 同时保存运行中的 `Machine` 和用于重置的初始副本。

| 模块 | 固定容量或本次大小 | 能否通过关闭调试器释放 |
| --- | --- | --- |
| PS1 主 RAM | 每台 2 MiB；运行态与重置副本合计 4 MiB | 不能 |
| BIOS 缓冲 | 每台 512 KiB；合计 1 MiB，内置演示也预留 | 不能 |
| 模拟 GPU VRAM | 每台 1024×512×2 字节，即 1 MiB；合计 2 MiB | 不能 |
| SPU 声音 RAM | 每台 512 KiB；合计 1 MiB | 不能 |
| CPU / GTE 状态 | 一个 `Cpu` 内联 608 字节；没有独立寄存器堆缓冲 | 不能关闭模拟 CPU；UI 快照仍保留这个小型状态 |
| 反汇编快照 | 本次 9 行的 Vec/String 容量共 552 字节；关闭时为 0 | 可以，已实现 |
| 主机音频队列 | 容量 4096 个立体声 i16 帧，即 16 KiB；SPU 输出队列另预留 32 KiB | 不随调试器关闭；上表宿主音频还包括 CPAL/PipeWire |
| CD-ROM / MDEC / SIO | 结构体分别 736 / 1176 / 472 字节；FIFO、媒体元数据等按实际使用增长 | 设备状态必须继续工作；CD 诊断 trace 默认未开启 |
| 记忆卡 | 每张原始卡 128 KiB；启用两槽及其重置副本时数据缓冲共 512 KiB | 不随调试器关闭；本次测量断开两槽 |
| RGB 帧 / UI BGRA 帧 | 320×240 时分别 225 / 300 KiB；RGB 通过 Arc 共享，在途帧数影响瞬时值 | 调试页释放 UI BGRA；核心 RGB 快照继续更新 |
| CRT 覆盖图 | 本次关闭为 0；开启后像素缓冲为物理宽×高×4 字节，另有 GPU 资源 | 调试页释放；返回画面按需重建 |

四项主要机器缓冲合计 **8 MiB**，与 UI 重构无关。`Machine` 内联 8808 字节，`Bus` 内联 8112 字节，GPU 584 字节，SPU 3792 字节，`Snapshot` 968 字节；这些父结构包含子结构，不能逐项累加为堆占用。启用 JIT、线程绘图或 Vulkan 后的动态缓存与驱动内存不在这组软件解释器测量中。

## 实施的优化与开关

1. **图标缩减与延迟加载**：38px 侧栏图标原先保留 1254×1254 的解码图（6,290,064 字节），还通过 `to_vec()` 复制 1,239,922 字节压缩 PNG。现在借用嵌入字节，首次绘制侧栏时只保留 256×256 BGRA（262,144 字节），同一窗口复用。完整包装图标没有改动。两项确定缓冲合计减少约 **6.93 MiB**。
2. **调试器默认关闭**：进入左侧 Debugger 后点击 Enable debugger / 开启调试器。可用同一按钮关闭；`--debugger` 可直接启用并打开页面。不持久化此开关，下次正常启动仍关闭。
3. **按可见页面采集**：Console、游戏视图和全屏使用 Display 快照，不生成反汇编。启用后返回调试页才恢复 Debug 快照，并等待新快照再显示寄存器。UI 会丢弃关闭前已在途的反汇编数据。
4. **隐藏视频不转换**：调试页释放 UI 视频图像和 CRT 覆盖缓存；显示恢复时只转换最新帧。用一个更新标记合并待显示帧，不额外缓存上一帧 RGB，也不重复做像素比较。
5. **保持设备运行和音频连续性**：切换快照模式不执行 CPU 指令、不重置节奏、不清空宿主或 SPU 音频，也不强制刷写记忆卡。正常设备执行和定时保存继续使用原路径。

保留了原有 `Runner` 构造函数和 `Snapshot::capture()` 的完整调试快照行为，供现有诊断客户端使用；桌面使用新增的快照模式。保留 608 字节 CPU 拷贝、RGB/图像的必要共享引用、字体/图集缓存和机器重置副本，没有引入第三方依赖、额外线程或 unsafe。

缩小图标仍需在首次使用时解码完整原图并缩放，因此启动瞬时峰值没有被完全消除；缩放只发生一次。关闭调试器也不强制清空 GPUI 的所有字体/图集缓存，避免来回切换重复加载。下一步可测量预生成小图标对冷启动峰值的影响；共享只读 BIOS 或改变重置副本需要另行验证 API 与重置语义，未在此轮改动。

## 快照微基准与验证

`cargo run --locked --release --example snapshot_bench`，同一内置演示状态、预热 2000 次，5×100000 次，中位数：

| 操作 | 耗时 |
| --- | ---: |
| 完整 Debug 快照 | 0.622 µs |
| Display 快照 | 0.057 µs |
| CPU 拷贝 | 0.010 µs |
| 单独格式化 9 条反汇编 | 0.571 µs |

Display 快照比完整快照少约 0.565 µs。即使每秒 60 次也只是约 34 µs/s；它是小额重复分配的清理，不能推导为游戏帧率大幅提升。

- `cargo fmt --all --check`：通过。
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`：通过；依赖 `proc-macro-error2` 仍有既存 future-incompatibility 提示。
- `cargo test --locked --workspace`：197 通过，5 个需特定硬件/环境的测试按原配置忽略。
- `cargo test --locked --features ui-tests --bin eidoryn`：9 通过，覆盖开关、离开/返回调试页、释放显示资源及既有导航/布局。
- `cargo build --locked --release`：通过。
- 内存脚本测试：4 通过。原生窗口确认图标和调试器按钮正常显示。

未测量 Windows/macOS，也未用本次内置演示验证商业游戏性能或兼容性。

## 复现

保留每版的原始 Release 二进制，不共用不同检出的 Cargo target 目录。

```bash
cargo build --locked --release
cp target/release/eidoryn /tmp/eidoryn-memory-final
python3 scripts/profile_ui_memory.py capture \
  --binary /tmp/eidoryn-memory-final --output /tmp/eidoryn-console-rss \
  --language en --niri --rss-only
python3 scripts/profile_ui_memory.py capture \
  --binary /tmp/eidoryn-memory-final --output /tmp/eidoryn-debug-heap \
  --language en --view debugger --niri
python3 scripts/profile_ui_memory.py report /tmp/eidoryn-debug-heap/end.heap \
  --base /tmp/eidoryn-debug-heap/start.heap --binary /tmp/eidoryn-memory-final
python3 -m unittest discover -s scripts -p test_profile_ui_memory.py
```

测量工具与归因限制详见 [UI 内存测量](UI-MEMORY.md)。每次使用新的输出目录，保持窗口尺寸、语言、缩放、后端和焦点一致；RSS 与注入分析器的进程分别运行。

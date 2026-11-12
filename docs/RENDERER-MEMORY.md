# 渲染器切换的内存测量

本轮从 `b0673f1` 新建 `perf/renderer-memory-residency`，没有合并到主分支。

**已定位 DMS 中内存数字突然下降的原因：监视器跨过 100 MiB 阈值后更换了统计口径。** 这次显示值下降不能作为 JIT/Vulkan 将进程内存减半的证据。首轮只比较 RSS，因此没有看到同样的跳变；追加同时采集 DMS 数据后，确认了这个差异。

独立验证的模拟器优化仍然有效：同一场景下，DRM 客户端统计的驻留内存从约 **450 MiB 降到 210 MiB**，并消除每次进入 Vulkan 时的一份 **2 MiB 临时转换副本**。进程 RSS 没有测到明确改善，不能把驱动统计的减少说成进程 RSS 的减少。

## DMS 为什么看起来减少了一半

本机安装的是 `dux-shell 2.1.2-1.1` 和 `dux-gop 2.1.1-1.1`，仍以 `dms` / `dgop` 命令运行。对应版本的 [dgop 代码](https://github.com/duxlabs-dev/dux-gop/blob/v2.1.1/gops/processes.go#L475) 使用以下规则：

- RSS ≤ 102400 KiB（100 MiB）：`memoryKB` 使用 RSS，`memoryCalculation` 为 `rss`。
- RSS > 100 MiB，且成功读取到非零 `Pss_Dirty`：`memoryKB` 改用该值，`memoryCalculation` 为 `pss_dirty`。
- 读取失败则保留 RSS。

`Pss_Dirty` 是按比例分摊的脏页统计，不是全部驻留内存、完整 PSS 或 Rust 存活堆。dgop 直接读取 [`smaps_rollup` 的这一字段](https://github.com/duxlabs-dev/dux-gop/blob/v2.1.1/gops/processes_linux.go#L22)。[DMS 界面](https://github.com/duxlabs-dev/dux-shell/blob/v2.1.2/quickshell/Services/DgopService.qml#L926) 则使用返回的 `memoryKB`；因此同一列会混合两种口径。

内置原创演示也能复现这个问题，无需 BIOS、光盘或存档。以下为同一个进程的连续采样，使用中文、Ember、Classic CRT、1280×900 窗口；这组数据不与后文英文/CRT 关闭的优化基线混用：

| 时刻 | 内核 RSS（MiB） | 内核 Pss_Dirty（MiB） | DMS 数据字段（MiB） | `memoryCalculation` |
| --- | ---: | ---: | ---: | --- |
| 25.49 秒 | 98.37 | 49.06 | 98.37 | `rss` |
| 26.60 秒 | 100.29 | 50.74 | 50.74 | `pss_dirty` |
| 29.89 秒 | 100.31 | 28.40 | 28.40 | `pss_dirty` |

**真实 RSS 增加时，显示值反而从 98.37 降到 50.74 MiB。** 后续 RSS 基本不变，脏页统计还会变化。该内置演示在切回软件后越过阈值，说明这种显示跳变也不要求正在开启硬件加速。具体数值随场景、字体、CRT、文件页状态和分配器变化，关键是阈值两侧比较的指标不同。

记录见 [DMS 口径对照](benchmarks/dms-memory-accounting-2026-11-12.json)。采集的是已安装 `dgop` 的真实输出，只保留探针 PID 的内存字段；没有按已知阈值伪造一个 DMS 读数。每次启动独立 CLI 获取新值，UI 常驻后端的缓存可能带来额外延迟。

后续应固定使用 RSS/PSS/存活堆分别比较。真实优化若让 RSS 从 100 MiB 以上降到以下，DMS 甚至可能显示“增加”，因此不能仅凭该列判断回退。此次没有修改系统里的 DMS，也没有通过额外分配来让显示值变小。

## 如何理解切换时的内存变化

- 软件绘图器的 VRAM 为 `1024 × 512 × 2` 字节，即 **1 MiB**，没有发现可在切换时直接释放约 40 MiB 的软件光栅化缓存。机器重置副本、CPU、音频和 UI 不会因切换后端而全部退出。
- PS1 使用软件绘图时，**GPUI 窗口仍使用 GPU**。切换 PS1 后端会额外创建设备、管线、上传和读回资源，并不是用 Vulkan 完整替换整个 UI 的软件实现。
- RSS 包括实际驻留的库代码和数据映射。本机软件启动时，仅 `libLLVM.so.23.1` 的文件映射 RSS 就约 **25.4 MiB**，`libvulkan_lvp.so` 约 **2.8 MiB**；运行中开启 Vulkan 后这些映射仍然存在。一次只允许 RADV 驱动加载的隔离对照，将软件启动 RSS 从 81.62 降至 76.39 MiB，LLVM 映射仍在，不能解释约 40 MiB 的下降。没有在应用中加入驱动白名单或修改全局环境。
- **释放对象不等于 RSS 立即下降。** 本轮独立存活堆采样中，软件阶段约 16.8–17.0 MiB，Vulkan 阶段约 21.3–21.5 MiB，切回软件又回到约 16.8–17.0 MiB。DRM 驻留统计也恢复到软件阶段水平。原生分配器测试中 RSS 却保持高位，这更符合分配器保留空闲页及新增代码页驻留的表现。堆采样使用了另一种分配器，不能据此精确拆分原生 RSS，也不能替代长期泄漏测试。

本次读取的 `/proc/PID/status` 中 `VmRSS` 与 `smaps_rollup` 的 RSS 在稳定采样时基本一致。JIT 开关单独测试，以及先 JIT 后 Vulkan、先 Vulkan 后 JIT 两种顺序，均通过现有 Worker 命令执行并记录实际 CPU/GPU 状态。JIT 编译块和本地指令计数也写入诊断日志，避免把未生效的开关当成成功样本。

## 测量条件

2026-11-12，Linux 7.3.1、Rust 1.98.1、AMD Radeon 780M / RADV、GPUI 0.2.2、Niri、150% 显示缩放。Release 构建，内置原创演示，1280×900 逻辑像素窗口，可见且聚焦；英文、Midnight、CRT 关闭、静音、解释器、调试器关闭、断开两张记忆卡。

原生分配器前后各 **3 个独立进程**，每个进程顺序执行：

| 时间 | 请求 |
| --- | --- |
| 启动 | Serial |
| 5 秒 | Threaded |
| 10 秒 | Serial |
| 15 秒 | Vulkan |
| 20 秒 | Serial |
| 25 秒 | Vulkan |
| 30 秒 | 正常关闭 |

诊断通过现有 Worker 命令切换，记录确认的后端、请求序号、运行状态及错误信息。每 2 秒读取一次进程页统计和 DRM 客户端统计，下面使用各阶段最后一次采样，再取三个进程的中位数。全部序列成功，采样时窗口均保持聚焦。诊断 CSV 缓冲在两版中相同，少量随时间触页，因此这些数值不能直接视为普通启动的绝对基线。

另用 gperftools 的定时转储采集一个优化后进程，观察已分配且尚未释放的请求字节数；该进程的 RSS 不参与原生分配器比较。没有用信号中断分配器。原始转储和机器路径留在本地，仓库只保存 [脱敏测量记录](benchmarks/renderer-memory-2026-11-12.json)。

## 实测结果

单位 MiB。DRM 列为去掉重复文件描述符后，各客户端 `resident-gtt + resident-vram` 的和，可能仍含跨客户端/进程共享对象；**不能加到 RSS，也不是纯 Rust 堆或独占显存**。

| 阶段 | 优化前 RSS | 优化后 RSS | 优化前 DRM 驻留 | 优化后 DRM 驻留 |
| --- | ---: | ---: | ---: | ---: |
| 初始 Serial | 81.70 | 81.40 | 183.14 | 183.14 |
| Threaded | 82.26 | 82.03 | 183.14 | 183.14 |
| 切回 Serial | 83.20 | 82.97 | 183.14 | 183.14 |
| 首次 Vulkan | 90.19 | 89.71 | 449.92 | 209.91 |
| 再切回 Serial | 91.93 | 91.50 | 183.15 | 183.14 |
| 再次 Vulkan | 96.19 | 96.32 | 449.92 | 209.91 |

首次 Vulkan 的 RSS 范围分别为 **90.09–90.37**、**89.67–90.22**；再次 Vulkan 分别为 **96.09–96.58**、**95.44–96.34**。范围重叠，且变化方向并不一致，不据此宣称 RSS 获益。首次 Vulkan 的 PSS 中位数为 77.80 / 77.31 MiB；PSS 还受其他进程共享页情况影响。

DRM 驻留减少约 **240 MiB**，来自为这个小型渲染器选择更合适的分配策略。本机软件阶段已有约 183 MiB 的 UI/驱动驻留；新增 PS1 Vulkan 后端的统计增量从约 **267 MiB 降到 27 MiB**。这是本机集成 GPU 的观察，不是所有显卡的固定收益。

## 修改范围与权衡

1. **减小过大的分配块**：在 [Vulkan 设备初始化](../crates/ps1/src/gpu/vulkan/mod.rs) 中显式使用 `wgpu::MemoryHints::MemoryUsage`。锁定的 wgpu 29.0.4 Vulkan 实现中，默认策略的设备块起始值为 128 MiB，主机块为 64 MiB；节省内存策略分别从 8 / 4 MiB 起步。应用的七个固定缓冲总共只有约 **7.625 MiB**。这个提示允许后续按需增长，不限制客体 VRAM、不降低画质，也没有减少同步屏障。
2. **直接构造上传数据**：把软件 VRAM 的 u16 数据直接扩展到 `Queue::write_buffer_with` 提供的写入视图，省去 `Vec<u32>` 的 2 MiB 分配和随后整块复制。GPU 上传缓冲仍然必需，仍在队列同步后确认切换成功。分配失败经原有错误路径上报，旧后端继续由原有切换逻辑保护。
3. **保留必要状态**：用于重置和回退的副本、GPU 读回缓存、队列暂存及 GPUI 资源没有被强行清空。没有引入依赖、unsafe、全局分配器更换或周期性的强制内存回收。

更小的分配块可能增加驱动分配次数，收益和耗时取决于 GPU/驱动；因此同时执行下述正确性和性能验证。消除 2 MiB 临时对象是确定的分配量减少，不保证 RSS 恰好下降 2 MiB。

## 性能对照

使用现有 `gpu_bench 240`，原创合成纹理叠加负载，包含 scanout/readback，不计设备初始化耗时。前后版本交替运行，各 3 个进程，每个进程内部 3 轮；每轮都逐字比较三个后端的最终完整 VRAM，全部一致。详细轮次见测量记录。

以下是各进程中位数再取中位数，单位 ms/frame，括号为三个进程中位数的范围：

| 后端 | 优化前 | 优化后 |
| --- | ---: | ---: |
| Serial | 4.383（4.343–4.510） | 4.380（4.356–4.399） |
| Threaded | 4.422（4.405–4.424） | 4.419（4.388–4.431） |
| Vulkan | 0.840（0.810–0.845） | 0.854（0.790–0.882） |

Vulkan 的汇总耗时约增加 1.7%，但不同进程间波动范围重叠，无法证明这是稳定的回退，也没有证据支持性能提升。此次收益是更少的内存占用；上传转换发生在初始化/切换阶段，该部分耗时不在本基准计时范围内。

## 验证

- `cargo fmt --all --check`：通过。
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`：通过；依赖 `proc-macro-error2` 的既存 future-incompatibility 提示仍在。
- `cargo test --locked --workspace`：197 通过，5 个硬件测试按默认配置忽略。
- `cargo test --locked --test vulkan_gpu -- --ignored --test-threads=1`：上述 5 个硬件测试在真实 GPU 上全部通过。
- `cargo test --locked --features ui-tests --bin eidoryn`：9 通过。
- `cargo check --locked --no-default-features --all-targets`：通过。
- Release 主程序、渲染/CPU 诊断和 GPU benchmark 构建：通过。
- Python 内存归因/统计测试：9 通过，覆盖共享映射累计、重复 DRM 文件描述符、CPU 切换、失败状态识别，以及显示值与 RSS 的分离。

本轮未测量 Windows/macOS、其他 GPU 驱动、长时间持续游玩或游戏帧率，不将合成光栅耗时换算为游戏 FPS。

## 复现

```bash
cargo build --locked --release --example renderer_memory_probe --example gpu_bench
python3 scripts/profile_renderer_memory.py \
  --binary target/release/examples/renderer_memory_probe \
  --output /tmp/eidoryn-renderer-memory-run --niri
target/release/examples/gpu_bench 240
python3 -m unittest discover -s scripts -p 'test_profile_*memory.py'
```

每次使用新的输出目录。`--niri` 只调整探针自己的窗口；其他 Linux 桌面省略该参数，并手动保持相同尺寸和可见性。工具保存 RSS/PSS、`VmRSS`、逐映射统计、去重的 DRM 客户端数据和后端确认日志；汇总只使用运行中的稳定阶段，失败、暂停或不完整的切换序列不能当成成功样本。采样前后若观察到后端或运行状态改变，则标记 `phase_stable=false` 并排除该条汇总；各内核字段与 dgop 仍是顺序读取，并非原子快照。

比较两版时先分别保留构建好的二进制并记录 SHA-256，不共用不同检出的 Cargo target 目录。普通应用不会记录这些诊断数据或自动切换渲染器。

JIT/Vulkan 组合及 DMS 口径复现：

```bash
cargo build --locked --release --example backend_memory_probe
python3 scripts/profile_renderer_memory.py \
  --binary target/release/examples/backend_memory_probe \
  --output /tmp/eidoryn-backend-memory-run --niri \
  --backend-sequence jit-first --dgop --language zh-CN --theme ember --crt classic
```

改为 `--backend-sequence vulkan-first` 可交换顺序。这个诊断在 12、17 秒分别开启两个后端，22、26 秒依次关闭，30 秒退出。未安装 `dgop` 时省略 `--dgop`；程序不会自动安装或更改监视器。可选 `--bios PATH --disc PATH` 会在窗口创建后依次通过 Worker 加载、启动，两个记忆卡槽保持断开；这些原始本地测试输出不要提交到仓库。

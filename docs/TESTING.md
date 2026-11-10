# 验证与诊断

测试使用自主构造的设备输入与原创 MIPS 程序，不需要商业游戏或专有 BIOS。仓库不分发实际游戏测试记录、按键脚本、镜像、存档、截图或音频。

## 常规检查

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
cargo test --locked --no-default-features
cargo build --locked --release
```

工作区检查必须包含 `--workspace`，以覆盖独立核心的单元测试和宿主组件测试。单独验证模块边界：

```bash
cargo test --locked -p eidoryn-ps1 --no-default-features
cargo test --locked -p eidoryn-host --no-default-features
cargo test --locked -p eidoryn-host --all-features
cargo tree --locked -p eidoryn-ps1 --no-default-features
cargo tree --locked -p eidoryn-host --no-default-features
```

后两项应只输出对应 crate 自身。应用层集成测试继续使用历史库入口，验证已有调用方、CLI、存档和运行行为的兼容性。

测试覆盖 CPU 延迟槽/异常、解释器/JIT 对照、GTE、软件绘图、GPU 时序、DMA 边界与循环、MDEC 输出、CD-ROM/XA/CDDA、SPU、SIO、记忆卡持久化、媒体校验、线程命令与 CLI。GPU 测试默认忽略，以便无显卡环境运行常规检查。

有物理 Vulkan 设备时另行运行：

```bash
cargo test --locked --test vulkan_gpu -- --ignored --test-threads=1
```

该组检查硬件/软件完整显存一致性、纹理反馈、批次依赖、扫描输出与后端切换；无物理设备时会失败，不静默降级为 CPU 驱动。

## 原创光盘端到端测试

```bash
mkdir -p test-assets
cargo run --locked --no-default-features -- --export-disc-test test-assets/probe.exe
python3 scripts/make_test_disc.py test-assets/probe.exe test-assets/probe.iso
cargo run --locked --no-default-features -- \
  --disc test-assets/probe.iso --fast-boot --headless --frames 20 \
  --no-memory-cards --dump-frame test-assets/probe.ppm
```

输出拒绝覆盖已有文件。程序由客体 CPU 发出光驱命令、响应 IRQ、用 DMA3 读取卷描述符并验证完整扇区，再绘制水晶与网格。预期为 400,000 次指令、26 条 GPU 命令、1 个 CD 扇区；历史软件输出 PPM SHA-256：`5fab6a61e185611bd53dd0365a617cce1acb4bfa231fa9f0fdc324b065782f7f`。

`--frames` 是有界执行片段数，单片最多 20,000 次 CPU step，不是 VBlank 数量。桌面 FPS 使用实际模拟视频帧数。

## 可复现的微基准

```bash
cargo run --locked --release --no-default-features --example device_bench
cargo run --locked --release --no-default-features --example cd_bench
cargo run --locked --release --example gpu_bench -- 120
cargo run --locked --release --example cpu_bench
```

`cd_bench` 分别测量命令响应、raw 扇区 FIFO 和 cooked ISO / EDC 路径，打印校验和，共五轮。它只创建原创临时媒体，正常退出自动清理。

`device_bench` 分别测量 10,000 条 1,024 节点 DMA 链和 20,000 个 MDEC 宏块，共五轮，打印命令计数与输出校验和。`gpu_bench` 使用原创多层精灵，包含扫描输出/读回并比较完整显存。`cpu_bench` 使用原创整数循环。微基准不等于整机加速比。

请用 release 构建，在同一设备、电源模式与驱动下重复运行；避免编译、后台模拟器或其他重负载干扰。详细结果和局限见 [PERFORMANCE.md](PERFORMANCE.md)。

## 本地诊断工具

`examples/play_probe.rs` 可对自行提供的媒体进行交互诊断；帮助列出了参数和命令。它经客体手柄协议输入，不修改游戏代码。测试自己的存档时使用卡文件副本。

- `mark` / `restore`：保留、恢复本次进程内的临时检查点。
- `frame-bench FRAMES HEX_MASK PREFIX`：三个独立克隆，每 30 个视频帧按下指定按键两帧；记录执行/扫描时间、PC、设备计数和 RGB/PCM 哈希。范围 1–1,800 帧，不启用宿主限速。
- `window-probe SECONDS HEX_MASK CSV`：真实 GPUI 窗口，按宿主时间每半秒输入一次，记录快照接收与 render 调用；1–30 秒后关闭。只使用内存卡，不改动原会话。
- `render-bench` / `render-compare` 与 `cpu-bench` / `cpu-compare`：比较后端耗时和状态一致性。
- `record` / `stop-record`：写出核心 PCM；`pace`：使用系统声音测量实时调度。

窗口探针记录的是 GPUI 回调，不是显示器呈现时间戳；像素相同的帧会复用图像，不能把 render 帧号间隔全部算成漏帧。常规窗口不注入诊断输入、不收集 CSV。

协议级测试与软件/硬件一致性测试不能证明完整实机精度或所有游戏兼容。声音仍需实机波形比较，设备时序与同步呈现也有进一步验证空间。

# 渲染后端与开关

在侧栏点击 **图形与外观**，在游戏工具栏点击 **图形**，或按 **F8** 打开右侧设置面板：

- **Vulkan acceleration**：开启自主实现的 Vulkan 硬件绘图；关闭后恢复之前选择的软件模式。
- **Software multithreading**：软件绘图的多线程开关。关闭时由解释器线程绘图，开启时由独立工作线程绘图。
- **CPU JIT · experimental**：独立的实验性 CPU 原生执行开关，默认关闭；范围见 [JIT.md](JIT.md)。

Vulkan 始终通过独立线程提交任务，启用时软件多线程开关变灰。开关在当前会话中生效，不重置 CPU、RAM、显存或记忆卡；重置游戏、切换媒体后保留所选后端。切换会等待已提交绘图完成并迁移显存，可能短暂停顿。设置尚不写入配置文件，下次启动可用参数指定。

| 启动参数 | 绘图方式 |
| --- | --- |
| `--renderer serial` | 单线程软件绘图，默认 |
| `--renderer threaded` | 多线程软件绘图 |
| `--renderer vulkan` | Vulkan 计算着色器绘图，独立提交线程 |

```bash
cargo run --locked --release -- --renderer vulkan \
  --bios /path/to/bios.bin \
  --disc /path/to/game.cue \
  --run --fullscreen --crt classic
```

默认构建含 `desktop`、`vulkan` 和 `jit` 特性。只构建无窗口 Vulkan 核心可用 `--no-default-features --features vulkan`；`--no-default-features` 仍是仅用标准库的软件核心。Vulkan 模式需要集成或独立显卡及可用驱动，不会选用 llvmpipe 等 CPU 适配器；初始化失败会报告原因并保留当前会话，可继续使用软件模式。当前没有 OpenGL 后端。

## 实现范围

GPU 命令、光栅化算法和 WGSL 着色器均为本项目实现。wgpu 29.0.4 只提供安全的设备、缓冲和 Vulkan 提交接口，不包含 PS1 模拟核心。硬件后端处理三角形/四边形、精灵、线段、4/8/15-bit 纹理、CLUT、纹理窗口、透明混合、抖动、遮罩、显存上传/复制及 RGB15/RGB24 显示转换。

显存驻留在 GPU storage buffer 中，每个 PS1 半字使用一个 u32，避免两个线程竞争同一个打包字。图元按 8×8 像素分块，每个像素线程依次执行该块的命令列表，保持透明混合与遮罩的先后关系。纹理读区域与写区域有依赖时拆分批次；UV、纹理窗口和翻转参与依赖范围计算。自身纹理反馈和重叠显存复制使用单个有序 GPU invocation，保持软件基准的逐行反馈行为。CPU 上传合并为连续行段，缓冲和队列均有上限。

VBlank 采集沿用原有时序，先于客体中断处理中的清屏/交换缓冲区。显示转换由计算着色器执行，随后把 RGB 读回 GPUI 上传显示；这一步仍有宿主读回/上传开销，尚非零拷贝呈现。克隆、下载显存和切换后端也需要同步。设备错误会上报并停止仿真，不会静默当作软件加速。

当前保持原生分辨率和现有整数像素语义，没有增加内部分辨率、PGXP 或新的 PS1 时序精度。硬件与软件一致性证明两者在测试片段输出相同，不等于所有细节已经与实机一致。CPU、GTE、SPU 和其他设备仍在同一个仿真线程运行，因此光栅化加速不等于整机同比加速。

## 复现检查

常规测试不要求显卡。真实 GPU 测试必须显式运行，找不到物理设备时失败：

```bash
cargo test --locked
cargo test --locked --test vulkan_gpu -- --ignored --test-threads=1
cargo run --locked --release --example gpu_bench -- 120
```

`gpu_bench` 使用原创的 32 层半透明纹理精灵，计时包含扫描输出和读回，三轮交替模式顺序并比较最终完整显存。它衡量绘图负载，不是游戏 FPS。实际游戏对照使用 `play_probe` 的 `render-bench 180000000 vulkan` 与 `render-compare 9000 vulkan`；省略最后的模式参数仍比较软件单线程/多线程。详细环境和测量值见 [验证记录](TESTING.md)。

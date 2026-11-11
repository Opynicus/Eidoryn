# CPU 与设备周期模型

Eidoryn 使用单一的 CPU 时钟域作为 CPU、GPU 时序、root counter、CD-ROM、SIO 与 SPU 的时间基准。标称频率是 33,868,800 Hz，定义在 `eidoryn_ps1::timing::CPU_CLOCK_HZ`。这是一个逐步完善的**近似模型**，并非逐周期准确的 PS1 实现。

## 当前规则

| 操作 | 当前 CPU 周期 |
| --- | --- |
| 普通整数、分支、存储、异常入口 | 1 |
| 从主 RAM 读取（含镜像） | 7 |
| 从片内中断、DMA、定时器寄存器读取 | 5 |
| 从 scratchpad 读取 | 1 |
| MULT/MULTU 启动 | 1，后续运算与 CPU 并行；结果耗时按第一个操作数分为 6、9、13 周期 |
| DIV/DIVU 启动 | 1，后续运算与 CPU 并行；结果耗时 36 周期 |
| MFHI/MFLO | 1；若乘除尚未完成，先等待剩余周期 |
| 其他尚未建模的访问和操作 | 暂按 1 |

RAM 和片内寄存器的数值基于已发表的字读取测量；目前也作为字节/半字读取的近似。第二次乘除会取代之前的等待；MTHI/MTLO 会取消等待。乘除结果在当前实现中仍在启动时计算，而只有 MFHI/MFLO 会等待；取消运算时硬件 HI/LO 的中间值尚未模拟。

每个已执行的 CPU 步骤先完成解释器状态更新，再按 CPU 周期推进设备。每个事件边界按 CD-ROM、SIO、GPU/计时器、SPU 和 DMA/中断的既定顺序处理；中断在下一条 CPU 指令边界处理。设备错误会中止剩余周期并使机器停机；总周期数包含出错的那个周期。JIT 只对支持的整数指令执行原生代码，每条仍推进一个 CPU 周期；其他指令回退到相同的解释器时钟路径。

总线计算距离下一个 CD 命令、延迟完成或扇区事件，以及 SPU 采样或控制变化的周期数。没有这些事件的区间可以批量推进，事件发生的周期仍逐周期执行，保证同周期 CD 音频先于 SPU 采样。GPU 和计时器在批量区间内部仍逐周期推进；SIO 活跃、SPU 手动传输或任意 DMA 通道启动时，保守使用逐周期路径。这样不会因为光驱单次调用只处理一个完成事件而漏掉中间事件，也确保乘除等待期间设备继续工作。

## 查询与控制

- `Machine::step()` 保持一次调试步进的原有接口；`Machine::step_cycles()` 返回该步所用的 CPU 周期数。
- `Machine::elapsed_cycles()` 和 `Bus::elapsed_cycles()` 返回该总线已经开始的累计周期，达到 `u64::MAX` 后回绕。克隆保留当前时钟；从初始机器重置时也恢复其时钟。
- `Machine::run_steps()` 的预算仍为 CPU 步数，而不是周期数；异常入口可能消耗一步但不退休一条指令。
- 直接调用 `Cpu::step()` 只执行 CPU 步骤，不推进设备；调用者可通过 `Cpu::last_step_cycles()` 读取该步骤的估算值，再调用 `Bus::tick()`。应用层应优先使用 `Machine` 接口。
- `Bus::tick(n)` 保持与 `n` 次单周期推进相同的事件顺序和出错边界；`tick(0)` 不推进或触发设备事件。它不是跳过 CPU 的实时等待函数。

## 目前的近似和边界

- 指令缓存、缓存未命中、未缓存取指和 BIOS/外部 I/O 等待状态尚未建模；这类访问暂按单周期处理。
- RAM 读取目前按完整等待计费，没有模拟真实硬件允许独立指令部分重叠的 load shadow、DRAM 刷新或 DMA 总线争用。因此某些程序可能被估计得过慢。
- 写队列、部分字访问的精确总线时序、GTE 命令延迟，以及乘除被 HI/LO 写入中止后的部分结果尚未完整模拟。
- 指令内的访存和寄存器副作用目前是原子执行后再推进设备，不是将总线传输安置在精确的某一个周期；长等待期间产生的 IRQ 在下一指令边界被观察到。
- DMA 传输及 GPU busy、隔行半行、SPU 和光驱的部分细节仍是近似实现。该模型不能作为与实机逐周期一致的声明。

硬件资料：[PSX-SPX CPU load timing 和 multiply/divide](https://psx-spx.consoledev.net/dev/ps1/cpu/cpuspecifications/)、[memory control](https://psx-spx.consoledev.net/memorycontrol/) 和 [DMA](https://psx-spx.consoledev.net/dmachannels/)。

## 验证

```bash
cargo test --locked --test cpu_timing
cargo test --locked --test jit
cargo test --locked --test standalone batched_bus_clocks_keep_cdda_and_spu_in_cycle_order
cargo test --locked --workspace
cargo run --locked --release --no-default-features --example event_boundary_bench
```

测试覆盖 RAM 镜像、片内寄存器、异常、load delay、乘除边界与重叠、等待期间定时器 IRQ、CD 命令与 SPU 采样、解释器/JIT 比较，以及 CDDA、连续命令、GPU/计时器、SIO、SPU 和 DMA 的批量/逐周期一致性。基准使用固定周期和指令数的原创合成负载，不代表游戏 FPS。下一步应与专门的实机时序测试比对，再完善缓存、load shadow 与总线/DMA 争用。

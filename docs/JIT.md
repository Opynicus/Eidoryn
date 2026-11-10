# 实验性 CPU JIT

通过 Graphics/F8 面板的 **CPU JIT · experimental** 开关，或启动参数
`--cpu jit` 启用；`--cpu interpreter` 为默认值。支持 Linux x86-64。
切换保留 CPU、RAM、显存和记忆卡，重置/换媒体后保留选择；设置仅在当前
会话生效。可独立搭配 `serial`、`threaded`、`vulkan` 三种绘图后端。
没有 JIT 编译特性或主机不支持时，接口返回明确错误并保留原执行模式。

这是本项目原创的 MIPS I 到 x86-64 整数代码生成器，没有复制或链接其他
模拟器。当前编译 ADDIU、ADDU/SUBU、逻辑运算、比较、LUI 和移位。
访存、分支及延迟槽、HI/LO、协处理器、带溢出异常的算术仍由解释器执行。
有待提交的加载也先回到解释器。调试单步始终使用参考解释器。

同一入口访问 8 次后才编译，最多 64 条连续指令。直接映射缓存最多 4096
个入口，索引混合 PC 高低位以减少相距 16 KiB 代码之间的冲突；替换会释放不再引用的可执行映射。克隆机器可共享只读代码，但
寄存器、设备、缓存元数据和执行统计各自独立。每条原生指令完成后仍推进
一次设备时钟，并在执行下一条前检查中断和该地址的原始指令字。这使
DMA、RAM 镜像或调试接口写入代码后立即退出旧块，避免执行过期代码。
预编译读取仅使用无副作用的 `peek32`，不会提前读取 MMIO/FIFO。不适合原生执行的热入口会记住最多 16 步的解释执行长度，减少短轮询段重复查缓存；解释器仍逐步读取真实指令，所以该长度只是调度提示，代码变化也不改变语义。内置演示的客体帧计数逐步检查，避免跨过它特有的完成边界。

主模拟器继续使用 `unsafe_code = "forbid"`。原生边界隔离在本地
`crates/jit-runtime`：调用者只能提交经过寄存器/移位范围检查的类型化操作，
不能提交任意机器码或地址。代码通过 memmap2 0.9.12 建立匿名 RW 映射，
写入后转换为 RX；运行时不能修改。生成代码只访问传入的 32 个寄存器，
客体值不会变成宿主指针或跳转地址。回调 panic 在 Rust 中捕获，原生栈帧
返回后再恢复展开。ABI、边界与生命周期约束见
[SAFETY.md](../crates/jit-runtime/SAFETY.md)。

默认构建提供 `jit`，运行时仍默认解释器。无桌面 JIT 构建使用
`--no-default-features --features jit`；关闭全部默认特性仍为仅依赖标准库的
软件解释核心。设备精度、时钟频率和绘图分辨率没有改变。

验证命令：

```bash
cargo test --locked --workspace
cargo test --locked --no-default-features --features jit --test jit
cargo run --locked --release --example cpu_bench
```

`play_probe` 支持 `cpu interpreter|jit`、`cpu-bench 180000000` 和
`cpu-compare 9000`。基准从同一检查点创建解释器/JIT 副本，三轮交替顺序，
包含冷缓存编译、所有设备时钟和 GPU 完成等待，排除副本和后端初始化。
像素对照同时检查每个切片的完整 CPU 状态与全部新生成 PCM，末尾比较
RAM/VRAM。原会话和文件存档不推进。实际结果见 [TESTING.md](TESTING.md)。

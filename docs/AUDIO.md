# 音频

桌面版默认打开系统默认声音设备，音量为 75%。顶部和游戏视图均有 **Mute / Unmute** 及横向音量滑条。点击轨道或拖动圆形滑块连续调整 0–100%，旁边显示百分比；拖出轨道时限制在端点，松开或窗口失焦结束拖动。调整滑条会解除静音，静音开关则保留原音量。滑条使用当前主题的配色。`--mute` 只影响宿主音量，不停止客体音频设备。没有输出设备或设备断开时显示错误，游戏可以继续；**Enable sound** 尝试重新连接系统默认设备。

核心每 768 个模拟设备时钟生成一个 44.1 kHz 双声道 16-bit PCM 帧。实现包括 24 声道 ADPCM、4 点 Gaussian 插值、ADSR、音量扫变、噪声、音高调制、循环和 ENDX、混响 RAM 网络、CD 输入混音、捕获缓冲及 SPU IRQ。CD-XA 支持 4/8-bit、单/双声道、18.9/37.8 kHz 和 zigzag 重采样；CDDA 从 CUE 音轨解包 PCM，支持音量矩阵、静音、播放/暂停和轨道结束通知。

这些解码与混音代码为本项目实现；CPAL 仅负责系统设备输出。硬件插值/滤波常量来自 [PSX-SPX SPU](https://psx-spx.consoledev.net/soundprocessingunitspu/) 和 [CD 格式](https://psx-spx.consoledev.net/cdromformat/) 资料，没有使用其他模拟器源码。

工作线程将 PCM 送到独立的有界播放队列，不通过会被替换的视频快照传递。队列最多 4096 帧（约 93 ms），起播预缓冲 1024 帧；设备回调不分配内存，不等待模拟线程。输出按系统采样率线性重采样，并允许 ±0.5% 微调以吸收声卡和视频时钟的偏差。暂停、重置、换媒体和退出会清空旧音频；跟不上实时速度时重缓冲，避免无限积累延迟。

## WAV 诊断

无窗口模式不打开声卡，可直接录制核心 PCM：

```bash
cargo run --locked --release --no-default-features -- \
  --bios /path/to/bios.bin --disc /path/to/game.cue \
  --headless --frames 5000 --dump-audio test-assets/boot.wav
```

`--frames` 仍表示 20,000 step 的执行片段数量，不是视频帧。WAV 为 44.1 kHz 双声道 PCM，不覆盖已有文件，不受桌面音量影响。`examples/play_probe.rs` 的 `record PATH.wav` / `stop-record` 可在输入脚本期间录制；`pace SECONDS` 在启用 desktop 特性时实际播放声音并测量工作线程 FPS。

## 已验证范围与精度

自动测试覆盖取样数量、声道/循环/释放、中断、噪声、XA 格式和采样率、CDDA 播放及结束、静音、缓冲、重采样和 WAV 字节。

仍非逐周期或逐样本精确实现：混响的 22.05/44.1 kHz 转换目前使用 box/linear 滤波，未实现实机 FIR；XA emphasis、精确 CDDA report 峰值/双速播放重采样、ReadN/ReadS 音轨数据模式、完整 SPU 寄存器锁存/取 RAM 时序尚未实现。声音尚未与实机波形对照，不能据此宣称所有游戏音频均正确。

## 复用到其他主机

宿主音频现位于 `eidoryn-host`，不依赖 PS1 SPU。`AudioBuffer::with_sample_rate` 与 `WavWriter::with_sample_rate` 接受非零源采样率，已测试 32/44.1/48 kHz；旧默认入口仍为 44.1 kHz。系统设备采样率由输出后端处理，与核心源采样率分开。队列容量仍按帧计数，因此不同源采样率的毫秒延迟会不同；接入新核心时需重新测量，不能照搬 PS1 的延迟结论。

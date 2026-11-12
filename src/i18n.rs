//! Compile-time checked English and Simplified Chinese interface catalog.
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    En,
    ZhCn,
}

impl FromStr for Language {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "en" => Ok(Self::En),
            "zh-CN" => Ok(Self::ZhCn),
            _ => Err(format!("Unsupported language: {value}; choose en or zh-CN")),
        }
    }
}

impl Language {
    pub const fn id(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }

    pub fn from_locale(value: &str) -> Self {
        if value
            .split(['_', '-', '.'])
            .next()
            .is_some_and(|language| language.eq_ignore_ascii_case("zh"))
        {
            Self::ZhCn
        } else {
            Self::En
        }
    }

    pub fn preferred() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
            .map_or(Self::En, |locale| Self::from_locale(&locale))
    }
}

// A single bilingual entry defines each key and its exhaustive lookup.
macro_rules! messages {
    ($($key:ident => ($zh:literal, $en:literal)),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Text { $($key),* }
        impl Language {
            pub const fn text(self, key: Text) -> &'static str {
                let (zh, en) = match key { $(Text::$key => ($zh, $en)),* };
                match self { Self::ZhCn => zh, Self::En => en }
            }
        }
        #[cfg(test)]
        const ALL_TEXT: &[Text] = &[$(Text::$key),*];
    };
}
messages! {
    Close => ("关闭面板", "Close panel"),
    Mute => ("静音 / 取消静音", "Mute / unmute"),
    CleanFullscreen => ("纯画面全屏", "Clean fullscreen"),
    CleanFullscreenHint => ("全屏时隐藏控制栏；F12 切换，Esc 退出。", "Hide bars in fullscreen. F12 toggles; Esc exits."),
    KeyboardHint => ("Tab 切换焦点 · Enter 确认 · Esc 返回游戏输入", "Tab to navigate · Enter to activate · Esc for game input"),
    ProgramCounter => ("程序计数器", "Program counter"),
    BuiltinDemo => ("内置原创演示", "Built-in MIPS demo"),
    SerialRenderer => ("串行软件绘图", "Serial software"),
    ThreadedRenderer => ("多线程软件绘图", "Threaded software"),
    VulkanRenderer => ("Vulkan 硬件绘图", "Vulkan hardware"),
    Theme => ("界面主题", "Interface theme"),
    ThemeHint => ("主题只改变界面配色，游戏画面保持原样。", "Themes affect the interface; game colors stay unchanged."),
    CurrentTheme => ("当前主题", "Current theme"),
    ApplyTheme => ("点击应用", "Apply theme"),
    LoadTheme => ("加载自定义主题文件", "Load custom theme"),
    ThemePluginHint => ("支持 JSON 主题插件；应用后无需重启游戏。", "JSON theme plugins apply without restarting the game."),
    ThemePicker => ("加载 Eidoryn 主题插件（.json）", "Load an Eidoryn theme plugin (.json)"),
    Jit => ("CPU JIT · 实验性", "CPU JIT · experimental"),
    PlatformHint => ("可用性取决于平台与构建", "Availability depends on platform and build"),
    Crt => ("CRT 显示效果", "CRT display effect"),
    CrtHint => ("为游戏画面选择显示滤镜。", "Choose a display filter for the game."),
    Off => ("关闭", "Off"),
    Soft => ("柔和", "Soft"),
    Classic => ("经典", "Classic"),
    Performance => ("渲染与性能", "Rendering and performance"),
    PerformanceHint => ("选项是否可用取决于平台、构建和当前模式。", "Availability depends on platform, build, and current mode."),
    Vulkan => ("Vulkan 硬件加速", "Vulkan acceleration"),
    VulkanHint => ("需要受支持的硬件和构建", "Requires supported hardware and build"),
    Threaded => ("软件多线程", "Software multithreading"),
    ThreadedHint => ("为软件渲染启用独立工作线程", "Render software graphics on a separate worker"),
    Applying => ("正在应用设置…", "Applying settings…"),
    GpuThread => ("独立 GPU 提交线程", "Dedicated GPU submission thread"),
    SoftwareHint => ("当前使用软件渲染。", "Currently using software rendering."),
    NoVulkan => ("此构建不包含 Vulkan。", "Vulkan is unavailable in this build."),
    BiosPicker => ("选择 BIOS 固件（.bin/.rom，512 KiB）", "Select BIOS firmware (.bin/.rom, 512 KiB)"),
    ExePicker => ("打开 PS-X EXE", "Open PS-X EXE"),
    BiosRequired => ("请先选择 512 KiB BIOS。自制程序可使用快速启动。", "Select a 512 KiB BIOS first. Homebrew can use Fast boot."),
    FastPicker => ("自制程序 CUE/BIN 或 ISO：跳过 BIOS 初始化", "Homebrew CUE/BIN or ISO: skip BIOS initialization"),
    DiscPicker => ("打开游戏镜像（多轨 BIN 请选择 CUE，或选择 ISO）", "Open game image (CUE for multiple BIN tracks, or ISO)"),
    EnableSound => ("启用声音", "Enable sound"),
    Console => ("控制台", "Console"),
    GameView => ("游戏视图", "Game view"),
    Debugger => ("调试器", "Debugger"),
    Workspace => ("工作区", "Workspace"),
    Platforms => ("模拟平台", "Systems"),
    Current => ("当前", "Current"),
    Unsupported => ("未支持", "Unavailable"),
    Settings => ("设置", "Settings"),
    Controller => ("手柄", "Controller"),
    Turbo => ("手柄连发", "Controller turbo"),
    TurboHint => ("选中需要连发的按键，再长按对应键盘键。松开即停止；未选中的按键保持普通输入。默认全部关闭。", "Select buttons to repeat, then hold their keyboard keys. Release to stop. Unselected buttons work normally; all are off by default."),
    TurboRate => ("连发速度", "Turbo rate"),
    TurboRateHint => ("Hz 表示每秒按下次数。设置自动保存；方向键、Start 和 Select 不连发。", "Hz is presses per second. Settings are saved automatically. Directions, Start and Select do not repeat."),
    TurboClear => ("关闭全部连发", "Disable all turbo"),
    Restore => ("还原", "Restore"),
    Maximize => ("最大化", "Maximize"),
    BackConsole => ("返回控制台", "Back to console"),
    SettingsHint => ("调整你的游玩体验", "Customize your experience"),
    GraphicsSettings => ("图形设置", "Graphics"),
    Appearance => ("外观主题", "Appearance"),
    WorkspaceCrumb => ("工作区   /", "Workspace   /"),
    ConsoleEyebrow => ("PLAYSTATION / 工作区", "PLAYSTATION / WORKSPACE"),
    Welcome => ("欢迎回来，准备开始。", "Welcome back. Ready to play."),
    WelcomeHint => ("把注意力留给游戏，其余交给控制台。", "Focus on the game. Let the console handle the rest."),
    ChooseBios => ("选择 BIOS", "Choose BIOS"),
    OpenDisc => ("打开光盘镜像", "Open disc image"),
    Stopped => ("已停止", "Stopped"),
    Running => ("运行中", "Running"),
    Paused => ("已暂停", "Paused"),
    Display => ("画面输出", "Display output"),
    Pause => ("暂停", "Pause"),
    Run => ("运行", "Run"),
    Graphics => ("图形", "Graphics"),
    Fullscreen => ("全屏", "Fullscreen"),
    RunPause => ("运行 / 暂停", "Run / pause"),
    NotSelected => ("未选择", "Not selected"),
    Session => ("当前会话", "Current session"),
    DiscSession => ("PlayStation · 光盘镜像", "PlayStation · Disc image"),
    LocalSession => ("PlayStation · 当前会话", "PlayStation · Current session"),
    Renderer => ("渲染模式", "Renderer"),
    SectorsRead => ("读取扇区", "Sectors read"),
    OpenExe => ("打开 EXE", "Open EXE"),
    FastBoot => ("快速启动（自制程序）", "Fast boot (homebrew)"),
    Demo => ("启动内置演示", "Start built-in demo"),
    MemoryCards => ("记忆卡", "Memory cards"),
    Disconnected => ("未连接", "Disconnected"),
    TemporaryCard => ("临时记忆卡", "Temporary card"),
    Saving => ("保存中…", "Saving…"),
    Synced => ("已同步", "Synced"),
    SyncCards => ("同步卡数据", "Sync card data"),
    SaveHint => ("请在游戏内完成存档；同步只会写入已有卡数据。", "Save inside the game. Sync only writes existing card data."),
    Slot => ("卡槽", "Slot"),
    DebugEyebrow => ("PLAYSTATION / 开发工具", "PLAYSTATION / DEVELOPER TOOLS"),
    DebugWorkspace => ("调试工作区", "Debug workspace"),
    DebugHint => ("将硬件状态与执行过程放在一起观察。", "Inspect hardware state alongside execution."),
    EnableDebugger => ("开启调试器", "Enable debugger"),
    DisableDebugger => ("关闭调试器", "Disable debugger"),
    DebuggerOff => ("调试器已关闭", "Debugger is off"),
    DebuggerOffHint => ("按需开启以查看寄存器与反汇编。离开此页会暂停采集，游戏继续正常运行。", "Enable it to inspect registers and instructions. Collection pauses when you leave this page; emulation continues normally."),
    DebuggerLoading => ("正在获取调试快照…", "Waiting for a debug snapshot…"),
    Instructions => ("指令流", "Instruction stream"),
    Address => ("地址", "Address"),
    Opcode => ("机器码", "Opcode"),
    Instruction => ("指令", "Instruction"),
    Step => ("单步执行 · F10", "Step · F10"),
    Slice => ("执行切片 · F9", "Slice · F9"),
    Reset => ("重置", "Reset"),
    Registers => ("CPU 寄存器", "CPU registers"),
    GpuStatus => ("GPU 状态", "GPU status"),
    Resolution => ("画面分辨率", "Resolution"),
    Memory => ("内存 / 显存", "RAM / VRAM"),
    InstructionsExecuted => ("已执行指令", "Instructions executed"),
    Speed => ("速度", "speed"),
    Shortcuts => ("F6 游戏视图  ·  F7 CRT  ·  F8 图形  ·  F11 全屏", "F6 Game  ·  F7 CRT  ·  F8 Graphics  ·  F11 Fullscreen"),
    Language => ("语言", "Language"),
    LanguageHint => ("选择界面语言，下次启动继续使用。", "Choose the interface language. Saved for the next launch."),
    FilePickerError => ("文件选择器", "File picker"),
    ThemePickerError => ("主题选择器", "Theme picker"),
    ConfigError => ("无法保存设置", "Cannot save preferences"),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_are_complete_and_locale_parsing_is_predictable() {
        for &key in ALL_TEXT {
            assert!(!Language::En.text(key).trim().is_empty());
            assert!(!Language::ZhCn.text(key).trim().is_empty());
        }
        assert_eq!("en".parse::<Language>().unwrap(), Language::En);
        assert_eq!("zh-CN".parse::<Language>().unwrap(), Language::ZhCn);
        assert!("fr".parse::<Language>().is_err());
        assert_eq!(Language::from_locale("zh_CN.UTF-8"), Language::ZhCn);
        assert_eq!(Language::from_locale("en_US.UTF-8"), Language::En);
        assert_eq!(Language::from_locale("C"), Language::En);
    }
}

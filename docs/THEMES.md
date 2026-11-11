# 主题与插件

点击 **图形与外观**，在右侧面板选择 **外观主题**。Lavender（浅紫）、Midnight（深紫）、Ocean（浅蓝）、Forest（深绿）为内置主题，桌面默认使用 Midnight。点击色板立即应用；关闭按钮或 Esc 收起面板。游戏视图沿用当前配色，游戏帧保持原始颜色。

## 加载自定义主题

主题插件为 UTF-8 JSON 文件。放入启动目录的 `plugins/themes/` 后会自动加入列表；也可通过 **Load theme…** 选择任意位置的文件，加载后立即应用。编辑文件后再次加载同一 `id` 会刷新主题，不增加重复项。无须重编译或重启游戏。

当前选择在本次会话中有效。固定启动主题可使用：

```bash
cargo run --locked --release -- --theme forest
cargo run --locked --release -- --theme-plugin plugins/themes/ember.json
```

`--theme` 接受主题 `id`；若同时给出两个选项，先加载插件，再按 `--theme` 选择。自动扫描目录相对于当前工作目录；从其他目录启动时可给 `--theme-plugin` 绝对路径。

## 插件格式 v1

```json
{
  "api_version": 1,
  "kind": "theme",
  "id": "my-purple",
  "name": "My Purple",
  "base": "midnight",
  "colors": {
    "primary": "#D0BCFF",
    "on_primary": "#381E72",
    "background": "#141218"
  }
}
```

`base` 为四个内置主题之一。未指定的颜色继承该主题。`id` 为 1–64 个小写字母、数字或连字符，不能使用内置主题 ID；`name` 最多 32 个字符。文件最大 64 KiB，颜色必须是 `#RRGGBB`。

| 颜色字段 | 用途 |
| --- | --- |
| `background`, `surface`, `surface_variant` | 页面、卡片和卡片内次级背景 |
| `border`, `text`, `muted` | 边框、正文和辅助文字 |
| `primary`, `on_primary` | 主按钮/标记背景及文字 |
| `primary_hover`, `primary_pressed` | 主按钮悬停及按下状态 |
| `tonal`, `on_tonal` | 次按钮、选中指令和暂停状态 |
| `tonal_hover`, `tonal_pressed` | 次按钮悬停及按下状态 |
| `error`, `on_error` | 错误提示和停止状态 |
| `success`, `on_success` | 运行状态 |
| `video_background` | 游戏画面周围的留边 |

完整示例见 [Ember](../plugins/themes/ember.json)。当前插件接口仅提供颜色数据，不加载动态库或执行脚本。未知字段、无效颜色或不支持的版本会显示错误，并保留已选主题和游戏会话。

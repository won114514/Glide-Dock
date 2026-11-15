# Glide-Dock 预览图

除 `11`/`12`/`13`/`14` 外，其余图片由同一个 `dock-render` 渲染器离线生成，再合成到
一张程序生成的深色/浅色壁纸上，不需要桌面会话即可评审。Dock 图尺寸均为 1440×900；
特写图从合成结果中裁切。`11`/`12` 是 WinUI 3 设置中心的实机截图，尺寸为 1444×1109；
`13`/`14` 是 Windows.UI.Composition 毛玻璃的实机截图，尺寸 1000×130。`15`–`24` 使用
与实时 Dock 相同的 `MotionEngine` 采样，分别展示 6 套悬停峰值与 4 套启动关键帧。

重新生成 Dock 预览：

```powershell
cargo run -p glide-dock -- --render-preview preview
```

设置中心截图需要先运行 `scripts/build-settings.ps1`，再启动设置进程截取。

| 文件 | 说明 |
| --- | --- |
| `01-dock-dark.png` | 深色主题，悬停放大 + 运行圆点，完整桌面构图 |
| `02-dock-light.png` | 浅色主题，完整桌面构图 |
| `03-dock-empty.png` | 首次启动的空 Dock 提示 |
| `04-dock-tooltip.png` | 悬停名称气泡，完整桌面构图 |
| `05-dock-menu.png` | 右键菜单打开，完整桌面构图 |
| `06-menu-closeup.png` | 菜单 + Dock 特写（主要评审图） |
| `07-dock-drag.png` | 拖动排序：浮动图标 + 插入指示条 + 原槽淡出 |
| `08-tooltip-closeup.png` | 悬停放大 + 提示气泡特写 |
| `09-error-dark.png` | 启动失败：错误气泡 + 失效条目角标，完整桌面构图 |
| `10-error-closeup.png` | 失效条目 + 错误气泡特写 |
| `11-settings-light.png` | WinUI 3 设置中心 · 浅色 · 常规页（搜索、主题、强调色、快速导航） |
| `12-settings-dark.png` | WinUI 3 设置中心 · 深色 · 外观页（滑杆与实时数值） |
| `13-native-dark.png` | 实机截图：Windows.UI.Composition 实时毛玻璃（深色） |
| `14-native-light.png` | 实机截图：Windows.UI.Composition 实时毛玻璃（浅色） |
| `15-motion-glide-zoom.png` | 悬停预设 Glide Zoom 峰值 |
| `16-motion-bounce.png` | 悬停预设 Bounce 跳跃 |
| `17-motion-wave.png` | 悬停预设 Wave 波浪联动 |
| `18-motion-fluid-press.png` | 悬停预设 Fluid Press |
| `19-motion-glow.png` | 悬停预设 Glow 光晕 |
| `20-motion-perspective.png` | 悬停预设 Perspective 倾斜 |
| `21-launch-glide.png` | 启动预设 Glide Launch 100ms 帧 |
| `22-launch-triple-bounce.png` | 启动预设 Triple Bounce 100ms 帧 |
| `23-launch-ripple.png` | 启动预设 Ripple Launch 220ms 帧 |
| `24-launch-pulse.png` | 启动预设 Pulse Launch 130ms 帧 |

评审重点：

- 透明/阴影过渡是否自然，圆角是否干净；
- 图标清晰度与底对齐；悬停放大时邻居的推开距离；
- 菜单、提示与 Dock 的间距和材质一致性；
- 中英文混排、运行圆点、失效角标与错误提示的可读性。

> 图标来自本机已安装应用（Explorer、Edge、Windows Terminal、PowerShell、
> 计算器、记事本、任务管理器、设置），因此不同机器上生成的结果会略有差异。
> 各应用图标版权归其各自所有者，仅用于本项目的界面预览。

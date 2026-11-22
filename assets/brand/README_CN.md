# Glide-Dock 标志资源

标志以开放式 G 为主体，使用几何轮廓、斜切横笔与青蓝渐变。矢量稿使用透明背景，不包含外框、位图、阴影滤镜或字体依赖。

## 矢量源文件

- `05-Source/Glide-Dock-Mark.svg`：G 图形的唯一母版。
- `05-Source/Glide-Dock-Wordmark.svg`：已转换为矢量路径的 Glide-Dock 字标。
- `01-Logo/Glide-Dock-Logo-Symbol.svg`：供安装器和 Slint 设置中心直接使用的 SVG。
- `01-Logo/Glide-Dock-Logo-Horizontal.svg`：横版组合标志，字标随系统深浅色偏好调整颜色。

## 原生图标

PNG 与 Windows ICO 均从同一份 SVG 直接按目标尺寸渲染，保留透明通道。
PNG 提供 16、20、24、32、40、48、64、96、128、256、512、1024 像素；ICO 内含 16–256 像素的十个尺寸。
FullColor 用于默认应用图标，Light/Dark 为背景适配版本；Mono SVG 使用 currentColor。
05-Source 仅保留 SVG 源文件。01-Logo 中的横版 PNG 从新版 SVG 生成，供 WinUI 安装器使用。

## 重新生成

在 installer/ui 中安装开发依赖及 Playwright Chromium 后运行：

```powershell
npm run brand:build
```

也可指定现有浏览器：

```powershell
npm run brand:build -- --browser "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
```

生成脚本同时更新旧设置项目使用的 ICO/PNG，并校验透明角落、黑色边缘和输出尺寸。
04-Showcase/Glide-Dock-Brand-Preview.png 为当前设计预览，同时用于 WinUI 安装器的欢迎页面。

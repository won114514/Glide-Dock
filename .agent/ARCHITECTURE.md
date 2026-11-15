# Glide-Dock 架构设计

> 文档版本 **2.6**（v0.4.0 实现对齐）· 最后更新 **2026-11-14**
> 关联文档：[README.md](README.md)（产品定位与路线图）· [PLAN.md](PLAN.md)（任务卡与门禁）
> 本文取代 1.x 版本中的 `Modern Dock` 设计稿，范围、命名与选型均已对齐。
> **实现状态**：M2 MVP 与 M4 设置中心已落地，M3 部分完成；偏差与 v0.3.3 修复记录见 §17。

---

## 0. 摘要

Glide-Dock 是一个 **内容 100% 由用户拖入决定** 的 Windows 桌面 Dock：用户把 `.lnk`、`.exe`、文件夹或文件拖进 Dock，程序解析并持久化，之后负责启动、排序、动画与外观。

三条不可动摇的架构约束：

1. **不扫描**：启动时不枚举开始菜单、`Program Files`、注册表卸载项，不根据进程列表自动生成条目。
2. **不推断**：运行状态只做**标记**（图标下方小圆点），不新增、不重排、不隐藏条目。
3. **不丢数据**：任何解析失败都保留原始路径，条目仍然可见、可编辑、可删除。

数据流只有一条主干：

```
用户拖入路径 ──► 解析（快捷方式/PE/文件夹） ──► DockItem ──► DockConfig(JSON) ──► 渲染与启动
```

---

## 1. 产品边界

### 1.1 定位

「更轻、更快、更贴近 Windows 11 的 Dock」——保留 Winstep Nexus 的**效率内核**（常驻启动栏、拖放管理、运行状态、外观可调），抛弃它的**视觉负担**（拟物皮肤、多层属性窗口、Win7 风格控件）。

### 1.2 内容模型：显式条目

| 不变量 | 具体要求 | 违反时的后果 |
| --- | --- | --- |
| 不扫描 | 首次启动为空 Dock，只显示占位提示 | 启动变慢、条目噪音、与用户预期不符 |
| 不推断 | 进程监控只更新运行指示器 | 图标乱跳、误删用户配置 |
| 不丢数据 | 解析失败保留 `DockItem`，运行时标记为 unresolved | 用户拖入的东西「消失」，信任崩塌 |

**这意味着**：Dock 的条目集合是用户意志的完整映射。程序永远不「帮用户决定」放什么。

### 1.3 非目标（明确不做）

- 应用商店、插件市场、在线主题下载
- 全盘 / 开始菜单 / UWP 应用自动扫描与收录
- 传统 Windows 属性页（多层 Tab 对话框）
- 皮肤脚本系统、像素级拟物材质
- 内置浏览器内核（WebView2 仅在未来「小组件」中作为可选能力，不进入 Dock 主渲染路径）

### 1.4 与 Winstep Nexus 的能力映射

| 能力 | Nexus 做法 | Glide-Dock 做法 | 里程碑 |
| --- | --- | --- | --- |
| 添加应用 | 扫描 + 手动拖入 | **仅手动拖入**（解析 `.lnk` 目标） | M2 |
| 图标放大 | 固定放大动画 | 鼠标位置驱动的衰减放大 + 弹簧 | M2 |
| 拖动排序 | 支持 | 支持，拖拽阈值 8px，插入点高亮 | M2 |
| 删除条目 | 菜单 | 拖出 Dock 松手 / 右键菜单 | M2 |
| 外观设置 | 多层对话框 + 皮肤 | 单一设置中心，卡片式，实时预览 | ✅ M4 |
| 运行状态 | 是 | 图标底部圆点（仅 Windows 10/11 任务栏语义） | M3 |
| 桌面小组件 | 模块系统 | 统一组件规范，后置 | M5 |
| 皮肤 / 主题包 | 是 | **不做**；只提供深/浅/跟随系统 + 参数化外观 | — |

---

## 2. 设计原则

1. **显式优于推断** — 用户拖什么就有什么，其余一律不猜。
2. **空闲即零成本** — 无动画时定时器必须关闭，消息循环阻塞在 `GetMessageW`，CPU 占用趋近 0。
3. **失败可见、可恢复** — 每个失败路径都有日志、有回退值、有兜底 UI。
4. **边界清晰** — 领域模型不出现 `HWND`，渲染层不出现 Win32 调用。
5. **先稳定，后华丽** — 宁可少一个效果，不可多一次崩溃。

---

## 3. 技术选型

### 3.1 依赖清单（crates.io 实查，2026-11-08）

| crate | 版本 | 用途 | 许可 | 引入阶段 |
| --- | --- | --- | --- | --- |
| `windows` | 0.62.2 | Win32 / COM / Shell / GDI 绑定 | MIT OR Apache-2.0 | M1 |
| `tiny-skia` | 0.12.0 | CPU 光栅：圆角、渐变、图像缩放、混合 | BSD-3-Clause | M1 |
| `ab_glyph` | 0.2.32 | 字体加载与字形光栅（菜单/提示文本） | Apache-2.0 | M2 |
| `serde` / `serde_json` | 1.0.229 / 1.0.154 | 配置序列化 | MIT OR Apache-2.0 | M1 |
| `thiserror` | 2.0.21 | 分层错误类型 | MIT OR Apache-2.0 | M1 |
| `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.33 | 结构化日志 | MIT | M1 |
| `Microsoft.WindowsAppSDK` / WinUI 3（C#） | 1.7.260224002 | 设置中心 UI（独立进程） | Microsoft 软件许可 | ✅ M4 |

工具链：Rust **1.99.1**（stable-x86_64-pc-windows-msvc），Edition 2024（`rust-version = 1.85`），MSVC 生成工具 + Windows SDK。

### 3.2 关键决策：Windows 绑定只锁一条版本线

crates.io 依赖解析结果（实查）：

| 包 | 版本 | 依赖 |
| --- | --- | --- |
| `windows` | 0.62.2 | `windows-core ^0.62.2`、`windows-collections ^0.3.3`、`windows-future ^0.3.3`、`windows-numerics ^0.3.3` |
| `windows-window` | 0.100.0 | `windows-core ^0.100.0` |
| `windows-composition` | 0.100.0 | （同属 0.100 家族） |
| `windows-animation` | 0.100.0 | （同属 0.100 家族） |

**结论**：这些拆分 crate 真实存在，但与主包 `windows 0.62.2` 属于**两条互不兼容的版本线**。同时依赖会在编译产物里出现两份 `windows_core`，`HWND`、`IUnknown`、`Result` 等类型彼此不可转换，必须写类型转换胶水，风险与收益完全不成正比。

**MVP 决策**：主包只依赖 `windows = 0.62.2`；不引入 0.100 拆分 crate。v0.3.3 使用该包自带的 `UI_Composition` 能力实现毛玻璃 HostBackdropBrush，仍保持单一 `windows-core`。

**何时重估**：当 `windows-window` / `windows-composition` 与 `windows` 对齐到同一 `windows-core` 大版本，或整个项目迁移到 0.100 全家桶。重估时以「渲染后端可替换」为前提（见 3.3），不阻塞业务层。

### 3.3 渲染后端选型

| 方案 | 透明窗口 | GPU | 实现成本 | 可测试性 | 结论 |
| --- | --- | --- | --- | --- | --- |
| **A. 分层窗口 + `UpdateLayeredWindow` + tiny-skia** | 原生逐像素 alpha | 否（CPU 光栅） | 低 | **可离线渲染 PNG 快照** | **MVP 采用** |
| B. DirectComposition + D3D11/D2D | 原生 | 是 | 高（设备丢失、交换链、COM 互操作） | 难 | M6 候选 |
| C. `Windows.UI.Composition`（`windows 0.62.2` 的 `UI_Composition`） | 原生 | 是 | 中 | 难（需桌面会话） | v0.3.3 用于 HostBackdropBrush，不替换主渲染 |
| D. WebView2 / Tauri | 需要额外合成层 | 是 | 中 | 中 | 不采用（见 3.4） |

**选 A 的理由**

1. Dock 窗口面积小（典型 900×110 px ≈ 10 万像素），全量重绘一帧在普通机器上 1–3 ms，完全够 60 FPS。
2. 逐像素 alpha 由系统完成，`WS_EX_LAYERED` + `ULW_ALPHA` 是 Win2000 起就稳定的路径，无驱动兼容问题。
3. **可以离线渲染**：同一套绘制代码输出 PNG，做快照测试与设计评审，不需要人工截图。
4. 渲染层通过 `trait RenderBackend` 隔离（输入 → 像素缓冲），替换成 B/C 时业务层零改动。

**已知代价**：CPU 光栅在「同时 40+ 图标全量放大」时帧时间会上升。缓解手段见 §11，且真实 Dock 很少超过 15 个图标。

### 3.4 为什么不用 Tauri / egui / iced

| 框架 | 不适用的原因 |
| --- | --- |
| Tauri / WebView2 | 需要常驻 Web 渲染进程，空闲内存 80 MB+；Dock 只需要画十几个图标 |
| egui | 立即模式，每帧全量重建；字体与主题定制成本高，视觉偏工具化 |
| iced | 跨平台抽象会遮蔽 `WS_EX_NOACTIVATE`、分层窗口、`WM_DROPFILES` 等 Dock 必需能力 |
| WinUI 3 / Windows App SDK（设置中心） | 官方 Fluent 控件、Mica、无障碍与 DPI 支持完善；独立进程不干扰 Dock 的零定时器消息循环 |

---

## 4. 分层与依赖规则

```
┌───────────────────────────────────────────────────────────┐
│ app (glide-dock.exe)                                        │
│  启动序列 · 消息循环 · 状态机 · 效果调度 · 组合根          │
└──────┬──────────────────┬───────────────────┬─────────────┘
       │                  │                   │
┌──────▼─────────┐ ┌──────▼────────┐ ┌────────▼──────────┐
│dock-platform-  │ │ dock-render   │ │ dock-storage      │
│windows         │ │ tiny-skia     │ │ serde_json + 迁移 │
│Win32 · COM     │ │ 布局 → 像素    │ │ 原子写 + 备份     │
└──────┬─────────┘ └──────┬────────┘ └────────┬──────────┘
       └──────────────────┴───────────────────┘
                          │
                   ┌──────▼──────┐
                   │  dock-core  │  纯数据 + 纯数学，零 OS 依赖
                   └─────────────┘
```

**依赖规则**

| 规则 | 说明 |
| --- | --- |
| `dock-core` 零依赖 | 只允许 `serde`；**禁止** `windows`、`std::process`、文件系统 |
| `dock-render` 不依赖 `windows` | 输入是 `FrameLayout` + `Pixmap` 图标，输出是 `tiny_skia::Pixmap` |
| `dock-platform-windows` 不依赖 `dock-storage` | 平台层不认识配置文件 |
| `app` 是唯一组合根 | 只有它同时知道存储、平台、渲染 |
| 禁止反向依赖 | 子 crate 不得 `use` 上层 |

`dock-render` 定义 `IconSource` trait，`dock-platform-windows` 提供实现 —— 这样「图标从哪来」对渲染层透明，也让快照测试可以注入假图标。

---

## 5. 目录结构（目标形态）

```
Glide-Dock/
├── Cargo.toml                      # workspace，workspace.dependencies 统一版本
├── .agent/                         # 设计文档（本文件、README）
├── app/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                 # 入口：日志 → DPI → 装配 → 消息循环
│       ├── app.rs                  # DockApp：状态机 + 命令处理 + 效果调度
│       ├── wndproc.rs              # 窗口过程：消息 → 命令
│       └── render_loop.rs          # 帧驱动：弹簧步进 → 布局 → 绘制
├── crates/
│   ├── dock-core/src/
│   │   ├── lib.rs
│   │   ├── geometry.rs             # Point / Size / Rect
│   │   ├── models.rs               # DockItem / DockConfig / Appearance / Behavior
│   │   ├── layout.rs               # 几何计算、放大曲线、命中测试、菜单布局
│   │   ├── animation.rs            # 弹簧积分器
│   │   ├── state.rs                # DockState / DragSession / Menu
│   │   ├── events.rs               # DockCommand / DockEvent
│   │   └── theme.rs                # 调色板：Dark / Light
│   ├── dock-storage/src/
│   │   └── lib.rs                  # ConfigStore + 版本迁移链（load / save / atomic write）
│   ├── dock-render/src/
│   │   ├── lib.rs                  # RenderBackend trait
│   │   ├── painter.rs              # 背景、图标、指示器、拖拽提示
│   │   ├── text.rs                 # 字体加载 + 字形光栅 + 混入
│   │   └── icons.rs                # IconSource trait + 内存缓存
│   └── dock-platform-windows/src/
│       ├── lib.rs
│       ├── window.rs               # 窗口类、创建、样式、位置
│       ├── surface.rs              # DIB + UpdateLayeredWindow
│       ├── shell.rs                # 快捷方式解析、ShellExecute、文件类型
│       ├── resolver.rs             # 后台 STA 快捷方式解析
│       ├── icons.rs                # IShellItemImageFactory → Pixmap
│       ├── glass.rs                # Windows.UI.Composition HostBackdropBrush 实时玻璃
│       ├── dragdrop.rs             # DragAcceptFiles / WM_DROPFILES
│       ├── monitor.rs              # 工作区、DPI 换算
│       └── error.rs
└── assets/                         # 预留：图标与字体资源
```

> 设置中心是 `settings-app/` 下的 WinUI 3 C# 应用，以独立进程运行；Dock 通过
> `--settings` 启动它，设置通过 `WM_COPYDATA` 实时同步。`dock-settings` Rust crate
> 不创建。

---

## 6. 领域模型

### 6.1 条目

```rust
pub enum ItemKind { Application, File, Folder, Separator }

pub struct DockItem {
    pub id: u32,                          // 稳定标识，跨重排不变
    pub kind: ItemKind,
    pub display_name: String,             // 默认取文件名（去扩展名）
    pub target_path: String,              // 解析后的目标：.exe / 文件夹 / 文件
    pub shortcut_path: Option<String>,    // 原始 .lnk（存在则优先用它启动）
    pub arguments: Option<String>,        // 来自 .lnk
    pub working_directory: Option<String>,// 来自 .lnk
}
```

设计要点：

- `target_path` 与 `shortcut_path` **同时保存**：前者用于展示与去重，后者保留快捷方式的完整启动语义（参数、起始目录、图标位置）。
- 图标优先从 `shortcut_path` 取（Shell 会自动应用 `.lnk` 的图标覆盖），失效时回退到 `target_path`。
- 去重键：`target_path`（不区分大小写），避免同一程序被拖入多次。

### 6.2 配置

> **v0.4.0 更新**：`version` 升到 3。v2 把数值动效字段从 `Behavior` 迁入顶层
> `motion`（见 §18）；v3 给每个 `DockItem` 增加 `launch_type`。下面 6.2 的旧快照
> 保留作历史对照，实际字段以 `models.rs` 为准。

```rust
pub struct DockConfig {
    pub version: u32,             // 当前 3
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub motion: MotionConfig,     // v2 起
    pub monitor: MonitorConfig,
    pub items: Vec<DockItem>,
    pub next_item_id: u32,
}

pub struct Appearance {
    pub icon_size: f32,       // DIP，默认 48，范围 24–128
    pub spacing: f32,         // DIP，默认 10
    pub padding: f32,         // DIP，默认 14（48 + 14×2 = 76 px 高）
    pub corner_radius: f32,   // DIP，默认 20
    pub opacity: f32,         // 0.0–1.0，默认 0.78
    pub theme: ThemeMode,     // System / Light / Dark
    pub accent: AccentColor,  // Blue / Teal / Violet / Orange / Pink
    pub position: DockPosition, // Bottom / Top / Left / Right（MVP 仅 Bottom）
    pub edge_margin: f32,     // 与屏幕工作区边缘的距离，默认 10
    pub frosted_glass: bool,  // 默认 true；不可用时回退到渐变
}

pub struct Behavior {
    pub magnification: bool,          // 默认 true
    pub magnification_factor: f32,    // 默认 1.35，v0.4 范围 1.05–3.50
    pub magnification_range: f32,     // 以「槽位」计的衰减半径，默认 2.6
    pub stiffness: f32,               // 弹簧刚度，默认 560
    pub damping: f32,                 // 阻尼比，默认 0.72
    pub auto_hide: bool,              // 默认 false（M3 生效）
    pub auto_hide_delay_ms: u32,      // 默认 450
    pub launch_animation: bool,       // 启动时图标回弹，默认 true
    pub show_tooltips: bool,          // 悬停显示名称提示，默认 true
    pub show_running_indicator: bool, // 图标下方运行圆点，默认 true
}

pub struct MonitorConfig {
    pub display_id: String,   // 空 = 主显示器（M3 起使用）
    pub offset: f32,          // 相对居中的水平偏移（DIP）
}
```

### 6.3 度量（DPI 归一）

配置是 DIP，渲染是设备像素，二者用 `Metrics` 桥接，**渲染函数内部不再做任何 DIP 换算**：

```rust
pub struct Metrics {           // 全部为设备像素
    pub dpi_scale: f32,        // GetDpiForWindow / 96
    pub icon_size: f32,
    pub spacing: f32,
    pub padding: f32,
    pub corner_radius: f32,
    pub edge_margin: f32,
}
```

### 6.4 运行期状态

```rust
pub struct DockState {
    pub cursor: Option<Point>,      // 窗口本地坐标，None = 鼠标不在窗口内
    pub hovered: Option<usize>,     // 索引，不是 id
    pub pressed: Option<u32>,       // 按下但未松手的 item id
    pub drag: Option<DragSession>,
    pub menu: Option<Menu>,
    pub running: BTreeSet<u32>,     // M3：运行中的条目 id
    pub unresolved: BTreeSet<u32>,  // 目标路径已失效的条目 id（每次加载时判定，不写盘）
}

pub struct DragSession {
    pub item_id: u32,
    pub from: usize,
    pub grab_dx: f32,        // 抓取点相对图标左边的偏移
    pub cursor: Point,
    pub insertion: usize,    // 当前插入位置（0..=len）
    pub active: bool,        // 是否已越过拖拽阈值
}

pub struct Menu {
    pub target: MenuTarget,          // Item(u32) | Background
    pub entries: Vec<MenuEntry>,
    pub hovered: Option<usize>,
    pub rect: Rect,                  // 菜单窗口本地坐标
}
```

### 6.5 命令与事件

事件驱动，**所有输入（鼠标、拖放、菜单、配置变更）统一翻译成 `DockCommand`**，再由 `DockApp` 单点处理，避免状态散落在各处：

```rust
pub enum DockCommand {
    AddPaths { paths: Vec<PathBuf>, at: Option<usize> },
    RemoveItem { id: u32 },
    MoveItem { from: usize, to: usize },
    Activate { id: u32 },
    OpenItemLocation { id: u32 },
    SetHovered(Option<usize>),
    PointerDown(Point),
    PointerUp(Point),
    PointerMove(Point),
    OpenMenu { target: MenuTarget, at: Point },
    CloseMenu,
    ReloadConfig,
    Shutdown,
}

pub enum DockEvent {
    ItemsChanged,
    LaunchFailed { id: u32, message: String },
    ConfigSaved,
    Quit,
}
```

---

## 7. 核心流程

### 7.1 启动序列

```
1. CoInitializeEx(APARTMENTTHREADED)           // Shell 需要 STA
2. SetProcessDpiAwarenessContext(PM_V2)        // 必须在建窗前
3. 初始化 tracing（控制台 + 文件）
4. ConfigStore::load()  ── 失败 → 默认配置 + 备份坏文件 + 日志
5. 计算 Metrics / 窗口尺寸（按条目数）
6. CreateWindowExW（分层窗口）+ 首次 UpdateLayeredWindow
7. 提交首批图标请求 → 图标线程
8. GetMessageW 循环（空闲时阻塞，0 CPU）
```

### 7.2 拖入添加（核心流程）

```
资源管理器拖拽
   │
   ▼ WM_DROPFILES
DragQueryFileW(全部路径) + DragQueryPoint(插入点)
   │
   ▼ DockCommand::AddPaths { paths, at }
   ▼ resolver 线程（STA）：逐个路径解析
   ├─ .lnk  → IShellLinkW：GetPath / GetArguments / GetWorkingDirectory
   │          └─ 目标不存在 → 仍建条目，标记 unresolved
   ├─ .exe  → 直接作为 target
   ├─ 目录   → kind = Folder
   └─ 其他   → kind = File
   │
   ▼ PostMessage(WM_PATHS_RESOLVED) → UI 线程
去重（target_path 忽略大小写）→ 插入 at → 图标请求 → 保存配置 → 落位动画
```

**插入点判定**：`DragQueryPoint` 给出窗口内坐标，遍历当前 `item_rects`，第一个 `point.x < rect.center().x` 的位置即插入点，否则追加到末尾。拖拽过程中该位置显示 2px 高亮竖条。

### 7.3 快捷方式解析（`.lnk`）

| 步骤 | API | 失败回退 |
| --- | --- | --- |
| 创建对象 | `CoCreateInstance(&CLSID_ShellLink)` | 退化为普通文件条目 |
| 读文件 | `IPersistFile::Load(path)` | 同上 |
| 取目标 | `IShellLinkW::GetPath` | 用 `.lnk` 自身路径显示 |
| 取参数 | `GetArguments` | `None` |
| 取目录 | `GetWorkingDirectory` | `None` |
| 解析环境变量 | `ExpandEnvironmentStringsW` | 保留原文 |

`.url` 文件不解析，按普通文件交给 Shell 启动。

### 7.4 图标获取与缓存

```
path ──► SHCreateItemFromParsingName ──► IShellItemImageFactory::GetImage(SIZE{256,256},
                                            SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK)
      ──► HBITMAP ──► GetDIBits(32bpp, BI_RGB) ──► BGRA
      ──► 预乘归一化 ──► tiny_skia::Pixmap ──► 缓存
```

**预乘归一化**：Shell 返回的位图 alpha 语义不统一。读入后扫描 `max(r,g,b)` 与 `a` 的关系：若存在 `rgb > a` 说明是**直通 alpha**，整体预乘；否则认为已是预乘，直接使用。这一步消除「边缘发黑 / 发亮」的经典问题。

**缓存**：`IconMap<u32, Arc<Pixmap>>`，容量为 `max(64, 当前条目数)`，按插入顺序淘汰；重载配置时清空。图标请求带配置代次，重载后迟到的旧结果会被丢弃。

**线程**：图标抽取跑在**单独的 STA 线程**，UI 线程只发请求、收结果（`PostMessage(WM_APP_ICON_READY)` 唤醒）。理由：网络路径、损坏 `.lnk`、慢速磁盘都可能让 `GetImage` 阻塞数百毫秒，绝不能在消息循环里做。

### 7.5 点击启动

```
.lnk  → ShellExecuteEx(.lnk, 无额外参数/目录)，保留快捷方式完整语义
.exe  → CreateProcessW("exe", 参数, 标准用户环境块, 目标父目录)
文件/文件夹/URL → ShellExecuteEx，失败时保留原条目并提示
```

`.exe` 直启避免 ShellExecuteEx 的 DDE/关联层和 Dock 当前目录污染，并用
`CreateEnvironmentBlock` 取当前用户标准环境，修复浏览器空配置、
`0xc0000142` 等问题。启动后不等待；运行状态由 `WindowTracker` 的窗口事件更新，不做进程轮询。

### 7.6 悬停放大

见 §8.2。触发源是 `WM_MOUSEMOVE` + `TrackMouseEvent(TME_LEAVE)`，不做轮询。

### 7.7 拖拽排序与拖出移除

| 阶段 | 判定 | 行为 |
| --- | --- | --- |
| 按下 | `WM_LBUTTONDOWN` 命中图标 | `pressed = Some(id)`，不改布局 |
| 进入拖拽 | 位移 > 8 DIP | `drag.active = true`，被拖图标跟随鼠标并抬升 4px |
| 拖动中 | 每次移动 | 计算 `insertion`，其余图标让位（弹簧动画） |
| 松手（窗口内） | `WM_LBUTTONUP` | `MoveItem { from, to }` → 重排后必须重建 `DockGeometry`/窗口，再保存 → 归位动画 |
| 松手（窗口外） | 松手点 `y < dock.top - 24` | `RemoveItem` → 收缩动画 |

点击与拖拽互斥：`drag.active == false` 的松手才算「点击启动」。

### 7.8 右键菜单

独立的分层小窗口（复用 `LayeredWindow`），定位在光标上方，超出屏幕时翻转。
右键在 **`WM_RBUTTONDOWN`** 即打开，不需要等松开；Dock 周围 6 DIP 也算命中区。
外观参数变化后，`relayout_window` 之后必须立即 `render()`；只 resize surface 不重绘会留下
旧帧，直到鼠标事件才刷新。玻璃宿主更新后再 `DwmFlush()`，避免 DWM 合成延迟。
注意：分层窗口用逐像素 alpha 命中测试，原生玻璃模式下 Dock 层必须在玻璃区域填
`alpha=1` 的圆角底，否则背景完全透明会整条点击穿透，背景菜单永远打不开。

菜单项：

| 目标 | 菜单项 | 命令 |
| --- | --- | --- |
| 图标 | 打开 / 打开文件位置 / 从 Dock 移除 | `Activate` / `OpenItemLocation` / `RemoveItem` |
| 背景 | 设置 / 添加开始菜单 / 添加分隔线 / 添加商店应用 / 退出 Glide-Dock | `OpenSettings` / 添加 / `Shutdown` |

关闭条件：点击菜单项、点击菜单外（60 ms 定时器轮询 `GetAsyncKeyState` + 光标位置）、
前台窗口变化、30 s 超时。**不使用** `TrackPopupMenu`——它会破坏 `WS_EX_NOACTIVATE` 的焦点语义。

### 7.9 退出

`WM_CLOSE` → 保存配置 → `DestroyWindow` → `PostQuitMessage(0)` → 循环结束 → `CoUninitialize`。

---

## 8. 布局与动画数学

### 8.1 几何

全部以**设备像素**计算，窗口左上角为原点。

```
content_w = n * icon + (n-1) * gap
dock_w    = content_w + 2 * padding
dock_h    = icon + 2 * padding
headroom  = (factor - 1) * icon          // 放大时图标向上溢出的预留高度
shadow    = 24 * dpi_scale               // 阴影外扩
window    = (dock_w + 2*shadow, dock_h + headroom + 2*shadow)
```

- 背景矩形固定：放大时 **Dock 背景不伸缩**，图标在上方的 `headroom` 透明区域里生长。
  > v0.1 实现改为「背景随图标一起轻微呼吸」：背景高度与宽度跟随最大缩放值扩展，
  > 使放大图标始终有玻璃底，详见 §17.3。
- 图标**底对齐**：`bottom = dock.bottom - padding`，`y = bottom - icon*scale`。
- 空 Dock：`content_w = icon + 16px`，居中绘制「拖入应用或快捷方式」提示。
  > v0.1 使用 286 DIP 宽的提示区，容纳两行中文提示。
- `headroom` 在 v0.1 中还包含右键菜单预留（约 250 DIP），菜单与 Dock 共用分层窗口。

### 8.2 放大曲线

以鼠标 x 与图标基线中心的距离计算，`range_px = magnification_range * (icon + gap)`：

```
t = clamp(1 - |center_i - cursor_x| / range_px, 0, 1)
w = t² (3 - 2t)                      // smoothstep，C1 连续
target_scale_i = 1 + (factor - 1) * w
```

**锚定**：取缩放值最大的图标为锚点（通常即距光标最近者），保持其**基线中心**不动，其余图标依次向两侧推开：

```
left = base_center[anchor] - width[anchor]/2 - Σ_{i<anchor}(width[i] + gap)
```

随后整体平移使其落入 `[content_left, content_right]`；窗口两侧预留的 slack 按放大半径计算，总宽超出容器时居中并夹取。

### 8.3 弹簧

每个图标一条独立弹簧，只对 `scale` 积分（位置由 scale 推导，避免两套动画不同步）：

```
a = -k (x - target) - 2ζ√k · v          k = stiffness, ζ = damping
v += a · dt ;  x += v · dt              // 半隐式欧拉，内部子步长 1/240 s
```

默认 `k = 560`、`ζ = 0.72`：`ω ≈ 23.7 rad/s`，按 1% 带宽约 230 ms、按 0.1% 位置阈值约 370 ms 收敛，带一次轻微回弹（超调约 3%）。`dt` 上限 34 ms（掉帧时不跳变）。

### 8.4 帧调度（节能的关键）

| 状态 | 定时器 | CPU |
| --- | --- | --- |
| 全部弹簧静止 | **无定时器**，`GetMessageW` 阻塞 | ≈ 0% |
| 有弹簧未收敛 | `SetTimer(hwnd, TIMER_FRAME, 16)` | 1 帧 = 步进 + 绘制 |
| 收敛判定 | 所有 `|x-target| < 0.001` 且 `|v| < 0.01` | 立即 `KillTimer` |

例外只有两处：菜单打开期间保留 60 ms 轮询（前台变化/30 s 超时即停），
启动等待最多 12 s（超时回到 `Idle`）。

绝不用 `PeekMessage` 空转，也不用固定 60 Hz 心跳。这是「空闲 CPU < 0.1%」的唯一实现方式。

---

## 9. 平台层细节

### 9.1 窗口

| 项 | 取值 | 理由 |
| --- | --- | --- |
| 类样式 | `CS_HREDRAW \| CS_VREDRAW` | 尺寸变化时重绘 |
| 窗口样式 | `WS_POPUP` | 无边框、无标题栏 |
| 扩展样式 | `WS_EX_LAYERED` | 逐像素 alpha |
| | `WS_EX_TOOLWINDOW` | 不出现在任务栏 / Alt+Tab |
| | ~~`WS_EX_TOPMOST`~~ | **不使用**；Dock 位于桌面层，普通窗口可覆盖 |
| | `WS_EX_NOACTIVATE` | **点击不抢焦点**，不打断用户当前窗口 |
| 光标 | `IDC_ARROW` | |
| 显示 | `ShowWindow(SW_SHOWNOACTIVATE)` | 首次显示也不激活 |

### 9.2 渲染管线

```
FrameLayout + Pixmap 图标
        │  tiny-skia（预乘 RGBA）
        ▼
   Pixmap(window_w × window_h)
        │  逐行 RGBA → BGRA 交换（预乘保持不变）
        ▼
   32bpp DIB 段（CreateDIBSection，复用不重建）
        │
        ▼
 UpdateLayeredWindow(hwnd, …, BLENDFUNCTION{AC_SRC_OVER, 255, AC_SRC_ALPHA}, ULW_ALPHA)
```

DIB 与内存 DC 在窗口生命周期内**只创建一次**，窗口尺寸变化时才重建 —— 避免每帧分配。

### 9.3 命中测试与点击穿透

- 应用层自己做命中（`FrameLayout.item_rects`），不依赖 Win32 命中。
- 阴影边距区域（alpha 很低但非 0）会捕获点击。`WM_NCHITTEST` 中对「既不在背景矩形、也不在任何图标矩形内」的点返回 `HTTRANSPARENT`，让点击落到下层窗口。
- `WS_EX_NOACTIVATE` 保证任何点击都不会把焦点从用户当前应用抢走。

### 9.4 DPI

| 场景 | 处理 |
| --- | --- |
| 进程声明 | `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)` |
| 获取比例 | `GetDpiForWindow(hwnd) / 96.0` |
| 变化 | 处理 `WM_DPICHANGED`：重建 `Metrics` → 重算窗口尺寸 → 重新定位 → 重建 DIB |
| 实现约束 | 所有布局常量来自 `Metrics`，**禁止**代码里出现字面量像素值 |

### 9.5 显示器与工作区

- 用 `MonitorFromWindow` + `GetMonitorInfoW().rcWork` 取**工作区**（自动避让任务栏）。
- 定位：水平居中 + `monitor.offset`，`y = rcWork.bottom - edge_margin - window_h`。
- 托盘/自动隐藏/边缘吸附在 M3；多显示器切换监听 `WM_DISPLAYCHANGE` 后重算位置。

### 9.6 拖放通道

MVP 用 `DragAcceptFiles(hwnd, true)` + `WM_DROPFILES`：

- 优点：实现量极小，稳定，支持多文件。
- 限制：拿不到拖拽过程中的视觉反馈与精确插入位置（只能在落点补动画）；`UIPI` 下低权限进程收不到高权限进程的拖放（Glide-Dock 若以管理员运行则拖放失效 —— 文档中明确「不要以管理员运行」）。
- 升级路径：M3 换成 `IDropTarget`（OLE 注册），获得拖拽悬停高亮与逐项插入预览。

### 9.7 线程模型

| 线程 | 职责 | COM |
| --- | --- | --- |
| UI 线程 | 窗口、消息循环、状态、布局、绘制 | STA（`CoInitializeEx`） |
| 图标线程 | `IShellItemImageFactory` | 独立 STA |
| 快捷方式线程 | `.lnk` 解析（`IShellLinkW`） | 独立 STA |
| 存储 | 同步小文件写入（配置通常 < 32 KB） | 不需要 |

**不做异步运行时**：MVP 不引入 tokio。线程间用 `std::sync::mpsc` + `PostMessage` 唤醒，足以覆盖全部需求，且不引入调度开销。

---

## 10. 存储与配置

### 10.1 位置

| 用途 | 路径 |
| --- | --- |
| 配置 | `%APPDATA%\Glide-Dock\config.json` |
| 日志 | `%LOCALAPPDATA%\Glide-Dock\logs\glide-dock.log` |
| 图标磁盘缓存（M3 可选） | `%LOCALAPPDATA%\Glide-Dock\icon-cache\` |
| 坏文件备份 | 同目录 `config.json.corrupt-<毫秒时间戳>.json` |

### 10.2 示例

```json
{
  "version": 1,
  "appearance": {
    "icon_size": 48.0, "spacing": 10.0, "padding": 14.0,
    "corner_radius": 20.0, "opacity": 0.78, "frosted_glass": true,
    "theme": "system", "position": "bottom", "edge_margin": 10.0
  },
  "behavior": {
    "magnification": true, "magnification_factor": 1.35,
    "magnification_range": 2.6, "stiffness": 560.0, "damping": 0.72,
    "auto_hide": false, "auto_hide_delay_ms": 450, "launch_animation": true,
    "show_tooltips": true
  },
  "monitor": { "display_id": "", "offset": 0.0 },
  "items": [
    {
      "id": 1, "kind": "application", "display_name": "Visual Studio Code",
      "target_path": "C:\\Users\\me\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
      "shortcut_path": "C:\\Users\\me\\Desktop\\Code.lnk"
    }
  ],
  "next_item_id": 2
}
```

### 10.3 写入策略

- **原子写**：写 `config.json.tmp` → `File::sync_all` → `fs::rename`（Windows 上覆盖目标）。
- **防抖**：连续变更合并，最后一次变更后 300 ms 落盘；退出时强制落盘。
- **只写字段**：`serde` 序列化时跳过 `None`，保持文件干净、可手工编辑。

### 10.4 版本迁移

| 起点 | 终点 | 迁移内容 |
| --- | --- | --- |
| （缺失） | 1 | 生成默认配置 |
| 0 | 1 | 补齐 `monitor`、`next_item_id`，`kind` 统一小写 |
| 未来 n | n+1 | 每步一个纯函数 `fn migrate_vN_to_vN1(&mut Value)`，链式执行 |

迁移**只操作 `serde_json::Value`**，不反序列化到强类型 —— 旧字段先保留、由新结构 `#[serde(default)]` 忽略，避免「迁移即丢字段」。

### 10.5 损坏恢复

| 现象 | 处理 |
| --- | --- |
| JSON 语法错误 | 备份为 `config.json.corrupt-*.json`，用默认配置启动，日志 `error!` |
| 版本高于程序 | 只读打开，拒绝写回，日志警告（防止新版本配置被旧版本覆盖） |
| version 不是整数 | 只读打开并保留原文件，不做 v0 迁移 |
| 条目字段缺失 | `#[serde(default)]` 兜底，不做整体失败 |
| 路径全部失效 | 条目保留，图标显示占位符，不自动删除 |

---

## 11. 性能预算

| 指标 | 目标 | 上限（告警线） | 测量方式 |
| --- | --- | --- | --- |
| 空闲 CPU | < 0.1% | 0.5% | 任务管理器 60 s 平均 |
| 动画 CPU | < 3% | 8% | 连续悬停 30 s |
| 内存（工作集） | < 25 MB | 40 MB | 10 图标 + 256px 图标缓存 |
| 冷启动 | < 300 ms | 600 ms | 到首帧可见 |
| 单帧耗时 | < 4 ms | 8 ms | 10 图标全放大 @160% |
| 配置保存 | < 10 ms | 30 ms | 原子写 32 KB |
| 图标首次加载 | < 80 ms | 300 ms | 单个 `.exe` |

优化手段（按优先级）：

1. **静止即停**：无动画不绘制、不设定时器（最大收益）。
2. **复用 DIB**：窗口尺寸不变就不重建缓冲。
3. **图标解码一次**：256px 母版 + 绘制时双线性缩放，换尺寸不重新解码。
4. **增量重绘**（M5，可选）：脏矩形合并，只重绘变化区域。

---

## 12. 错误处理与日志

### 12.1 错误分层

```rust
// dock-platform-windows
pub enum PlatformError { Window(String), Shell(String), Icon(String), Drop(String) }
// dock-storage
pub enum StorageError { Io { path, message }, NotAnObject, Serialize(String), UnsupportedVersion(u32) }
```

- 库 crate 用 `thiserror` 定义**具体**错误，不使用 `anyhow`。
- `app` 层统一 `tracing::error!` 记录并降级，**绝不 `unwrap` 用户可触发的路径**。
- 窗口过程整体包 `catch_unwind`：panic 记录日志后返回 `DefWindowProcW`，避免整个进程被一次偶发 panic 带走。

### 12.2 关键日志字段

| 事件 | 字段 |
| --- | --- |
| 拖入 | `path`、`kind`、`resolved`、`elapsed_ms` |
| 图标 | `path`、`source`（`shortcut`/`target`）、`size`、`hit` |
| 启动 | `id`、`exe`、`shell_result` |
| 配置 | `version`、`items`、`bytes`、`atomic` |
| 帧 | `frame_ms`、`items`、`animating`（`trace` 级，默认关闭） |

---

## 13. 测试策略

| 层次 | 方式 | 覆盖 |
| --- | --- | --- |
| `dock-core` | 单元测试 | 几何、放大曲线、锚定与夹取、弹簧收敛、命中测试、插入点计算、配置校验 |
| `dock-storage` | 单元测试 + 临时目录 | 往返序列化、缺失文件、损坏文件、旧版本迁移、只读高版本 |
| `dock-render` | **快照测试** | 同一 `FrameLayout` 渲染 PNG，断言尺寸与关键像素；人工评审图存 `preview/` |
| `app` | 冒烟测试 | 启动 → 建窗 → 退出，CI 上无窗口环境时跳过 |
| 平台层 | 手工清单 | 拖放、DPI 切换、多屏、任务栏位置、休眠唤醒 |

CI（本地 + GitHub Actions）：`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test --workspace`、`cargo build --release`。

---

## 14. 兼容性与构建

| 项 | 要求 |
| --- | --- |
| 系统 | Windows 10 1809（17763）+ / Windows 11 全版本 |
| 架构 | x64（MVP）；ARM64 待 M6 |
| 工具链 | Rust 1.85+（Edition 2024），MSVC v143，Windows SDK 10.0.19041+ |
| 运行时依赖 | 无（纯静态，除系统 DLL） |
| 权限 | **普通用户**；以管理员运行时拖放会因 UIPI 失效，需在文档提示 |
| 单实例 | `CreateMutexW("Local\\Glide-Dock.SingleInstance")`；已实现，互斥体创建失败时降级继续运行 |

---

## 15. 风险登记册

| # | 风险 | 影响 | 对策 | 阶段 |
| --- | --- | --- | --- | --- |
| R1 | 分层窗口走 CPU 光栅，图标多时帧时间上升 | 掉帧 | 图标缓存 + 静止即停 + 后期 DComp 后端 | M1/M6 |
| R2 | `windows-window`/`windows-composition` 生态与主包脱节 | 无法用 Composition | 只锁 0.62.2；渲染层 trait 隔离；跟踪上游版本对齐 | 持续 |
| R3 | Shell 图标/快捷方式解析阻塞（网络路径、损坏 lnk） | UI 卡顿 | 图标线程 + 独立 resolver 线程 | ✅ 已解决 |
| R4 | 图标位图 alpha 语义不一致 | 边缘黑边 | 预乘检测归一化（§7.4） | M1 |
| R5 | 混合 DPI 多屏 | 模糊/错位 | PMv2 + `WM_DPICHANGED` 重建 Metrics | M3 |
| R6 | 以管理员运行导致拖放失效（UIPI） | 无法拖入 | 文档声明 + 启动时检测并提示 | M2 |
| R7 | 配置文件被手改坏 | 启动失败 | 备份 + 默认值 + 只读降级 | M1 |
| R8 | 设置中心 UI 许可 | 法律风险 | 已决策：使用 Microsoft Windows App SDK / WinUI 3，保留 Microsoft 许可声明；不引入 Slint | ✅ 已解决 |
| R9 | 无代码签名，SmartScreen 拦截 | 分发受阻 | 发布阶段处理（签名证书 / 说明） | 发布前 |

---

## 16. 决策记录（ADR）

| ID | 决策 | 备选 | 结论 | 日期 |
| --- | --- | --- | --- | --- |
| ADR-001 | 内容来源仅用户拖入 | 全盘扫描 / 开始菜单扫描 | 采纳拖入 | 2026-11-08 |
| ADR-002 | 只依赖 `windows 0.62.2` | `windows-window` 0.100 家族 | 采纳 0.62.2 | 2026-11-08 |
| ADR-003 | 渲染用分层窗口 + tiny-skia | DirectComposition / WUC / WebView | 采纳分层窗口 | 2026-11-08 |
| ADR-004 | 不做皮肤系统 | Nexus 式皮肤包 | 采纳参数化外观 | 2026-11-08 |
| ADR-005 | 不引入异步运行时 | tokio | 采纳线程 + mpsc | 2026-11-08 |
| ADR-007 | 毛玻璃优先 WUC HostBackdropBrush | DWM backdrop / WinUI 3 / 纯截屏 | 采纳 WUC + 渐变兜底，不截屏 | 2026-11-14 |

---

## 17. 实现记录（v0.3.3，2026-11-14）

本节记录实际落地的代码与设计稿之间的差异。**设计稿仍是目标形态**；下面的偏差是
为在 M2 范围内交付一个可运行、可测试的闭环而做的取舍。

### 17.1 已交付范围

| 模块 | 实际内容 |
| --- | --- |
| `dock-core` | `Point/Size/Rect`、模型与 normalize、主题、`Metrics/DockGeometry/FrameLayout`、smoothstep 放大、锚定、弹簧、命中测试、插入点、菜单条目类型 |
| `dock-render` | 圆角路径、渐变、模糊阴影、Dock/空状态/图标/运行点/拖拽/提示/错误气泡、上下文菜单、字体回退、离线壁纸合成 |
| `dock-storage` | `ConfigStore`、原子写、`serde(default)` 容错、v0→v1 迁移、坏文件备份、未来版本只读 |
| `dock-platform-windows` | 分层窗口、`UpdateLayeredWindow`、工作区/DPI、ShellExecute、`.lnk` 解析、`IShellItemImageFactory` 图标线程、`WM_DROPFILES`、运行进程枚举、单实例 |
| `app` | CLI、日志、消息循环、输入状态机、菜单、拖拽、持久化、主题/DPI 响应、`--selftest`、`--render-preview` |
| 测试 | 97 个 Rust 单元测试；WinUI 设置中心手工验收；`cargo fmt`/`clippy -D warnings` 全绿 |

M2 门禁中的 7 个场景均已手工验证；G2 记录见 [PLAN.md](PLAN.md) §12。

### 17.2 实际的窗口与消息

- 只有一个顶层窗口：`Glide-Dock.DockWindow`。Dock 与右键菜单都画在同一个窗口里。
- 窗口比 Dock 大：顶部预留 `max(250 DIP, 放大 + 提示高度)`，左右预留放大溢出与阴影。
  透明区域在 `WM_NCHITTEST` 返回 `HTTRANSPARENT`，不拦截桌面点击。
- 菜单关闭靠 60 ms 定时器轮询 `GetAsyncKeyState(VK_LBUTTON/VK_RBUTTON/VK_ESCAPE)`，
  并在前台窗口变化或 30 s 超时时关闭；不依赖焦点（窗口仍是 `NOACTIVATE`）。
- 所有输入先落到 `DockApp` 的方法，再由 `dispatch(DockCommand)` 单点处理；透传消息
  交给 `DefWindowProcW`。
- 弹簧全部收敛后立即 `KillTimer(TIMER_FRAME)`；`--selftest` 断言 frame/tooltip/save/
error/running 五个定时器均为空闲，且不读写用户配置。

### 17.3 布局偏差：会「呼吸」的 Dock 背景

设计稿写的是背景矩形固定、图标在上方 `headroom` 内放大。实机验证发现：
48 px 图标 + 14 px 内边距下，1.35× 图标只比 Dock 顶边高约 16.8 px，视觉上仍被
「包」在固定条里，且被放大的图标会挤到相邻槽位外。

v0.1 改为：

```
extra_top    = (max_scale - 1) * icon
dock.top     = base.top - extra_top
dock.height  = base.height + extra_top
dock.left    = min(base.left, min_icon.left - padding)
dock.right   = max(base.right, max_icon.right + padding)
```

图标仍然底对齐；背景的最小宽度包含菜单所需空间（`440 * dpi`）。这样放大时玻璃会
向上/向两侧轻微张开，图标不会悬空。`FrameLayout::compute` 在窗口范围内对内容做
平移夹取，保证极端光标位置不越界。

### 17.4 菜单共用窗口（ADR-006）

设计稿建议独立的分层小窗。实现改为同一窗口，理由：

1. 菜单与 Dock 共用字体、调色板、阴影缓存和渲染器，避免重复加载 19 MB CJK 字体；
2. 不需要第二个 `WndProc` / 第二组 `GWLP_USERDATA` / 跨窗口命令消息；
3. 菜单锚点、DPI 变化、主题变化天然与 Dock 一致；
4. 透明外边距在 `WM_NCHITTEST` 中仍可穿透。

代价：窗口常驻面积比 Dock 大（典型 767×478 @125%），但只有 Dock/菜单/提示区域参与绘制
和命中；空闲时没有定时器、没有重绘。M6 若切换 Composition 后端，可重新评估独立菜单窗。

### 17.5 字体策略与内存

- 使用 `FontArc` 共享字形数据：Segoe UI（常规/半粗）+ Microsoft YaHei（一份）回退。
- 中文、拉丁混排按字符选择首个拥有字形的字体，并在同字体内部应用 kerning。
- 字体文件缺失时 `FontStack::empty()` 生效，文本绘制变成 no-op，不 panic、不出黑块。
- 代价：YaHei 约 19 MB，工作集高于 25 MB 目标；后续可用 GDI/DirectWrite 文本或字体
  子集化替换 `ab_glyph`，渲染接口不受影响。

### 17.6 图标提取的实机修正

- `IShellItemImageFactory::GetImage` 返回的 `HBITMAP` 不一定是 DIB section；`GetDIBits`
  必须传入屏幕 DC（`GetDC(NULL)`），否则对部分 Shell 位图返回 0 扫描行。
- 读入 32bpp BGRA 后扫描是否存在 `rgb > a`：存在则按直通 alpha 预乘，否则视为已预乘，
  避免图标边缘发黑。
- 图标线程为独立 STA，结果经 `mpsc` + `PostMessage(WM_APP + 1)` 回到 UI；失效路径与
  失败结果由 UI 侧占位图兜底。
- 尺寸固定请求 256 px 母版，绘制时按目标双线性缩放；暂未实现磁盘缓存。

### 17.7 运行状态

- `WindowTracker` 用 `SetWinEventHook` 监听顶层窗口创建/显示/隐藏/销毁；事件只投递
  一条消息，UI 侧以 80 ms 合并后再枚举可见窗口，不轮询进程。
- 精确路径匹配（忽略大小写）；仅当任一路径位于 `WindowsApps` 时才回退到文件名匹配，
  避免同名 exe 误亮。
- 圆点绘制在图标底部，带一层很淡的光晕；不改变图标位置。

### 17.8 拖动排序的取舍

- 采用「插入指示条 + 浮动图标 + 原槽位淡出」，没有实现其余图标的让位弹簧动画。
- 插入位置在移除被拖条目后的列表上计算，`MoveItem { from, to }` 的语义与设计稿一致。
- 拖出移除阈值：窗口坐标 `y < dock.top - 24 DIP`。
- 让位动画留待后续版本；当前插入条已能让用户明确看到落点。

### 17.9 存储与测试环境

- 原子写：`config.json.tmp` → `sync_all` → `rename`（Windows 上覆盖目标）。
- 损坏 JSON / 非对象根节点 / 字段类型错误都会备份为 `config.json.corrupt-<毫秒>.json`
  并使用默认配置；版本高于程序或版本不是整数时只读。
- 单元测试把临时目录放在 `target/test-data`，以兼容会拒绝 `%TEMP%` 写入的构建沙箱；
  这不改变生产路径（`%APPDATA%`）。

### 17.10 性能观察

- 空闲：帧定时器为 0，消息循环阻塞；启动时有一次进程枚举与图标请求。
- 动画：Dock 窗口全量重绘 + 阴影缓存（几何按 2 px 量化）；在 125% DPI、8 图标手工
  观察流畅，但尚未在目标硬件做 60 s CPU / 工作集 / 帧时的正式测量。
- 阴影缓存只保留最后一份，缩放动画中会多次重建；如成为瓶颈，可按宽度分桶缓存或
  改为静态阴影。

### 17.11 与设计稿的差异清单

| 设计稿 | v0.3.3 实现 | 原因 |
| --- | --- | --- |
| 菜单独立分层小窗 | 与 Dock 同窗 | 减少窗口/字体/状态重复，见 §17.4 |
| 背景固定、图标在 headroom 放大 | 背景随图标呼吸 | 视觉更自然，见 §17.3 |
| 空 Dock 宽 `icon + 16` | 286 DIP | 容纳两行中文提示 |
| 默认 opacity 0.72 | 0.78 | 浅色壁纸下图标对比度更好 |
| 拖动时其余图标让位 | 插入条 + 原槽淡出 | M2 时间盒内的取舍，见 §17.8 |
| 设置中心（WinUI 3） | 已实现（独立进程、WM_COPYDATA，Dock 唯一写者） | ✅ M4 |
| 自动隐藏 / 多屏记忆 / 热键 | 未接线 | M3 范围 |

### 17.12 下一步

1. M3：自动隐藏热区、`display_id` 显示器记忆、开机启动、全局热键、窗口切换；
2. M4 后续：设置中心补充位置/多屏/热键（当前仅底部、单屏偏移）、导入导出；
3. 性能：替换字体方案、图标磁盘缓存、目标硬件上的 60 s CPU/工作集测量；
4. 交互：拖拽让位弹簧、菜单键盘导航、托盘图标。


### 17.13 v0.2.1 修复记录（2026-11-14）

- `Waiting` 增加 12 s 超时，等待圆点受 `show_running_indicator` 控制，空闲帧定时器
  不再被托盘/无窗口应用永久占用。
- 设置中心改为只发送 `appearance`/`behavior`/`monitor`，由 Dock 作为 `config.json`
  唯一写者；Dock 校验 `WM_COPYDATA` 的发送进程与 256 KB 上限，设置端发送带 1 s
超时；相对 `--config`
  在传给设置进程前转为绝对路径。
- 失效条目的警示角标不再被“图标加载成功”清除。
- `.lnk` 解析移到独立 STA resolver 线程，拖放消息循环不再执行 COM。
- `--selftest` 使用临时配置且 `persist = false`，不写用户配置；断言五个定时器。
- 菜单在 60 ms 轮询中加入前台窗口变化与 30 s 超时；重载配置时清理 `menu_pressed`。
- 图标缓存容量为 `max(64, 条目数)`；请求带配置代次，重载后丢弃迟到结果。
- Dock 宽度按工作区收敛，放大 slack 按 `magnification_range` 计算；极端图标/屏幕
  组合不再把图标挤出窗口。
- WinEvent 只接受顶层窗口并以 80 ms 合并；运行态仅对 `WindowsApps` 回退文件名匹配。
- 单实例区分“已存在/创建失败”；关闭流程幂等，避免重复写盘。
- 配置 `version` 非整数时只读；损坏备份使用毫秒时间戳。
- 设置滑杆范围与 `normalize()` 对齐；图标按源宽高比等比绘制；字体目录取 `WINDIR`；
  `ellipsize` 对 0 宽度返回空串。
- `package.ps1` 校验 Dock 与设置进程的完整性标签、复制许可证与第三方声明、清理
  publish 目录；新增 `LICENSE-MIT`、`LICENSE-APACHE` 与 GitHub Actions CI。


### 17.14 毛玻璃背景（v0.3.3）

- 先做过 **DWM Acrylic 背景窗** 原型：独立 `WS_POPUP` + `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE)`。
  在 Windows 11 build 26300 上 API 返回成功，但普通 Win32 窗口不会真正绘制 Mica/Acrylic；
  加 `SetWindowRgn` 或 `DWMWA_WINDOW_CORNER_PREFERENCE` 还会退化成不透明灰块。因此没有采用。
- 也试了 WinUI 3 背景进程（`GlideDock.Settings.exe --backdrop` + `DesktopAcrylicBackdrop`）：
  `DesktopAcrylicController.IsSupported()` 返回 true，但窗口在本次实机会话中仍是纯色，
  Mica/Acrylic 都没有真正合成；该路径已撤回，避免多一个只显示灰块的进程。
- 最终实现走 **Windows.UI.Composition HostBackdropBrush**（`platform::glass`）：
  - `WS_EX_NOREDIRECTIONBITMAP` 的 `WS_POPUP` 窗口，当前 UI 线程创建
    `DispatcherQueueController`、`Compositor`、`ICompositorDesktopInterop::CreateDesktopWindowTarget`。
  - 根 `ContainerVisual` 下挂两个 `SpriteVisual`：底层 `CreateHostBackdropBrush()`，上层
    `CompositionColorBrush` 主题 tint；`DwmSetWindowAttribute(DWMWA_USE_HOSTBACKDROPBRUSH=17)`
    让 DWM 采样窗口背后的桌面。
  - `CompositionRoundedRectangleGeometry` + `CompositionGeometricClip` 裁出与 Dock 相同的
    20 DIP 圆角；`SetWindowPos(glass, dock)` 保证玻璃窗紧贴 Dock 下方。
  - 宿主窗口设置 `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE`，关闭 DWM 给矩形窗口
    画的白色边框；圆角完全由 composition geometry clip 决定，不会留下直边。
  - 窗口 `HTTRANSPARENT` / `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`：点击穿透、不抢焦点、
    不出现在 Alt+Tab；无第二个进程，GPU 合成，壁纸/窗口变化实时更新。
- **不截屏**：v0.2.4 起删除了 `capture_screen_rect`、`blur_rgba`、`fill_rounded_image`
  与 `backdrop` 字段；启动过程不读取屏幕内容。Composition 创建失败时直接使用主题
  半透明渐变，只有一条降级路径。
- `appearance.opacity`（0.2–1.0，默认 0.78）直接缩放玻璃根 `ContainerVisual` 的
  `Opacity`，同时保留玻璃 tint；设置中心拖动“背景不透明度”会立即调用
  `update_native_glass`，不需要重启。
- `appearance.frosted_glass`（默认 true）可关闭毛玻璃。
- 限制：HostBackdropBrush 的模糊量由 DWM 控制，不能调半径；原生玻璃的 Dock 背景
  固定在基础矩形，悬停时图标可放大到玻璃之外；不支持 HostBackdropBrush 的系统使用
  半透明渐变。

---

## 18. 动效系统（v0.4.0，2026-11-14）

### 18.1 范围

- 6 套悬停预设、4 套启动预设、Fluid Press 点击层、Wave 邻居传播、Glow/ Perspective；
- 五层独立通道：`motion::{hover, press, launch, status, compose}`；
- 设置中心动画页：预设、参数、复制动画配置；
- schema v2：新增顶层 `motion`，`Behavior` 只保留行为开关；
- schema v3：`DockItem.launch_type`（v0.4.0 启动方案，见 §19）；
- `MonitorMetrics` 增加 `refresh_hz`，帧定时器按刷新率采样。

### 18.2 与渲染后端的关系

**仍保留 `UpdateLayeredWindow` + tiny-skia 图标渲染（ADR-003）。** WUC 只用于毛玻璃。
Windows.UI.Composition 的 KeyFrameAnimation 不能作用于分层窗口的像素；整体迁移到
`DesktopWindowTarget`/交换链属于 M6。这里的 “Composition” 指五层通道合成模型：
独立属性、明确优先级、统一 `MotionSnapshot`。

### 18.3 数据结构

```rust
// DockConfig v3
pub struct DockConfig {
    pub version: u32,            // 3
    pub appearance: Appearance,
    pub behavior: Behavior,      // magnification / launch_animation / tooltips / running dot
    pub motion: MotionConfig,    // presets + max_scale / range_slots / hover_ms / elasticity
    pub monitor: MonitorConfig,
    pub items: Vec<DockItem>,    // DockItem.launch_type 持久化
    pub next_item_id: u32,
}

pub struct MotionSnapshot {
    pub layout_scales: Vec<f32>, // hover 布局基底
    pub icons: Vec<IconMotion>,  // press × launch + hover lift/tilt/glow/ripple
    pub hover_progress: f32,
    pub active: bool,
}
```

### 18.4 合成规则

- `scale = layout_scale × press_scale × launch_scale`；
- `offset = hover + press + launch`，clamp ±16 DIP；
- `glow = max(...)`；`ripple = launch > press`；
- Status 最后绘制，任何通道不能清除等待/运行点；
- 启动时间轴不可被 Hover/设置切换取消；启动结束不等于应用已启动；
- 等待/运行状态仍只由 WindowTracker 决定。

### 18.5 启动服务解耦

`app/src/launcher.rs` 的 `LauncherService` 只负责后台线程与结果队列；`MotionEngine`
只接收 `begin_launch` / `launch_failed` / `window_detected` / `window_closed`。
`activate` 先启动视觉时间轴，再请求 Launcher，两者无返回值依赖。

### 18.6 刷新率与降级

- `DwmGetCompositionTimingInfo.rateCompose` → 30–360 Hz，失败 60 Hz；
- 帧定时器 `clamp(1000/hz, 6, 16)ms`，时间轴按绝对时间采样；
- `motion.enabled=false` 或系统减少动态效果：所有通道归零，等待点静态 0.75；
- 等待最多 12s，之后 `KillTimer`；空闲无定时器。

### 18.7 Glide Zoom 连续放大场

`crates/dock-core/src/magnification/` 是全 Dock 唯一的 Apple-style 放大控制器：

- `MagnificationAnchors` 保存 `DockGeometry::slot_centers()` 的固定中心；
- `influence`：`smoothstep(clamp(1-d/R))`，`target=1+(Smax-1)w`；
- `smoothing`：`1-exp(-dt/τ)`，进入 35ms、退出 50ms，绝不使用固定帧增量；
- `controller`：每帧一次鼠标采样，所有图标统一更新；没有每图标 MouseEnter 状态；
- 鼠标离开时 target=1 但 current 保留，按 50ms 衰减；重新进入直接续接；
- 缩放由 `FrameLayout` 以 bottom-center 应用到视觉矩形，窗口和基础锚点不变；
- 其他 Hover 预设继续使用原通道；默认 `Glide Zoom` 走连续场。

这与 §18.2 的渲染后端取舍一致：连续场不依赖 Composition 视觉树，图标仍是
`UpdateLayeredWindow` 位图；没有为每个图标创建 WUC Visual 或独立时钟。

### 18.8 启动与商店应用

见 §19；`DockItem.launch_type` 在 schema v3 持久化。

---

## 19. 应用启动与商店应用（v0.4.0）

### 19.1 启动类型

```rust
pub enum LaunchType {
    Auto,         // 反序列化缺省，仅在 normalize 前存在
    Exe,          // CreateProcessW
    Shortcut,     // ShellExecuteExW
    File,         // ShellExecuteExW
    Folder,       // ShellExecuteExW
    Url,          // ShellExecuteExW
    PackagedApp,  // IApplicationActivationManager / AppsFolder fallback
}
```

`DockItem` 持久化 `launch_type`；v2 旧条目在 `DockConfig::normalize` 中按目标与
`shortcut_path` 推断。`.lnk` 添加时由 `IShellLinkW` 解析目标、参数和起始目录；目标为
EXE 时存成 `Exe`，点击直接 `CreateProcessW`，不再绕回快捷方式。

### 19.2 统一入口

`app/src/launcher.rs` 的 `LauncherService` 在后台线程调用
`dock-platform-windows::shell::launch`。所有类型都在 `launch` 内分派，不经过
`cmd.exe` / PowerShell。Shell 慢不会阻塞动画。

### 19.3 商店应用扫描

`store::scan_store_apps` 用 `FOLDERID_AppsFolder` + `IShellFolder::EnumObjects`
枚举 Start 的虚拟 AppsFolder；只保留解析名含 `!` 的 AUMID。`StoreAppService` 在
后台 STA 线程扫描，`WM_APP_STORE_APPS` 唤醒 UI；右键“添加商店应用...”用原生
`TrackPopupMenuEx` 选择。

不使用 `PackageManager.FindPackages()`：普通桌面进程会 `E_ACCESSDENIED`；
AppsFolder 只读当前用户 Start 菜单，权限稳定且天然只含可启动项。

### 19.4 Low Integrity 回退

AAM 拒绝 Low Integrity 调用方。`launch_packaged` 失败时回退到
`ShellExecuteExW("shell:AppsFolder\{AUMID}")`；打包目录中的 exe 为 Medium，
AAM 正常。详见 [`docs/LAUNCHER.md`](../docs/LAUNCHER.md)。

### 19.5 内置小组件与分隔线

`ItemKind::Widget` 使用 `target_path` 作为动作 id；目前 `start_menu` 通过
`keybd_event(VK_LWIN)` 打开系统开始菜单。图标按 `RtlGetVersion` 的 build 选择
用户提供的 icons8 彩色标志：Windows 11 `#1976D2` 四格、Windows 10 `#00B0FF`
斜切四格（源 SVG 在 `assets/start-menu/`，渲染器按 48×48 viewBox 精确复刻
path 几何）；`ItemKind::Separator`
保留 12 DIP 槽宽与标准 18 DIP 间距，但只有 1.5px 竖线，不参与放大、按压、
启动动画；`FrameLayout` 在大倍率 reflow 后仍把分隔线中心钉回基础坐标；
`DockItem::has_path` 特判保留，`normalize` 不对其去重。渲染层为 Widget 画四格
标志、为 Separator 画竖向细线；两者都不请求 Shell 图标，也不参与运行状态。
右键背景菜单提供“添加开始菜单”“添加分隔线”。

### 19.6 Dock 层级

`Appearance.layer`（`desktop` / `dynamic` / `topmost`，默认 `desktop`）：

- `desktop`：`place_above_desktop`，位于普通窗口之下；
- `topmost`：`WS_EX_TOPMOST`；
- `dynamic`：静止时桌面层，光标进入/菜单/拖动时置顶，离开后落回。

`DockApp::apply_dock_layer` 统一计算目标状态，只在状态变化时调用
`SetWindowPos`，并使用 `SWP_NOACTIVATE`，不抢焦点；玻璃窗口跟随同一层级。

### 19.7 双模式开机自启动与安装包

设置中心的常规页提供可展开的“开机自启动”卡片：

- **快速**：`schtasks.exe /Create /XML` 注册当前用户登录任务 `Glide-Dock`，
  `LogonTrigger Delay=PT0S`、`InteractiveToken`、`LeastPrivilege`、
  `Priority=4`、`StartWhenAvailable=true`；动作是 `<dock exe> --autostart`。
  创建/删除需要 UAC；
- **兼容**：写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Glide-Dock`，
  值为带引号的 exe 路径；无需管理员，但 Windows 可能延迟启动。

`StartupManager` 保证两种机制互斥：先启用新模式，成功后再清理旧模式；关闭时先删任务
再删 Run，任一步 UAC 取消/失败都会回退 UI。模式偏好保存在
`%APPDATA%\Glide-Dock\startup-mode.txt`，不进入 `config.json`。

`--autostart` 只用于任务动作：启动前最多等待 5 秒 `Progman`，避免 Explorer
尚未就绪时 `place_above_desktop` 找不到桌面锚点。Dock 启动设置时通过
`--dock-exe` 把自身路径传给设置中心。

安装包由 `installer/Glide-Dock.iss` + `scripts/build-installer.ps1` 生成：
Inno Setup 6、Program Files 安装、仅 Dock 的开始菜单/可选桌面快捷方式（设置从
Dock 右键“设置...”进入，不注册独立设置快捷方式）、静默安装支持；
卸载时停止 Dock/设置进程并删除计划任务与 HKCU Run 值，保留用户配置。

品牌资源位于 `assets/brand/`：Rust Dock 通过 `app/build.rs` + `winres` 嵌入
FullColor ICO，设置中心通过 `<ApplicationIcon>`、`AppWindow.SetIcon` 和标题栏
`Assets/Glide-Dock-FullColor-128.png` 使用同一套图标，安装包使用
`SetupIconFile` 指向同一 ICO。

### 19.8 系统托盘、守护进程与权限诊断

托盘图标由 `dock-platform-windows::tray` 注册；右键菜单由 Dock 自绘：
独立的全屏透明 layered overlay 窗口，`tiny-skia` 渲染圆角卡片、线框图标、
子菜单箭头和 canvas 子菜单。主菜单为打开设置 / 外观 / Dock 层级 / 退出；
不再包含权限诊断、重载或配置目录入口。`TaskbarCreated` 消息触发重新注册。

守护进程复用 `glide-dock.exe --guard --guard-pid <pid> --config <abs>`。Dock 启动后
spawn 隐藏 guard；guard 用 `Local\Glide-Dock.Guard` 互斥量、`OpenProcess` +
`WaitForSingleObject` 监控 Dock。退出码 0（托盘退出/正常关闭）时 guard 退出；
非 0 时 1 秒后重启，最多 5 次/60 秒。重启的子进程设置
`GLIDE_DOCK_GUARDED=1`，避免递归创建 guard。安装包卸载的
`taskkill /IM glide-dock.exe` 会同时结束两者。

权限诊断移至设置中心「诊断与维护」页；Dock 启动时不再弹管理员气球。
设置页通过 TokenElevation/TokenIntegrityLevel 说明 UIPI 或 Low Integrity，
不自动降权/提权。

设置中心新增「诊断与维护」页（`publish` 的设置进程内读取，只读）：
`DiagnosticsService` 通过 `FindWindowW("Glide-Dock.DockWindow")` 定位 Dock，
`OpenProcess` + `OpenProcessToken` 读取 Dock/设置/Explorer 的 TokenElevation 与
TokenIntegrityLevel；`OpenMutexW("Local\\Glide-Dock.Guard")` 判断守护进程；
读取 UAC 注册表、`StartupTask`/`StartupRun`、开始菜单快捷方式、AppCompatFlags
与路径。页面提供刷新、复制诊断、打开日志目录；修复按钮保留禁用，不修改系统。

---
## 附录 A：MVP 涉及的 Win32 / COM 接口

| 模块 | 接口 |
| --- | --- |
| 窗口 | `RegisterClassW`、`CreateWindowExW`、`ShowWindow`、`SetWindowPos`、`DestroyWindow`、`DefWindowProcW` |
| 分层 | `CreateDIBSection`、`CreateCompatibleDC`、`SelectObject`、`UpdateLayeredWindow` |
| 消息 | `GetMessageW`、`TranslateMessage`、`DispatchMessageW`、`PostQuitMessage`、`SetTimer` / `KillTimer`、`TrackMouseEvent` |
| DPI | `SetProcessDpiAwarenessContext`、`GetDpiForWindow` |
| 显示器 | `MonitorFromWindow`、`GetMonitorInfoW` |
| Shell | `SHCreateItemFromParsingName`、`IShellItemImageFactory::GetImage`、`IShellLinkW`、`IPersistFile`、`ShellExecuteExW`、`CreateProcessW`、`CreateEnvironmentBlock`、`DragAcceptFiles`、`DragQueryFileW`、`DragQueryPoint`、`DragFinish` |
| 图形 | `GetDIBits`、`GetDC`、`ReleaseDC` |
| Composition | `RoInitialize`、`CreateDispatcherQueueController`、`Compositor`、`ICompositorDesktopInterop::CreateDesktopWindowTarget`、`CreateHostBackdropBrush`、`CompositionRoundedRectangleGeometry`、`CompositionGeometricClip`、`DwmSetWindowAttribute(DWMWA_USE_HOSTBACKDROPBRUSH)` |
| COM | `CoInitializeEx`、`CoCreateInstance`、`CoUninitialize` |

## 附录 B：术语

| 术语 | 含义 |
| --- | --- |
| DIP | Device Independent Pixel，与 96 DPI 对齐的逻辑像素 |
| Metrics | 由 DIP + 当前 DPI 换算出的一整套设备像素常量 |
| headroom | 为放大动画预留的窗口顶部透明高度 |
| 锚点 | 放大时位置保持不动的那个图标（缩放值最大，通常即距光标最近者） |
| 配置代次 | 每次 reload 递增，用于丢弃过期的图标/解析结果 |

---

## 附录 C：任务分解

M1–M6 的逐条任务卡、依赖关系、工时估算、门禁清单与验证命令，统一维护在 **[PLAN.md](PLAN.md)**。本文件不再重复，避免两处描述不一致。

| 里程碑 | 任务卡 | 位置 |
| --- | --- | --- |
| M1 首帧 | T1.1–T1.8 | PLAN §3 |
| M2 MVP 闭环 | T2.1–T2.10 | PLAN §4 |
| M3 桌面级体验 | T3.1–T3.7 | PLAN §5 |
| M4–M6 | 概要级 | PLAN §6 |

# Glide-Dock 实施计划

> 版本 **1.1** · 2026-11-08
> 上游文档：[README.md](README.md)（产品定位与里程碑）、[ARCHITECTURE.md](ARCHITECTURE.md)（架构、流程与数学）
> 本文件是**施工单**：按什么顺序、动哪些文件、怎么证明做完了。三者冲突时以 ARCHITECTURE 为准，本文件负责排序与验收。
> **v0.3.3 交付状态**：M0 ✅ · M1 ✅ · M2 ✅（G2 通过） · M3 🟡 部分 · M4 ✅ 设置中心与启动动画已落地 · M5–M6 ⬜ 待开始。
> 实际交付、偏差与验证记录见 §12。

---

## 0. 总览

| 阶段 | 目标 | 工期 | 产出物 | 门禁 |
| --- | --- | --- | --- | --- |
| **M0** | 方案定稿 | — | README / ARCHITECTURE / 本文件 | ✅ 已定稿 |
| **M1** | 首帧：窗口能出现、能上屏、能退出 | 5 天 | 可运行的 `glide-dock.exe`（空 Dock） | **G1** |
| **M2** | MVP 闭环：拖入 → 显示 → 启动 → 排序 → 保存 | 9 天 | **v0.1** 可用版本 | **G2** |
| **M3** | 桌面级体验：自隐藏、多屏、DPI、单实例、运行状态 | 6 天 | v0.2 | G3 |
| **M4** | 设置中心（WinUI 3） | 6 天 | v0.3 | G4 |
| **M5** | 任务管理 + 桌面小组件 | 8 天 | v0.4 | G5 |
| **M6** | 渲染升级与发布 | 5 天 | v1.0 | G6 |

单人全职口径合计 **39 个工作日**。M4 与 M5 之间无强依赖，可与 M3 的收尾并行。

**关键路径**：`T1.4 分层表面 → T1.5 背景绘制 → T2.2 布局与弹簧 → T2.3 图标绘制 → T2.5 拖入 → T2.9 持久化`。这条链上任何一环延期都会顺延 v0.1。

**范围守卫**：M2 结束前不碰自动隐藏、多屏、设置界面、运行状态。任何"顺手加一下"的需求记入 §9 Backlog，不进入当期。

> 施工单中的 T1.x / T2.x 仍保留原始拆分；实际完成情况、验收证据与偏差见 §12。
> M3 只交付了 DPI、单实例与运行状态圆点；自动隐藏、热键、多屏记忆仍未接线。

---

## 1. 工作方法

### 1.1 每个任务的完成定义（DoD）

一个任务只有同时满足以下 6 条才算完成，缺一条即视为未完成：

1. `cargo fmt --all -- --check` 通过
2. `cargo clippy --workspace --all-targets -- -D warnings` 零告警
3. `cargo test --workspace` 全绿，新增逻辑有对应单测
4. 新增的公开 API 有文档注释；新增的错误路径有 `tracing` 日志
5. 用户可触发的路径上不出现 `unwrap()` / `expect()` / `panic!()`
6. 本文件对应任务卡勾选，且「验证」列的命令手工跑过一遍

### 1.2 提交节奏

- 主干开发 + 短分支（`feat/t1-4-surface`），单次提交聚焦一个任务卡。
- 每个提交必须能独立编译通过，不允许"半截代码入库等下次补"。
- 里程碑达成打 tag：`v0.0.1-m1`、`v0.1.0-mvp`，tag 前必须过对应门禁。
- `Cargo.lock` 入库并锁定，依赖升级单独提交、单独验证。

### 1.3 每轮对话的工作方式

1. 先跑一遍当前门禁命令，确认基线是绿的；
2. 只做一个任务卡（或一个卡内的独立子项）；
3. 写代码 → 编译 → 跑测试 → 跑该卡的验证命令；
4. 汇报：改了什么、验证结果、下一张卡是什么；
5. 遇到设计文档没覆盖的问题，**先改文档再改代码**，避免文档与实现漂移。

---

## 2. 门禁（Gates）

门禁是可执行的验收脚本 + 一张手工清单。**不过门禁不进下一阶段。**

### G0 环境（已完成）

```powershell
rustc --version            # 1.99.1 stable-x86_64-pc-windows-msvc
cargo --version            # 1.99.1
# MSVC: VS 18 Community + Windows SDK
```

### G1 首帧（M1 出口）

```powershell
cargo run -p glide-dock -- --selftest          # 建窗 → 渲染 1 帧 → 自动退出，退出码 0
cargo run -p glide-dock -- --render-preview preview
```

| 检查项 | 通过标准 |
| --- | --- |
| 窗口出现 | 屏幕底部居中出现圆角半透明 Dock，无标题栏、无任务栏图标 |
| 上屏正确 | 预览 PNG 与屏幕肉眼一致；圆角外为全透明 |
| 点击穿透 | 在 Dock 外 2 px 处点击能选中桌面图标 |
| 不抢焦点 | 点击 Dock 时，当前活动窗口标题栏不变化 |
| 空闲开销 | 静置 60 s，CPU 平均 < 0.5%，工作集 < 25 MB |
| 依赖健康 | `cargo tree -d` 中不出现两份 `windows-core` |

### G2 MVP（M2 出口，v0.1）

| # | 场景 | 通过标准 |
| --- | --- | --- |
| 1 | 拖入 5 个应用（含 `.lnk` 与 `.exe`） | 全部出现，图标清晰，顺序与拖入顺序一致 |
| 2 | 重启程序 | 5 个条目仍在，顺序不变 |
| 3 | 点击图标 | 对应程序启动；失败时有抖动反馈与日志 |
| 4 | 悬停快速划过 | 放大平滑、无跳变、无残留 |
| 5 | 拖动排序后重启 | 顺序持久 |
| 6 | 手工改坏 `config.json` 再启动 | 自动备份坏文件、以默认配置启动、不崩溃 |
| 7 | 拖到窗口外松手 | 条目移除并落盘 |

### G3–G6

分别在 M3–M6 结束时定义，纳入同一张表维护；M3 起追加自动化冒烟：`--selftest` 覆盖窗口重建、DPI 切换模拟、配置迁移。

---

## 3. M1 任务卡（首帧，5 天）

### T1.1 Workspace 骨架 · 2 h

| 项 | 内容 |
| --- | --- |
| 依赖 | 无 |
| 改动 | `Cargo.toml`、`app/Cargo.toml`、`crates/*/Cargo.toml`、各 `lib.rs`、`app/src/main.rs` |
| 要点 | 4 个成员 crate + 1 个 bin；`[workspace.dependencies]` 统一锁版本；`resolver = "3"`；Edition 2024；release 开 `lto = "thin"` |
| DoD | `cargo check --workspace` 通过；`main` 打印版本后返回 0 |
| 验证 | `cargo check --workspace`、`cargo tree -d` |

### T1.2 核心模型与几何 · 6 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.1 |
| 改动 | `dock-core/src/{geometry,models,theme,layout,animation,state,events}.rs` |
| 要点 | `Point/Size/Rect`；`DockItem/DockConfig/Appearance/Behavior/MonitorConfig`；`Metrics` DPI 换算；`Palette` 深浅两套；`DockCommand`/`DockEvent` 枚举先定型 |
| DoD | 序列化往返、默认值补齐、字段缺失容错、`normalize()` 修 id 与去重均有单测 |
| 验证 | `cargo test -p dock-core` |

> 提示：本卡只建模型与纯函数骨架，**放大曲线与弹簧放到 T2.2**，避免过早优化。

### T1.3 窗口、DPI 与定位 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.2 |
| 改动 | `dock-platform-windows/src/{window,dpi,monitor,error}.rs` |
| 要点 | `CoInitializeEx(STA)`；`SetProcessDpiAwarenessContext(PM_V2)` 必须在建窗前；`WS_POPUP` + `WS_EX_LAYERED\|TOOLWINDOW\|NOACTIVATE`（**不再使用 TOPMOST**，桌面层）；`ShowWindow(SW_SHOWNOACTIVATE)`；用 `MonitorFromWindow`+`rcWork` 底部居中 |
| DoD | 窗口出现且不抢焦点；125%/150% 缩放下窗口尺寸成比例；日志记录 DPI 与工作区 |
| 验证 | `--selftest window`；手工切换显示缩放复测 |

### T1.4 分层表面 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.3 |
| 改动 | `dock-platform-windows/src/surface.rs` |
| 要点 | `CreateDIBSection` 32bpp 顶朝下 + 复用内存 DC；`tiny_skia::Pixmap` → `BGRA` 逐行交换；`BLENDFUNCTION{AC_SRC_OVER, 255, AC_SRC_ALPHA}` + `UpdateLayeredWindow`；尺寸变化才重建缓冲 |
| DoD | 能上屏任意带 alpha 的图；连续 600 帧无泄漏（句柄数稳定）；单帧拷贝 < 1 ms |
| 验证 | `--selftest surface`（上屏棋盘格后自检像素） |

### T1.5 背景绘制与预览 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.4 |
| 改动 | `dock-render/src/{lib,painter}.rs`、`app/src/render_loop.rs` |
| 要点 | 圆角路径 + 垂直渐变（深色 #3A3A42→#18181D，透明度 0.78）；1 px 顶部高光描边；可缓存的模糊阴影；`--render-preview <dir>` 输出 PNG |
| DoD | 预览图圆角干净无锯齿、阴影自然；`cargo run -- --render-preview` 产出可直接评审的 PNG |
| 验证 | 生成 `preview/01-dock-dark.png` 与 `02-dock-light.png`，人工评审 |

### T1.6 文本光栅 · 5 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.2 |
| 改动 | `dock-render/src/text.rs` |
| 要点 | `ab_glyph` 加载 `C:\Windows\Fonts\msyh.ttc`（TTC 用 `try_from_vec_and_index`），失败回退 `segoeui.ttf`，再失败则禁用文本；字形掩码混入 `Pixmap`；只做拉丁 + 基础排版，不做复杂整形 |
| DoD | 空状态提示「拖入应用或快捷方式」正确渲染；两套字体都缺失时不 panic、不出黑块 |
| 验证 | 快照 PNG 目视；`cargo test -p dock-render` 覆盖"无字体"分支 |

### T1.7 消息循环、帧调度与自检开关 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.5 |
| 改动 | `app/src/{main,app,wndproc}.rs` |
| 要点 | `GetMessageW` 阻塞循环；`WndProc` 经 `GWLP_USERDATA` 取 `Box<DockApp>`；`WM_TIMER` 仅在有动画时存在；`catch_unwind` 兜底；CLI：`--selftest`、`--render-preview`、默认运行 |
| DoD | 静置无定时器（自测断言 timer 未创建）；`--selftest` 退出码 0；Ctrl+C / `WM_CLOSE` 干净退出 |
| 验证 | `--selftest` + 任务管理器观察 60 s CPU |

### T1.8 门禁 G1 · 4 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.1–T1.7 |
| 改动 | 修 bug、补文档、打 tag |
| DoD | G1 六项全过；`cargo clippy -D warnings` 零告警；仓库打 `v0.0.1-m1` |
| 验证 | 完整跑一遍 §2 G1 |

**M1 累计：约 49 h ≈ 5 个工作日。**

---

## 4. M2 任务卡（MVP 闭环，9 天）

### T2.1 配置存储 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.2 |
| 改动 | `dock-storage/src/lib.rs`（迁移链在文件内） |
| 要点 | `%APPDATA%\Glide-Dock\config.json`（`APPDATA` 环境变量）；临时文件 + `File::sync_all` + `fs::rename`；300 ms 防抖；版本迁移链只操作 `serde_json::Value`；损坏文件备份为 `config.json.corrupt-<ms>.json`；高版本/非法版本只读 |
| DoD | 往返、缺文件、坏 JSON、高版本、v0→v1 迁移五类单测全绿；写 32 KB < 10 ms |
| 验证 | `cargo test -p dock-storage`；手工删改 `config.json` 复测 |

### T2.2 布局、放大与弹簧 · 10 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T1.2 |
| 改动 | `dock-core/src/{layout,animation}.rs` |
| 要点 | `geometry()` 算窗口/dock/基线中心；`target_scales()` smoothstep 衰减；锚点保持 + 边界夹取；`Spring` 半隐式欧拉、子步长 1/240、`dt` 上限 34 ms；`hit_test()` |
| DoD | 单测覆盖：0/1/2/15 个条目；光标在极端位置不越界；弹簧 400 次迭代内收敛且无 NaN；命中边界半开区间正确 |
| 验证 | `cargo test -p dock-core`；`--render-preview --cursor 300` 输出放大态快照 |

### T2.3 图标绘制与 `IconSource` · 6 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.2 |
| 改动 | `dock-render/src/{painter,icons}.rs` |
| 要点 | `IconSource` trait；`draw_pixmap` 双线性缩放到目标矩形；底对齐；运行指示点预留绘制位；`--render-preview` 支持注入假图标（生成 8 个彩色方块） |
| DoD | 缩放后图标无锯齿毛边；快照 PNG 中 8 个图标排布正确、间距均匀 |
| 验证 | `preview/06-menu-closeup.png` 人工评审 |

### T2.4 图标提取线程与缓存 · 10 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.3 |
| 改动 | `dock-platform-windows/src/icons.rs`、`app/src/app.rs`（请求/响应接线） |
| 要点 | 独立 STA 线程 + `mpsc`；`SHCreateItemFromParsingName` → `IShellItemImageFactory::GetImage(256)`；`GetDIBits` 32bpp；**预乘归一化**（检测 `rgb > a`）；容量 `max(64, 条目数)` 的内存缓存；请求带配置代次；完成后 `PostMessage(WM_APP_ICON_READY)` |
| DoD | 20 个桌面快捷方式图标全部正确；网络路径 `.lnk` 不卡 UI；失败有占位图标与日志 |
| 验证 | `--selftest icons <path>` 导出提取结果 PNG |

### T2.5 拖放接收与条目构造 · 10 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.1 |
| 改动 | `dock-platform-windows/src/{dragdrop,shell}.rs`、`app/src/app.rs` |
| 要点 | `DragAcceptFiles` + `WM_DROPFILES` 多文件；`DragQueryPoint` 定插入点；`.lnk` 用 `IShellLinkW` 取目标/参数/起始目录并展开环境变量；按 `target_path` 忽略大小写去重；失效目标仍建条目 |
| DoD | 一次拖 3 个文件插入位置正确；同名重复拖入只保留一条；失效 `.lnk` 有条目和占位图标 |
| 验证 | 手工清单：拖 `.lnk`/`.exe`/文件夹/普通文件各一次 |

### T2.6 点击启动与失败反馈 · 5 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.5 |
| 改动 | `app/src/app.rs`、`dock-platform-windows/src/shell.rs` |
| 要点 | 优先 `ShellExecuteW` 原始 `.lnk`，否则目标 + 参数 + 工作目录；返回值 ≤ 32 判失败；失败触发抖动动画 + `error!` 日志 |
| DoD | 5 个条目点击全部能启动；构造一个不存在路径，点击后不崩溃且有反馈 |
| 验证 | 手工 + `--selftest launch`（对 `notepad.exe` 启动后立即结束进程） |

### T2.7 拖拽排序与拖出移除 · 10 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.5、T2.2 |
| 改动 | `dock-core/src/state.rs`、`app/src/app.rs`、`dock-render/src/painter.rs` |
| 要点 | 8 DIP 阈值进入拖拽；被拖图标抬升 4 px 跟随；插入点竖条；松手窗口内 `MoveItem`、窗口外（`y < dock.top - 24`）`RemoveItem`；点击与拖拽互斥 |
| DoD | 10 次随机拖拽排序 100% 准确；误触（抖动 3 px）仍判定为点击 |
| 验证 | 手工压力测试 + 单测覆盖插入点计算 |

### T2.8 右键菜单窗口 · 10 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.1、T1.6 |
| 改动 | `dock-platform-windows/src/window.rs`（复用）、`dock-render/src/painter.rs`、`app/src/app.rs` |
| 要点 | 与 Dock 同窗；`ab_glyph` 文本；定位光标上方、越界夹取；关闭靠 60 ms 轮询 `GetAsyncKeyState`、前台窗口变化或 30 s 超时 |
| DoD | 菜单不抢焦点；点击外部 200 ms 内关闭；「退出」能走完整保存+销毁流程 |
| 验证 | 手工清单 + `--selftest menu`（模拟打开/关闭，断言状态机） |

### T2.9 状态接线与持久化 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.1、T2.5、T2.7、T2.8 |
| 改动 | `app/src/app.rs`、`dock-core/src/events.rs` |
| 要点 | 所有输入 → `DockCommand` → 单点处理 → 落盘（防抖）+ 请求重绘；启动时加载并 `normalize()`；退出强制落盘 |
| DoD | 任一变更 300 ms 内落盘；连续 20 次快速拖动只写一次盘 |
| 验证 | 观察日志写入次数；断电模拟（写前 `taskkill /f`）后配置可读 |

### T2.10 门禁 G2 与 v0.1 · 8 h

| 项 | 内容 |
| --- | --- |
| 依赖 | T2.1–T2.9 |
| DoD | G2 七项全过；README/ARCHITECTURE 同步实际行为；打 `v0.1.0-mvp` |
| 验证 | 完整跑 §2 G2 + 一次全新用户路径（删配置 → 全新开始） |

**M2 累计：约 85 h ≈ 9 个工作日。**

---

## 5. M3 概要（桌面级体验，6 天）

| 卡 | 内容 | 关键点 | 验收 |
| --- | --- | --- | --- |
| T3.1 | 自动隐藏 | 鼠标离开延迟隐藏；屏幕边缘 2 px 热区唤出；动画复用弹簧 | 全屏应用下不遮挡；唤出 < 120 ms |
| T3.2 | 多显示器 | 记忆 `display_id`；`WM_DISPLAYCHANGE` 重定位；显示器拔插回到主屏 | 双屏拔插后位置正确 |
| T3.3 | DPI 完整适配 | `WM_DPICHANGED` 重建 `Metrics` + DIB + 图标尺寸 | 混合 DPI 拖动窗口不模糊 |
| T3.4 | 单实例 | `CreateMutexW` + 二次启动唤起已有实例 | 双击两次只出现一个 Dock |
| T3.5 | 开机启动 | `HKCU\...\Run` 写入/删除，路径带引号 | 重启后自动出现 |
| T3.6 | 全局热键 | `RegisterHotKey`（默认 `Ctrl+Alt+D`）显示/隐藏 | 与其他软件冲突时可改 |
| T3.7 | 运行状态圆点 | `SetWinEventHook` 顶层窗口事件 + 80 ms 合并刷新 | 图标下圆点准确；空闲 CPU 仍 < 0.5% |

---

## 6. M4–M6 概要

**M4 设置中心（已提前完成）**：`settings-app/` 下的 WinUI 3 C# 应用，独立进程，
左侧导航 + 卡片式设置，覆盖主题、五种强调色、尺寸/间距/内边距/圆角/不透明度、
边缘距离、水平偏移、放大倍数/范围、弹簧刚度/阻尼、提示/启动回弹/运行圆点开关、
搜索、恢复默认、打开配置目录与重新加载。改动通过 `WM_COPYDATA` 实时同步；Dock
是 `config.json` 唯一写者，300 ms 防抖落盘。Dock 仍是单一分层窗口；设置窗口是普通应用层级。

**M5 任务管理与小组件（8 天）**：窗口分组（同 AUMID/路径合并）、点击切换/最小化/恢复、多窗口选择菜单；小组件规范（`Widget` trait + 统一刷新节拍）先落时钟与 CPU/内存两个。

**M6 渲染升级与发布（5 天）**：评估 DirectComposition 后端（`RenderBackend` trait 替换实验）、ARM64 交叉编译、签名与安装包、崩溃日志上报开关。

---

## 7. 验证矩阵

| 层次 | 自动化 | 手工 | 频率 |
| --- | --- | --- | --- |
| 领域逻辑 | `cargo test -p dock-core` | — | 每次提交 |
| 存储 | `cargo test -p dock-storage` | 改坏配置复测 | 每次提交 |
| 渲染 | 快照 PNG 尺寸/像素断言 | 评审 `preview/*.png` | 每次提交 |
| 窗口与上屏 | `--selftest` | 点击穿透、不抢焦点 | 每张卡 |
| 平台交互 | 部分 `--selftest` | 拖放、DPI、多屏、菜单 | 每里程碑 |
| 性能 | `--selftest perf` 记录 CPU/内存/帧时 | 任务管理器复核 | 每里程碑 |

**预览图规范**：每次涉及视觉的改动都要更新 `preview/` 下的快照，并在提交说明里附路径。评审只看图，不看代码。

---

## 8. 风险与回退

| # | 计划层风险 | 触发信号 | 回退动作 |
| --- | --- | --- | --- |
| P1 | `windows` 首次编译时间长，拖慢迭代 | 单次全量 > 3 min | 只 `cargo check -p <目标 crate>`；`[profile.dev.package."*"] opt-level = 2` 已配置 |
| P2 | crate 版本漂移导致 API 变更 | `cargo update` 后编译失败 | `Cargo.lock` 入库锁死；升级单独提交并跑全门禁 |
| P3 | 图标 alpha/预乘处理仍出黑边 | 预览图边缘发黑 | 退回"仅支持带 alpha 的 256px 位图 + 占位图兜底"，把归一化挪到 M3 |
| P4 | 拖放受 UIPI 限制（管理员运行） | 拖入无反应 | 启动检测管理员令牌并弹日志提示；文档已声明 |
| P5 | 视觉评审反复（主观项） | 同一处改 3 次以上 | 先冻结参数表（README §视觉与默认参数），改动只走配置不走代码 |
| P6 | 手工测试（多屏/DPI）缺设备 | 无法复现 | 用 `--render-preview --dpi 1.25/1.5` 离线覆盖布局分支，真机项延后到有设备时补 |
| P7 | 范围蔓延（"顺手加个功能"） | 当期任务卡外的新需求 | 一律记 §9 Backlog，不插队 |

架构层风险（R1–R9）见 [ARCHITECTURE.md](ARCHITECTURE.md) §15。

---

## 9. Backlog（明确不入当期）

- 文件夹栈（点击展开网格）
- 最近使用排序 / 使用频率加权
- 拖入时的实时插入预览（`IDropTarget`）
- 图标磁盘缓存与预加载
- 多 Dock（每屏一个）
- 配置导入导出与云同步
- 高对比度 / 讲述人支持
- 像素级阴影（引入模糊滤镜）

---

## 10. 变更控制

1. 任何影响 ARCHITECTURE 的改动（新增 crate、换依赖、改数据模型）→ 先提 ADR，再改文档，最后改代码。
2. 任何影响里程碑范围的改动 → 更新本文件 §0 总览与对应任务卡，并重估工期。
3. 本文件是活的：每完成一张卡就勾掉，偏差超过 20% 时重排后续排期。

---

## 11. 当前进度

| 阶段 | 状态 | 备注 |
| --- | --- | --- |
| M0 | ✅ 定稿 | 三分文档就位，已按 v0.1 实现回写 |
| M1 | ✅ 已完成 | workspace、core、分层窗口、surface、背景/文本、消息循环、`--selftest` |
| M2 | ✅ 已完成 | v0.1 闭环：拖入、图标、启动、放大、排序、拖出、菜单、持久化、自愈 |
| M3 | 🟡 部分完成 | DPI / 单实例 / 运行圆点 / 双模式开机启动已接线；自动隐藏、多屏记忆、全局热键未做 |
| M4 | ✅ 设置中心已完成 | WinUI 3 独立进程、实时同步、Dock 唯一写者、桌面层/普通窗口层级 |
| M5–M6 | ⬜ 待开始 | 任务管理/小组件、渲染升级 |

**下一步动作**：

1. M3 收尾：自动隐藏热区、`display_id` 多屏记忆、`RegisterHotKey`；
2. 拖拽让位弹簧与菜单键盘导航；
3. 设置中心后续：位置/多屏/热键、导入导出、UI 缩放；
4. 字体方案与图标磁盘缓存，用于压工作集。

---

## 12. 交付记录（v0.2.0，2026-11-08；v0.3.3 修复见 §13）

### 12.1 实际产出

| 位置 | 内容 |
| --- | --- |
| `app/src/main.rs` | CLI（`--demo` / `--selftest` / `--render-preview` / `--config`）、日志、DPI aware、单实例、消息循环 |
| `app/src/app.rs` | `DockApp`：输入 → `DockCommand` → 状态 → 布局 → 绘制；菜单、拖拽、持久化、主题/DPI |
| `app/src/wndproc.rs` | 窗口过程 + `catch_unwind` 兜底 |
| `app/src/preview.rs` | 真实系统图标 + 离线壁纸的预览生成 |
| `crates/dock-core` | 模型、几何、布局、弹簧、主题、状态/命令、菜单条目 |
| `crates/dock-render` | tiny-skia 绘制、模糊阴影、Segoe UI + YaHei 文本、图标占位、菜单/提示/错误 |
| `crates/dock-storage` | 原子写、迁移、坏文件备份、未来版本只读 |
| `crates/dock-platform-windows` | 分层窗口、DIB surface、Shell/快捷方式、图标 STA 线程、拖放、工作区/DPI、进程枚举、单实例 |
| `preview/` | 10 张评审图与画廊说明 |

### 12.2 门禁结果

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | ✅ 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 零告警 |
| `cargo test --workspace` | ✅ 97 个单元测试通过（core 51 / render 20 / platform 14 / storage 9 / app 3） |
| `cargo run -p glide-dock -- --selftest` | ✅ 退出码 0；断言空闲时 `frame_timer == false` |
| `cargo run -p glide-dock -- --render-preview preview` | ✅ 生成 01–10 Dock 预览图；11–12 为设置中心实机截图 |
| 依赖健康（`cargo tree -d`） | ✅ 只有一份 `windows-core 0.62.2` |
| G2 场景 1：拖入 `.lnk` / `.exe` | ✅ 实机拖入 `CC Switch.lnk`，解析目标并提取图标 |
| G2 场景 2：重启后条目仍在 | ✅ 配置写入 `preview/demo-config.json`，重启恢复 |
| G2 场景 3：点击启动 | ✅ Explorer / Edge / Windows Terminal / Notepad 均成功拉起；失败路径有提示 |
| G2 场景 4：悬停放大 | ✅ 125% DPI 实机验证，弹簧平滑 |
| G2 场景 5：排序持久化 | ✅ Edge 从索引 1 拖到索引 4，JSON 顺序更新 |
| G2 场景 6：坏配置自愈 | ✅ 单元测试覆盖语法错误、非对象根、字段类型错误、未来版本 |
| G2 场景 7：拖出移除 | ✅ 记事本拖出后条目与配置同步移除 |

### 12.3 实际测试环境

- Windows 11，2560×1440 物理分辨率，125% 缩放；
- Rust 1.99.1 MSVC；
- 屏幕截图为 DPI-aware 抓取（2560×1440），验证 layered window 的逐像素 alpha；
- `--demo` 配置写在 `preview/demo-config.json`，不污染用户 `%APPDATA%`。

### 12.4 与任务卡的偏差

| 任务卡 | 偏差 | 处理 |
| --- | --- | --- |
| T2.7 拖动排序 | 未实现其余图标让位弹簧；使用插入指示条 + 浮动图标 + 原槽淡出 | 记入后续 |
| T2.8 菜单 | 菜单与 Dock 共用分层窗口，非独立小窗 | 见 ARCHITECTURE §17.4；收益大于成本 |
| T2.4 图标缓存 | 内存缓存容量 `max(64, 条目数)`，无磁盘缓存 | 记入 M3/M6 |
| T2.10 G2 | 7 个场景全部手工通过；无自动化 UI 测试 | 保留手工清单 |
| M3 | 仅 DPI / 单实例 / 运行圆点 | 其余顺延 |
| M4 | ✅ 已完成 | 改用 WinUI 3，不引入 Slint；许可证风险解除 |

### 12.5 范围外但已顺手验证

- 点击穿透（`WM_NCHITTEST → HTTRANSPARENT`）通过 `SendMessage` 断言；
- 菜单外部点击 / Escape 关闭；
- 主题注册表读取（`AppsUseLightTheme`）；
- 单实例互斥体；
- 窗口不抢焦点（`WS_EX_NOACTIVATE`）；
- 125% DPI 下窗口、阴影与文字重排；
- 设置中心：浅色/深色、主题切换、七项外观滑杆、行为开关、动画预设、搜索、
  恢复默认、配置落盘、`WM_COPYDATA` 实时同步、设置进程生命周期；
- 设置窗口整条标题栏空白区可拖动；位置保存到配置旁 `settings-window.txt`，
  重启后恢复到同一位置（实机验证 702,237 → 702,237）；
- 层级：Dock 与设置窗口均无 `WS_EX_TOPMOST`（`GetWindowLongPtr` 验证）；
  设置窗口 `WS_EX_WINDOWEDGE` 普通应用层；
- 启动修复：Explorer 通过 `CreateProcessW` 直启成功；`.lnk` 不再重复叠加参数；
  `.exe` 使用 `CreateEnvironmentBlock` 标准用户环境，避免空浏览器配置与
  `0xc0000142`；
- 环境隔离验证：父进程 `%APPDATA%` 指向工作区时，启动的 `cmd.exe` 仍读取
  `C:\Users\Administrator\AppData\Roaming`。

---

## 13. v0.3.3 修复与验证（2026-11-14）

详细变更见 [ARCHITECTURE.md](ARCHITECTURE.md) §17.13。验证命令与结果：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p glide-dock -- --selftest
cargo build --release -p glide-dock
```

| 检查 | 结果 |
| --- | --- |
| `cargo fmt` | ✅ 通过 |
| `cargo clippy -D warnings` | ✅ 零告警 |
| `cargo test --workspace` | ✅ 97 个测试通过（core 51 / render 20 / platform 14 / storage 9 / app 3） |
| `--selftest` | ✅ 退出码 0，五个定时器空闲，不读写用户配置 |
| release 构建 | ✅ 通过 |
| 设置中心 | ✅ `dotnet build` 通过（C# 改动）；XAML 未改动 |

本轮重点修复：Waiting 永久帧定时器、配置双写与相对路径、失效角标、UI 线程上的
`.lnk` 解析、selftest 副作用、菜单无界轮询、滑杆范围不一致、图标缓存与迟到结果、
极端布局越界、WinEvent 事件风暴、单实例错误分类、退出重复写盘、非法版本只读、
图标宽高比、字体目录、打包完整性检查与许可证/CI。

**v0.3.3 追加**：毛玻璃背景改为 `Windows.UI.Composition` HostBackdropBrush
（`WS_EX_NOREDIRECTIONBITMAP` + `CreateDesktopWindowTarget` + 圆角 composition clip +
DWM `DWMWA_USE_HOSTBACKDROPBRUSH`），实时 GPU 模糊、无第二个进程、启动不截屏；
Composition 不可用时直接使用主题半透明渐变。DWM system backdrop 与 WinUI 3
`DesktopAcrylicBackdrop` 在本机均返回成功但不出材质，已放弃。

## 14. v0.4.0 动效系统（2026-11-14）

### 14.1 交付范围

- 6 套悬停动效：Glide Zoom / Bounce / Wave / Fluid Press / Glow / Perspective；
- 4 套启动动效：Glide Launch / Triple Bounce / Ripple Launch / Pulse Launch；
- 五层独立通道与统一 `MotionSnapshot`（Hover / Press / Launch / Status / Compose）；
- Glide Zoom 改为全 Dock 连续放大场：固定锚点、Smoothstep、35/50ms 指数平滑，
  默认 1.35× / 18 DIP 间距 / 112.2 DIP 半径；
- schema v2：新增 `motion`，`Behavior` 只保留行为开关；v1→v2 迁移；
- schema v3：`DockItem.launch_type`；`.lnk` 解析后优先直接启动 EXE；商店应用
  AppsFolder 扫描 + AUMID 激活；
- `LauncherService` 解耦：动画引擎不等待启动结果，等待/运行只由 WindowTracker 决定；
- 刷新率查询（DWM rateCompose，回退 60Hz）与帧定时器适配；
- 设置中心动画页：预设、最大倍率、时长、弹性、范围、减少动态、复制配置；
- 渲染：图标 tilt、蓝紫光晕、扩散光环、锁定焦点绘制；
- 右键“添加商店应用...”：后台扫描 AppsFolder，原生菜单选择，AUMID 落盘；
- 右键“添加开始菜单”“添加分隔线”：内置 Widget / Separator 条目，可拖动排序；
- Dock 层级 `desktop` / `dynamic` / `topmost`，设置中心切换；
- 15–24 号离线动效预览图；
- 版本升至 0.4.0。

### 14.2 验证记录

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p glide-dock -- --selftest
cargo run -p glide-dock -- --render-preview preview
powershell -ExecutionPolicy Bypass -File scripts/build-settings.ps1
```

| 检查 | 结果 |
| --- | --- |
| `cargo fmt` | ✅ 通过 |
| `cargo clippy -D warnings` | ✅ 零告警 |
| `cargo test --workspace` | ✅ 175 个测试通过（core 110 / render 23 / platform 20 / storage 13 / app 9） |
| `--selftest` | ✅ 退出码 0，五个定时器空闲，不读写用户配置 |
| `--render-preview` | ✅ 生成 01–24 号图，动效图不 panic |
| 设置中心 | ✅ `scripts/build-settings.ps1` 自包含发布通过（166.4 MB） |
| v1→v3 | ✅ 旧 magnification/stiffness/damping 迁入 motion，launch_type 按目标推断，未知字段保留 |
| 商店应用 | ✅ AppsFolder 扫描 33 项；AUMID 图标、AAM 创建、Calculator 激活（Medium 测试进程）通过 |

### 14.3 明确取舍

- 不迁移 `UpdateLayeredWindow` 图标渲染到 WUC；Perspective 用仿射近似 ±5°；
- Bounce/Glow 的点击彩蛋在对应 Launch 已提供同类效果时抑制；
- 设置中心不内嵌动效预览，真实预览是实时同步的 Dock；
- 启动/按压时间轴不随 `hover_ms` 缩放，悬停层按基准时长缩放。 

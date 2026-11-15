# Dock 图标动效与启动动画（v0.4.0）

实现位置：

- 五层动效引擎：`crates/dock-core/src/motion/`
- 配置模型与 v3 schema：`crates/dock-core/src/models.rs`
- 布局与放大基数：`crates/dock-core/src/layout.rs`
- 图标变换、光晕与光环：`crates/dock-render/src/lib.rs`、`painter.rs`
- 输入/启动/窗口跟踪接线：`app/src/app.rs`、`app/src/launcher.rs`
- 刷新率查询：`crates/dock-platform-windows/src/monitor.rs`
- 设置中心：`settings-app/`

> 渲染后端保持不变：Dock 图标仍由 `UpdateLayeredWindow` + tiny-skia 绘制。
> Windows.UI.Composition 只负责毛玻璃 HostBackdropBrush。动效的“合成”在本项目中
> 指五层独立通道的合成模型，而不是把图标迁移到 WUC 视觉树。整体迁移属于 M6。

## 1. 配置

`config.json` 的 `version` 为 3，动画参数集中在顶层 `motion`：

```json
"behavior": {
  "magnification": true,
  "auto_hide": false,
  "auto_hide_delay_ms": 450,
  "launch_animation": true,
  "show_tooltips": true,
  "show_running_indicator": true
},
"motion": {
  "enabled": true,
  "respect_reduced_motion": true,
  "hover_preset": "glide_zoom",
  "launch_preset": "glide_launch",
  "max_scale": 1.35,
  "range_slots": 1.7,
  "hover_ms": 180,
  "elasticity": 0.75
}
```

- `hover_preset`：`glide_zoom` / `bounce` / `wave` / `fluid_press` / `glow` / `perspective`。
- `launch_preset`：`glide_launch` / `triple_bounce` / `ripple_launch` / `pulse_launch`。
- `max_scale`：1.05–3.50；`range_slots`：0.5–8.0；`hover_ms`：80–400；`elasticity`：0–1。
- 新装默认基础图标 48 DIP、间距 18 DIP（中心间距 66 DIP）；`Glide Zoom` 峰值 1.35×、
  影响半径 1.7 × 中心间距 = 112.2 DIP，进入 35ms / 退出 50ms。
- `hover_ms` 只作用于非 Glide Zoom 预设；Glide Zoom 的 35/50ms 是固定响应常数。
- 未知预设字符串回退默认值，不会导致整份配置被判损坏。
- `enabled=false` 关闭所有视觉动效；`respect_reduced_motion=true` 时跟随 Windows 的
  “动画效果”辅助功能开关。
- `behavior.magnification=false` 只关闭悬停层；`behavior.launch_animation=false` 只关闭
  启动时间轴。

### v1 / v2 → v3 迁移

- `behavior.magnification_factor` → `motion.max_scale`；
- `behavior.magnification_range` → `motion.range_slots`；
- `behavior.stiffness` / `damping` 反推 `motion.hover_ms` / `elasticity`；
- 旧数值字段从 `behavior` 移除，未知字段继续保留；
- v3 为每个 `items[]` 增加 `launch_type`；缺失时按目标/快捷方式推断，LNK→EXE
  条目会变成 `exe`，之后直接 `CreateProcessW`；
- 新装默认 1.35× / 1.7 slots / 180ms（非 Glide Zoom 预设）/ 75%；迁移文件保留旧手感。

## 2. 五层结构

```text
pointer / WindowTracker / LauncherService
                 │
                 ▼
        MotionEngine（纯 Rust，无 OS 依赖）
   Hover   Press   Launch   Status
        └─────────┬─────────┘
             Compose
                 │ MotionSnapshot
                 ▼
          FrameLayout + DockRenderer
```

| 层 | 负责 | 输出 |
| --- | --- | --- |
| Hover | Glide Zoom 连续场 / Bounce / Wave / Fluid Press / Glow / Perspective | 布局缩放、悬停位移、倾斜、光晕 |
| Press | Fluid Press、Bounce/Glow 点击彩蛋 | 按压倍率、点击位移、点击光环 |
| Launch | 4 套启动时间轴、Waiting/Settling、窗口确认 | 启动倍率、启动位移、启动光晕/光环、邻位避让 |
| Status | 等待呼吸点、运行点 | `waiting_opacity`、`running_dot_allowed` |
| Compose | 独立通道合并 | 每图标 `IconMotion` |

### Glide Zoom 连续场

`crates/dock-core/src/magnification/` 独立实现苹果式放大：

```text
单一鼠标 x + 固定锚点 aᵢ
      │
      ▼
u = clamp(1 - |x-aᵢ|/R, 0, 1)
w = u²(3-2u)
target = 1 + (Smax-1)w
      │
      ▼
current += (1-exp(-dt/τ))·(target-current)   τ = 35ms 放大 / 50ms 缩小
```

- 每个图标没有独立 MouseEnter/MouseLeave 动画，也没有独立定时器；
- 每帧只读取最新指针，所有图标统一更新；
- 默认 1.35× 且 18 DIP 间距时使用固定槽位中心；更大倍率才启用确定性避让；
- 图标宽度/位置按 bottom-center 扩展，底部基准线固定；窗口尺寸不变；
- 锚点来自 `DockGeometry::slot_centers()`，绝不使用缩放后的视觉中心；
- 鼠标离开时目标设为 1.0，当前值保留并按 50ms 衰减；恢复中重新进入直接续接；
- 关闭动画或系统减少动态效果时控制器立即归 1。

### 合成优先级

- `scale = layout_scale × press_scale × launch_scale`；
- `offset = hover + press + launch`，整体 clamp 到 ±16 DIP；
- `glow = max(hover, press, launch)`；
- `ripple = launch_ripple` 优先，其次 `press_ripple`；
- Status 最后绘制，任何缩放/位移/光环都不能清除等待点或运行点；
- 启动时间轴一旦开始，鼠标离开、悬停切换、设置切换都不能取消或重定向它；
- 重复点击重启同一个 Launch 时间轴，不叠加第二个启动动画；
- 启动动画结束不等于应用已启动：`Waiting` 只能由 WindowTracker 确认、失败或 12s 超时结束。

## 3. 悬停动效

默认 `hover_ms = 180`，作用于非 Glide Zoom 预设；Glide Zoom 使用固定 35/50ms
连续场。所有其他悬停层时长按 `hover_ms / 180` 缩放并 clamp 0.5–2.5；
按压/启动保持规格中的绝对时长。

| 预设 | 行为 | 默认时间 |
| --- | --- | --- |
| **Glide Zoom**（默认） | 全 Dock 连续鼠标场：固定锚点 + smoothstep 权重 + 指数平滑，峰值 `max_scale`，半径 `range_slots × 中心间距`，无锚点延迟、无过冲 | 进入 35ms，退出 50ms |
| **Bounce** | 悬停图标向上跳 `10 × (max_scale − 1)` DIP，带一次轻回弹 | 单次悬停 300ms，离开 220ms |
| **Wave** | 从悬停图标向两侧依次放大；`delay = |i−anchor| × min(40ms, 260ms/(n−1))`；上升 120ms，离开后 220ms 衰减 | 传播 260ms，衰减 220ms |
| **Fluid Press** | 悬停只鼓起 1.02×，重点在按压：按下 75ms 到 0.94×，释放 220ms 到 1.02× 再回 1.00× | 按下 75ms，释放 220ms |
| **Glow** | 边缘柔光 180ms 进入，悬停期间 2.4s 周期 ±8% 呼吸；点击时 420ms 光环 | 光晕 180ms，光波 420ms |
| **Perspective** | 按光标相对图标中心计算 ±5° 倾斜，距离衰减；离开后 200ms 回正；2D 仿射近似 | 进入 180ms，恢复 200ms |

- Glide Zoom 使用统一鼠标位置、固定锚点和单一平滑状态，不再为每个图标建立
  Enter/Leave 动画或 35ms/槽延迟；独立的 Wave 预设提供传播效果。
- Bounce 的“两次递减弹跳”和 Glow 的“点击光波”是点击彩蛋；当所选 Launch 已经提供
  同类效果（Bounce + Triple Bounce、Glow + Ripple Launch）时自动抑制，避免双重动画。
- Glow 的呼吸只在悬停期间运行；鼠标离开后光晕收敛并停止帧定时器。

## 4. 启动动效

以鼠标释放为 T=0；第一段从当前按压采样倍率开始，用缓动补到第一个关键帧，避免
0.94 → 1.00 跳变。表中倍率是相对布局基底的乘数。

| 预设 | 关键帧 | 时长 |
| --- | --- | --- |
| **Glide Launch**（默认） | 0ms 按压值 → 100ms `1.16×, −8 DIP` → 210ms `1.04×, −2 DIP` → 340ms `1.00×, 0` | 340ms |
| **Triple Bounce** | 0ms 按压值 → 100ms `1.15×` → 230ms `0.97×` → 350ms `1.04×` → 520ms `1.00×` | 520ms |
| **Ripple Launch** | 0ms `1.00×` → 120ms `1.06×` → 420ms `1.00×`；蓝紫色圆环半径 `0.5D → 1.8D`、线宽 `2.5 → 0.5 DIP`、透明度 `0.55 → 0` | 420ms |
| **Pulse Launch** | 0ms `1.00×, glow 0` → 120ms `1.10×, glow 0.9` → 240ms `1.03×, glow 0.45` → 400ms `1.00×, glow 0` | 400ms |

- 启动邻位：Glide Launch 第一邻位 `0.025D`、第二 `0.010D`，100ms 达峰，340ms 归零；
  Triple Bounce `0.018D` / `0.007D`，300ms 归零；Ripple/Pulse 不影响邻居位置。
- `Waiting` 呼吸点：直径 4 DIP，图标下 5 DIP，透明度 0.55–1.00，1s 周期余弦；
  减少动态效果时固定 0.75。
- 启动失败：取消等待、140ms 回中、显示错误气泡，不点亮运行点。
- 运行点：同位置固定强调色圆点，只在 WindowTracker 确认后显示，且按压/启动/等待期间隐藏。

## 5. 交互与启动接线

- 点击图标：`WM_LBUTTONUP` 先调用 `MotionEngine::pointer_up`，再 `DockCommand::Activate`；
  `DockApp::activate` 调用 `motion.begin_launch`，并交给 `LauncherService` 在后台线程启动。
- `LauncherService` 只返回成功/失败；成功不会写 `running` 或结束 `Waiting`。
- WindowTracker 的 `window_detected` / `window_closed` 是运行状态的唯一来源。
- 启动失败时 `motion.launch_failed` 让图标 140ms 回中；仍在运行的应用不会被清掉运行点。
- 启动等待超过 12s 超时回到 `Idle`，避免托盘/无窗口应用永久占用帧定时器。

## 6. 刷新率、DPI 与降级

- `DwmGetCompositionTimingInfo.rateCompose` 查询合成刷新率，失败回退 60 Hz；
  帧定时器间隔 `clamp(1000/hz, 6, 16)ms`，`WM_DISPLAYCHANGE` 重新查询。
- 动画全部以绝对时间采样，帧率只影响采样密度；60/120/144 Hz 下时长一致。
- 单帧 delta clamp 34ms，休眠或掉帧不会让图标跳变。
- 所有像素常量通过 `Metrics::scale` 转换；Perspective 角度不随 DPI 变化。
- `motion.enabled=false` 或系统减少动态效果：所有通道归零，等待/运行点静态；
  启动点击直接进入 Waiting/Idle，不播放时间轴。
- 空闲判定：所有弹簧收敛、无启动/等待/呼吸后立即 `KillTimer`；菜单、提示、错误气泡
  仍使用原有独立 spring。

## 7. 设置中心

WinUI 3 设置中心的“动画”页提供：

- 6 个悬停预设与 4 个启动预设的下拉选择；
- 最大放大倍率、动画时长、弹性强度、影响范围滑杆；
- 启用动画、跟随系统减少动态效果开关；
- 推荐搭配提示（不强制）；
- “复制动画配置”：把 `motion` 对象序列化为缩进 JSON 写入剪贴板。

设置改动通过 `WM_COPYDATA` 实时发送给 Dock；Dock 仍是 `config.json` 唯一写者，
复制/落盘由 Dock 侧 300ms 防抖完成；设置中心不内嵌独立动效预览，真实效果以桌面 Dock 为准。

## 8. 验证

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p glide-dock -- --selftest
cargo run -p glide-dock -- --render-preview preview
powershell -ExecutionPolicy Bypass -File scripts/build-settings.ps1
```

覆盖点：

- 6 套悬停的峰值/退出/延迟/角度；
- 4 套启动的每个关键帧、时长、从按压值开始不跳变；
- Hover 退出不取消 Launch、重复点击不叠加、中途切换预设不改变当前 Launch；
- 组合倍率相乘、位移相加、光环优先级、Status 不被覆盖；
- 减少动态效果、动画关闭、60/120/144 Hz、DPI 1.0/1.25/2.0；
- v1/v2→v3 迁移、未知预设回退；
- 15–24 号离线动效图：6 套悬停峰值 + 4 套启动关键帧。

# 设置中心与安装器检查记录

检查日期：2026-11-21。当前工作区版本：0.4.2。

## 当前运行链路

- Dock：Rust + Win32，配置由 Dock 主线程校验、应用与保存。
- 设置中心：`app/ui/settings.slint` 定义界面，`app/src/app/settings.rs` 承载 Slint 窗口线程与命令队列。
- 安装器：`installer/ui` 使用 React、Fluent UI React v9 和官方 Fluent Icons，`installer/src/main.rs` 承载命令与工作进程，`installer/src/engine.rs` 执行安装、更新和卸载。前端通过 Vite 构建到 `target/installer-ui` 后嵌入 EXE，运行时使用本地资源。
- 分发：`scripts/package.ps1` 生成 Dock payload，`scripts/build-installer.ps1` 将其嵌入安装程序。
- 旧版 `settings-app` / `setup-app` 仍保留在仓库中，不参与当前安装包的编译和发布。

## 已修正的问题

| 问题 | 原因 | 调整 |
| --- | --- | --- |
| 切换侧栏后多个选项同时高亮 | 每个 Button 都是独立的 checkable 按钮，内部勾选状态可覆盖外部绑定 | 用当前 page 决定 primary 状态，页面编号成为唯一状态来源；强调色按钮采用相同方式 |
| 下拉框异常变高、文字与控件相互挤压 | 固定高度卡片内的布局将多余高度分配给控件，文字没有统一宽度约束 | 复用 SettingCard，控件行保持 34 DIP，文字换行；窄卡片将控件放到说明下方 |
| 设置窗口一直使用浅色背景 | 界面颜色写死，与 Dock 的 theme 设置脱节 | 共用浅深色调色板，并同步 Slint 标准控件的 Palette 与 Windows 主题变化 |
| 小屏幕或高缩放下窗口超过工作区 | 初始大小固定为 980×720 DIP，未计算任务栏与显示缩放 | 以 Dock 所在显示器工作区计算初始尺寸和居中位置；矮窗口收紧侧栏 |
| 卡片末尾与诊断文字被裁切 | 内容使用固定高度、滚动区域未为滚动条预留间距 | 内容卡片按文字高度计算，长诊断按字符换行，列表单独滚动并预留滚动条空间 |
| 调参缺少数值反馈 | 滑块未显示当前值 | 显示 DIP、百分比、倍率和毫秒等数值；补回图标间距入口 |
| 安装器在窄工作区出现横向挤压 | body 的 760×600 最小尺寸与双列布局限制了可用空间 | 最小窗口调整为 560×400 DIP，窄窗口改单列，内容滚动、操作区保留 |
| 安装错误信息难以完整阅读 | 状态行强制单行并省略；长错误也可能撑高底部 | 详细信息放到内容区的 Fluent MessageBar，状态栏保留短提示；切页重置滚动和键盘焦点 |
| 对勾像字符，标题栏按钮错位且过大 | 手写 Unicode 勾号，最小化符号额外偏移，标题栏与按钮高度过大 | 使用官方 Fluent SVG 图标；三个窗口按钮统一为 40×32 DIP，图标 16 DIP，标题栏 40 DIP |
| 套用控件后界面仍显杂乱 | 顶部标签式步骤、零碎卡片和应用列表同时抢占空间 | 改为左侧步骤与右侧内容区；首次设置合并为三行偏好，应用列表通过 Accordion 按需展开；使用统一中文字体与 Fluent 主题 |
| 安装器只有以管理员身份运行才能打开 | dist 中的 EXE 继承了工作区 Low Integrity 标签，普通启动时 WebView2 创建失败；setup hook 返回错误又触发框架 panic | 打包时将交付 EXE 设为 Medium 并核验，保留原有访问规则；运行时识别 Low Integrity，窗口创建失败时显示原始错误并退出 |
| 安装后内置中文字体回退 | 字体只存在于开发目录，没有复制进 payload | 打包到 assets/fonts，并同时携带 OFL 许可证 |

## 验证

- Fluent UI 本地验收包（正式发布由 CI 重新构建）：`dist/installer/Glide-Dock-v0.4.2-Setup.exe`，约 37 MiB；SHA-256 `A90472CA14202097B3BABDE41BDEF7BD39424CF398D64241B70A746BF583E511`，与校验文件一致。普通启动 TokenIntegrityLevel=8192、TokenElevation=0、退出码 0、标准错误为空。
- 最终包原生交互：UI Automation 核对标题栏按钮等宽等高且 y 坐标一致，最大化/还原、卸载弹窗取消与默认保留数据、读取实际安装信息的更新确认页、右上角关闭均通过。仅打开预览，未执行安装、更新或卸载。

- 安装器普通启动回归：在实际交付路径 `dist/installer/Glide-Dock-v0.4.2-Setup.exe` 连续三次启动，核验进程 TokenIntegrityLevel=8192（Medium）、TokenElevation=0，窗口显示后正常关闭，标准错误为空。
- 启动异常回归：直接运行 Low Integrity 的 Cargo 产物时显示中文说明；用仅对子进程生效的 WebView2 路径覆盖模拟运行库不可用时显示原始错误。两者确认后均以状态码 1 退出，无 panic。
- 打包权限回归：以继承 Low Integrity 的临时文件验证 Medium 归一化，重复执行后内容哈希与原有 DACL 均保持一致，权限级别通过数值 SID 核验，不依赖 Windows 显示语言。
- 本次启动修复后复核：`cargo clippy --locked -p glide-setup --all-targets -- -D warnings` 与 `cargo test --locked -p glide-setup` 通过（2 个安装器单元测试）；下列全工作区结果来自设置页面修改后的检查。
- `cargo fmt --all -- --check` 通过。
- `cargo clippy --locked --workspace --all-targets -- -D warnings` 通过。
- `cargo test --locked --workspace`：187 个单元测试通过。此工作区构建产物带 Low Integrity 标签，运行测试时将 TEMP/TMP 指向工作区内可写临时目录，避免安装器测试在系统临时目录创建文件时被拒绝。
- 发布版设置窗口实机核对：浅深主题、常规/外观/行为/动画/诊断/关于，1225×900 与 800×550 物理像素窗口，Windows 缩放 125%。
- 侧栏连续切换与重复点击：浅深主题各 15 次，通过 UI Automation 调用真实按钮并检查截图，始终只有当前页面高亮。
- Fluent UI 安装器：生产构建经过 TypeScript 严格检查；Edge Playwright 覆盖 22 个测试，包含浅深主题，840×620、960×720、640×480、560×400 与 100%/125%/150%/200% 设备缩放，单一选中步骤、窗口按钮对齐、应用搜索与选择保留、异步路径校验、长错误、小窗口按钮可达性、卸载对话框焦点与 Escape、更新保留参数、防重复提交、进度不回退、失败重试和默认保留数据。
- 安装器自动化使用模拟的 Tauri 命令响应，只验证界面与导航；实际安装、UAC 提权、升级回滚和删除用户数据未在本机执行。

当前设置截图：[浅色](../preview/11-settings-light.png)、[深色](../preview/12-settings-dark.png)。
Fluent UI 安装器截图：[原生管理界面](../preview/27-installer-native-management.png)、[浅色](../preview/25-installer-fluent-light.png)、[深色](../preview/26-installer-fluent-dark.png)。

## 后续建议

- 将旧 WinUI 工程的中文字体源迁到共享 assets 目录，减少当前打包流程对旧工程资源路径的依赖。
- 为新装、覆盖升级、卸载与不同管理员账户确认建立独立 Windows 虚拟机验收流程。

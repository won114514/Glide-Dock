# 应用启动与商店应用扫描

Glide-Dock 用一个后台 `LauncherService` 统一处理所有启动请求。UI 线程只负责发出
请求和播放启动动效，不接触 `CreateProcessW` / COM / Shell。

## 启动类型

| `launch_type` | 目标 | 实现 |
| --- | --- | --- |
| `exe` | `.exe` / `.com` | `CreateProcessW`，带标准用户环境块、解析出的参数和起始目录 |
| `shortcut` | `.lnk` | `ShellExecuteExW("open")`，保留快捷方式语义 |
| `file` | 任意文件 | `ShellExecuteExW` |
| `folder` | 文件夹 | `ShellExecuteExW` |
| `url` | `http(s)://` | `ShellExecuteExW` |
| `packaged_app` | AppUserModelID | `IApplicationActivationManager::ActivateApplication` |

`auto` 只作为反序列化缺省值存在；`DockConfig::normalize` 在加载时把它解析成上面的
具体类型，不会写回 `auto`。

## 配置字段映射

产品方案里的 `DockApp` 对应现有的 `DockItem`。为兼容已有 v3 配置，保留稳定数字 id
和现有字段名：

| 方案字段 | Glide-Dock 字段 | 说明 |
| --- | --- | --- |
| `id: String` | `id: u32` | 稳定自增 id，避免重命名/迁移破坏引用 |
| `name` | `display_name` | 工具提示与菜单标签 |
| `icon` | 不持久化 | 每次按快捷方式/目标/AUMID 用 Shell 提取 |
| `target` | `target_path` | EXE 路径、文件、URL 或 AUMID |
| `args: Vec<String>` | `arguments: Option<String>` | 保留快捷方式原始命令行，避免错误拆分引号 |
| `working_directory` | `working_directory` | 原样传给 `CreateProcessW` / Shell |
| `launch_type` | `launch_type` | 新增枚举，v3 起显式持久化 |

## 添加路径

```text
WM_DROPFILES
    │
    ▼
ResolveService（STA 后台线程）
    │  IShellLinkW: target / args / working directory
    ▼
ResolvedEntry { launch_type, target_path, ... }
    │
    ▼
DockItem（持久化到 config.json）
```

`.lnk` 在添加时就完成解析：

- 目标为 `.exe` → `launch_type=exe`。点击时直接 `CreateProcessW`，不再绕回 `.lnk`；
  `shortcut_path` 仍保留，用于图标回退和启动失败回退。
- 目标是 `shell:AppsFolder\{AUMID}` → 去掉前缀，存 AUMID，`launch_type=packaged_app`。
- 目标为空 / 仍是 `.lnk` → `shortcut`，交给 Shell。
- 文件夹 → `folder`；其他文件 → `file`；`http(s)` → `url`。

## 商店应用扫描

用户右键 Dock 背景选择 **添加商店应用...** 时：

1. `StoreAppService` 在后台 STA 线程中枚举 Start 的虚拟 `AppsFolder`
   （`FOLDERID_AppsFolder` + `IShellFolder::EnumObjects`）。
2. 只保留解析名带 `!` 的 AUMID（例如
   `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`），普通 Win32 Start 项会被跳过。
3. 结果按名称排序后，用原生 `TrackPopupMenuEx` 弹出选择菜单。
4. 选中项转成 `ResolvedEntry::packaged_app`，复用拖入路径的插入、图标、落盘逻辑。

这里不用 `PackageManager.FindPackages()`：普通桌面进程调用它可能返回
`E_ACCESSDENIED`；AppsFolder 枚举只读取当前用户的 Start 菜单，权限更稳定，且天然
只包含可启动项。

## Low Integrity 回退

`IApplicationActivationManager` 拒绝 Low Integrity 调用方。Dock 以 Low Integrity
运行时（例如直接从受沙箱标记的工作区运行 `target\release\glide-dock.exe`）：

- 先尝试 AAM；
- 失败后回退到 `ShellExecuteExW("shell:AppsFolder\{AUMID}")`；
- 仍失败才向 UI 报错。

`scripts/package.ps1` 生成的 `%LOCALAPPDATA%\Glide-Dock\...` 包为 Medium 完整性，
AAM 正常可用。测试时请用打包目录中的 exe，不要直接运行工作区产物。

## 代码位置

- `crates/dock-core/src/models.rs` — `LaunchType`、`DockItem`、v3 迁移推断
- `crates/dock-platform-windows/src/shell.rs` — 启动分派、LNK 解析、AAM
- `crates/dock-platform-windows/src/store.rs` — AppsFolder 扫描、原生选择菜单
- `app/src/launcher.rs` — 后台启动队列
- `app/src/app.rs` — 右键菜单 `AddStoreApp`、`on_store_apps`、插入条目

## 验证

- `resolve` 单测会把真实 `.lnk` 解析成 `LaunchType::Exe`，并保留参数；
- AppsFolder 扫描在本机返回 33 个打包应用；
- AUMID 图标通过 `shell:AppsFolder\{AUMID}` 提取成功；
- 在 Medium 完整性测试进程中创建 AAM 并激活 Calculator 成功。

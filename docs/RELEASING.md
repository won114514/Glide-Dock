# GitHub 与 CNB 双平台发布

主仓库为 `won114514/Glide-Dock`，CNB 仓库为 `https://cnb.cool/won114514/Glide-Dock`。
`.github/workflows/ci.yml` 是统一发布入口，两端使用同一次 Windows 构建产生的 EXE 和 SHA-256 文件。

## 一次性配置

GitHub 仓库的 Actions 配置中设置：

| 类型 | 名称 | 内容 |
| --- | --- | --- |
| Secret | `CNB_TOKEN` | 能向目标 CNB 仓库推送代码、读取和发布 Release 的令牌 |
| Variable | `CNB_REPOSITORY` | `won114514/Glide-Dock` |

CNB Git HTTPS 用户名为 `cnb`。所用令牌应具备目标仓库代码推送和 `repo-release:rw` 权限；
直接配置仓库路径即可，不要求账户资料或列举全部仓库的权限。
令牌通过环境变量传入，不放在 Git 远端地址、源文件或命令行参数中。

官方资料：[Git 访问](https://docs.cnb.cool/zh/guide/git-access.html)、
[访问令牌](https://docs.cnb.cool/zh/guide/access-token.html)、
[OpenAPI](https://docs.cnb.cool/zh/develops/openapi.html)。
发布实现依据 CNB 官方 `https://api.cnb.cool/swagger.json` 中的 Release 接口。

## 发布新版本

1. 更新工作区 `Cargo.toml` 的版本，同时更新安装器前端 `package.json` 和锁文件中的项目版本。
2. 在 `docs/releases/v<version>.md` 编写发布说明。
3. 提交代码，创建与该提交对应的版本标签，然后推送主分支和标签，例如：

   ```powershell
   git tag -a v0.4.2 -m "Release v0.4.2"
   git push --atomic origin main v0.4.2
   ```

Windows 任务执行格式、Clippy、Rust 测试、22 项界面测试和 CNB 发布脚本测试。
通过后构建安装器，两个发布任务下载同一份 Actions 产物，并分别发布到 GitHub 与 CNB。
`scripts/publish-cnb.mjs` 只同步发布主分支与当前标签，不使用 force、mirror 或删除远端引用。

## 上传与重试

CNB 上传顺序为：读取目标仓库 → 同步代码和标签 → 创建草稿 → 获取上传地址 →
PUT 附件 → 确认上传 → 校验附件 → 公开版本。对象存储使用预签名 URL，
不会收到 CNB Bearer 令牌；附件设置为永久保存。

脚本在网络请求之前核验本地 EXE 与 SHA-256 文件一致，并确认发布标签指向当前提交。
失败会使 CI 任务报错；新版本在附件未齐时保持草稿。重新运行失败任务时，
已有且能校验为相同内容的附件会复用，不会静默替换不同的版本文件。

独立运行的方式为（提前通过安全的环境变量方式设置 `CNB_TOKEN`）：

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/publish-cnb.mjs --tag v0.4.2 --assets dist/installer
```

脚本测试为 `node --test scripts/tests/publish-cnb.test.mjs`，测试使用模拟 API，不触发真实发布。

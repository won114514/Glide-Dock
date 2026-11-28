# GitHub 与 CNB 双平台发布

主仓库为 won114514/Glide-Dock，CNB 仓库为 https://cnb.cool/won114514/Glide-Dock。
GitHub Actions 执行一次 Windows 构建，然后并行发布 GitHub Release 与 CNB Release，两端使用同一份 EXE 与 SHA-256 文件。

## 一次性配置

GitHub 仓库的 Actions 配置中设置：

| 类型     | 名称           | 内容                                                             |
| -------- | -------------- | ---------------------------------------------------------------- |
| Secret   | CNB_TOKEN      | 具备目标仓库 repo-code:rw 与 repo-release:rw 权限的 CNB 访问令牌 |
| Variable | CNB_REPOSITORY | won114514/Glide-Dock                                             |

CNB Git HTTPS 用户名为 cnb。GitHub 中的 CNB_TOKEN 负责同步代码、版本标签并发布 CNB Release。
publish-cnb 任务显式使用 --publication release；权限不足或上传错误会直接失败，不会静默降级为提交附件。
令牌通过环境变量传入，不放在 Git 远端地址、源文件或命令行参数中。
CNB 的 .cnb.yml 只在 main 普通 push 时运行发布脚本测试，不再通过 tag_push 重复发布。

官方资料：[Git 访问](https://docs.cnb.cool/zh/guide/git-access.html)、
[访问令牌](https://docs.cnb.cool/zh/guide/access-token.html)、
[OpenAPI](https://docs.cnb.cool/zh/develops/openapi.html)。
发布实现依据 CNB 官方 https://api.cnb.cool/swagger.json 中的 Release 和 Commit Assets 接口。

## 发布新版本

1. 更新工作区 Cargo.toml 的版本，同时更新安装器前端 package.json 和锁文件中的项目版本。
2. 在 docs/releases/v<version>.md 编写发布说明。
3. 提交代码，创建与该提交对应的版本标签，然后推送主分支和标签，例如：

   ```powershell
   git tag -a v0.4.2 -m "Release v0.4.2"
   git push --atomic origin main v0.4.2
   ```

Windows 任务执行格式、Clippy、Rust 测试、安装界面和发布脚本测试，然后构建安装器。
publish-github 与 publish-cnb 任务并行：前者上传 GitHub Release，后者同步 main 与当前标签到 CNB 并发布正式 Release。
两端使用 glide-dock-installer 产物中的同一份 EXE 和 SHA-256 文件；同步不使用 force、mirror 或删除远端引用。
CNB main 的普通 push 只运行发布脚本测试，不执行发版。

## 上传与重试

脚本核对 GitHub 与 CNB 的标签提交、附件大小、GitHub 摘要及 SHA-256 文件，
然后在独立 Git worktree 中读取该标签的发布说明；不会将 main 回退到旧提交。
对象存储使用预签名 URL，不会收到 CNB Bearer 令牌。
已有且能通过大小与 SHA-256 核验的附件会复用，不会替换不同的版本文件。

publish-cnb 任务显式使用 --publication release。先创建草稿，附件齐全并校验后公开；失败保留草稿并报告错误。
可在 GitHub Actions 重新运行失败的 publish-cnb 任务。脚本会复用大小与 SHA-256 均匹配的附件，不会替换不同的版本文件。
首次接入或恢复当前 Cargo.toml 版本时，也可手动运行 Republish CNB version files 工作流。

在已经安全设置具备 repo-release:rw 权限的 CNB_TOKEN 环境变量时，也可本地发布已有安装包：

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/publish-cnb.mjs --tag v0.4.2 --assets dist/installer --publication release
```

## 从 GitHub 已发布版本恢复 CNB 下载

GitHub Actions 中的 **Republish CNB version files → Run workflow** 可恢复指定版本的下载文件。
它运行发布测试，从正式 GitHub Release 下载和验证文件，不重新编译，也不移动版本标签。
工作流使用自带的 GITHUB_TOKEN 查询 GitHub API，避免匿名请求额度耗尽；两个平台的令牌均不会发送到公开下载或对象存储地址。

此应急工作流默认使用 auto 模式：CNB_TOKEN 具备 repo-release:rw 时发布正式 Release；
只有代码权限时，将相同文件发布为版本提交的永久附件，并在日志中显示对应入口。
其他权限、网络或上传错误不会静默切换发布方式。正式 Release 的日常发布由 GitHub Actions 的 publish-cnb 任务负责。

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/mirror-github-release.mjs --tag v0.4.2
```

本地可通过 GITHUB_TOKEN 或 GH_TOKEN 环境变量提供 GitHub 认证；未设置时使用匿名 API 额度。
可显式选择 --publication release 或 --publication commit；显式选择不会自动切换类型。

测试使用模拟 API，不触发真实发布：

```sh
node --test scripts/tests/publish-cnb.test.mjs scripts/tests/mirror-github-release.test.mjs
```

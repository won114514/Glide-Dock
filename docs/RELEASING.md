# GitHub 与 CNB 双平台发布

主仓库为 won114514/Glide-Dock，CNB 仓库为 https://cnb.cool/won114514/Glide-Dock。
GitHub Actions 执行一次 Windows 构建，两端上传同一份 EXE 与 SHA-256 文件。

## 一次性配置

GitHub 仓库的 Actions 配置中设置：

| 类型 | 名称 | 内容 |
| --- | --- | --- |
| Secret | CNB_TOKEN | 能向目标 CNB 仓库推送代码的 Git 客户端凭据 |
| Variable | CNB_REPOSITORY | won114514/Glide-Dock |

CNB Git HTTPS 用户名为 cnb。当前“Git 客户端凭据”可以推送代码和上传提交附件，但不具备 repo-release:rw 权限。
发布脚本默认使用 auto 模式：有 Release 权限时发布正式 Release；缺少该权限时，将同一份安装包与校验文件作为版本对应提交的永久附件发布。
日志会明确显示发布类型和入口；其他权限、网络或上传错误不会静默切换发布方式。

如果必须使用独立的 CNB Release 页面，再为令牌增加目标仓库的 repo-release:rw 权限即可。
不需要读取账户资料、列举全部仓库，也不依赖 CNB 云原生构建的触发权限。
令牌通过环境变量传入，不放在 Git 远端地址、源文件或命令行参数中。

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

Windows 任务执行格式、Clippy、Rust 测试、22 项界面测试和发布脚本测试，然后构建安装器。
GitHub Release 上传 EXE 和 SHA-256 文件后，publish-cnb 任务下载同一份 Actions 产物，
同步 main 与当前标签到 CNB，然后发布版本文件；不使用 force、mirror 或删除远端引用。
提交附件位于对应提交页面的“附件”区域，可以从 CNB 的 Tag 列表打开版本提交。

## 上传与重试

脚本在网络请求之前核验本地 EXE 与 SHA-256 文件一致，并确认发布标签指向当前提交。
对象存储使用预签名 URL，不会收到 CNB Bearer 令牌；Release 和提交附件均设置永久保存。
已有且能核验为相同内容的附件会复用，不会静默替换不同的版本文件。

Release 模式先创建草稿，附件齐全并校验后公开；失败保留草稿。
提交附件模式逐个上传并确认，两个附件均通过校验才报告成功；中断后可以重新运行。

在已经安全设置 CNB_TOKEN 环境变量的情况下，本地发布已有安装包：

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/publish-cnb.mjs --tag v0.4.2 --assets dist/installer --publication auto
```

也可显式选择 --publication release 或 --publication commit；显式选择不会自动切换类型。

## 从 GitHub 已发布版本恢复 CNB 下载

GitHub Actions 中选择 **Republish CNB version files → Run workflow**，填写版本标签即可。
该任务运行发布测试，并从正式 GitHub Release 下载和验证文件，不重新编译，也不移动版本标签。

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/mirror-github-release.mjs --tag v0.4.2
```

镜像脚本核对 GitHub 与 CNB 的标签提交、附件大小、GitHub 摘要及 SHA-256 文件，
然后在独立 Git worktree 中读取该标签的发布说明；不会将 main 回退到旧提交。
GitHub 附件尚未就绪时最多等待约 20 分钟，等待期间不创建 CNB 发布数据。

测试使用模拟 API，不触发真实发布：

```sh
node --test scripts/tests/publish-cnb.test.mjs scripts/tests/mirror-github-release.test.mjs
```

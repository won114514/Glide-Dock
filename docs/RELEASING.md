# GitHub 与 CNB 双平台发布

主仓库为 `won114514/Glide-Dock`，CNB 仓库为 https://cnb.cool/won114514/Glide-Dock。
GitHub Actions 完成一次 Windows 构建；CNB 原生流水线下载这次 GitHub Release 的原始文件并核验 SHA-256，避免两端重新编译产生差异。

## 一次性配置

GitHub 仓库的 Actions 配置中设置：

| 类型 | 名称 | 内容 |
| --- | --- | --- |
| Secret | CNB_TOKEN | 能向目标 CNB 仓库推送代码的 Git 客户端凭据 |
| Variable | CNB_REPOSITORY | `won114514/Glide-Dock` |

GitHub 中的个人令牌只负责通过 Git 同步源代码与标签，需要目标仓库的代码读写权限；Git HTTPS 用户名为 cnb。
CNB 的 .cnb.yml 使用平台在可信 push/tag_push 流水线中注入的临时 CNB_TOKEN 进行发版，其权限包含 repo-release:rw，流水线结束后自动销毁。
这两个环境变量同名，但来源和用途不同；GitHub 不需要保存 CNB 的个人发版令牌。

令牌通过环境变量传入，不放在 Git 远端地址、源文件或命令行参数中。
CNB 流水线不监听 PR 或其他不可信事件，发版锁避免同一仓库的镜像任务并发修改附件。

官方资料：[Git 访问](https://docs.cnb.cool/zh/guide/git-access.html)、
[访问令牌](https://docs.cnb.cool/zh/guide/access-token.html)、
[流水线临时令牌](https://docs.cnb.cool/zh/build/build-in-env.html#cnb_token)、
[OpenAPI](https://docs.cnb.cool/zh/develops/openapi.html)。
发布实现依据 CNB 官方 https://api.cnb.cool/swagger.json 中的 Release 接口。

## 发布新版本

1. 更新工作区 Cargo.toml 的版本，同时更新安装器前端 package.json 和锁文件中的项目版本。
2. 在 docs/releases/v<version>.md 编写发布说明。
3. 提交代码，创建与该提交对应的版本标签，然后推送主分支和标签，例如：

   ```powershell
   git tag -a v0.4.2 -m "Release v0.4.2"
   git push --atomic origin main v0.4.2
   ```

Windows 任务执行格式、Clippy、Rust 测试、22 项界面测试和发布脚本测试，然后构建安装器。
GitHub Release 上传 EXE 和 SHA-256 文件后，publish-cnb 任务只同步 main 和当前标签到 CNB；不使用 force、mirror 或删除远端引用。
CNB 的 tag_push 任务随后下载 GitHub 已发布的同一份附件，验证两端标签提交、附件大小、GitHub 摘要与 SHA-256 文件，再发布 CNB Release。

## 上传与重试

CNB 上传顺序为：读取目标仓库 → 创建草稿 → 获取上传地址 → PUT 附件 → 确认上传 → 校验附件 → 公开版本。
对象存储使用预签名 URL，不会收到 CNB Bearer 令牌；附件设置为永久保存。
镜像旧版本时在独立 Git worktree 中读取该标签的发布说明，不会将 main 回退到旧提交。

网络中断会使对应任务报错；新版本在附件未齐时保持草稿。CNB 流水线可重新运行，
已有且能核验为相同内容的附件会复用，不会静默替换不同的版本文件。
GitHub Release 尚未就绪时最多等待约 20 分钟，等待期间不创建 CNB Release。

首次接入或恢复当前工作区版本时，也可在 main 分支提交标题中加入 [release-mirror] 并推送到 CNB。
main 的普通 push 只运行发布脚本测试，版本标签负责日常发版。

CNB 可信流水线内可直接指定版本：

```sh
node scripts/mirror-github-release.mjs --tag v0.4.2
```

如果另有具备 repo-release:rw 的个人令牌，也保留从本地安装包发布的方式：

```powershell
$env:CNB_REPOSITORY = 'won114514/Glide-Dock'
node scripts/publish-cnb.mjs --tag v0.4.2 --assets dist/installer
```

测试使用模拟 API，不触发真实发布：

```sh
node --test scripts/tests/publish-cnb.test.mjs scripts/tests/mirror-github-release.test.mjs
```

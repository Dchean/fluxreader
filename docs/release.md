# 发布流程与说明文案

## 分支与发布

`dev` 承接调整，提交推送并验证后常规合并进 `main`；版本 bump 与发布 tag 在 `main`。两个分支项目内容同源，不能为发布剔除 `.agents/notes/`。安装包为 Windows x64 `setup.exe` 与 `.msi`，发布在 [Releases](https://github.com/Dchean/fluxreader/releases)。

发布顺序：

1. 在 `tmp/release-notes-vX.Y.Z.md` 起草附注，只写本版真实用户变化。
2. 在 `main` 做独立版本 bump 提交，推送并等待该 SHA 的 CI 通过。
3. 以 main tip 的 bump 提交创建 annotated tag：`git tag -a vX.Y.Z <bump提交> -F <附注文件>`，再推送 tag。
4. Release 工作流先核对 tag、main 归属与版本文件，再调用该提交的完整 `ci.yml`。全部通过后才构建 Windows x64 安装包，上传草稿并验证内容，最后公开 Release。

`ci.yml` 对 main/dev push、面向 main 的 PR 及 `workflow_call` 触发。`v*` tag 只触发 `release.yml`，由它调用同一提交的 `ci.yml`，因此 tag 不另起一套重复 CI，也不轮询历史成功记录。调用保留 frontend、Windows Edge 布局和 Windows Rust 三个 job 的全部门禁；任一失败或超时都阻止构建发布。发布前仍须满足 [AGENTS.md](../AGENTS.md) 的审计与验证要求；本文件不是执行发布的授权。

发布只接受稳定 `vX.Y.Z` annotated tag，附注非空、直接指向本次 workflow SHA，且该提交可达于远端 main。tag 版本必须与 `package.json`、`package-lock.json` 的根与本包版本、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 和 `Cargo.lock` 的 app 包版本一致。CI 与构建都检出固定 SHA；构建前和公开前复核 tag，防止等待期间 tag 被改指。流程仍要求创建 tag 时指向 main tip；可达性检查允许 main 后来前进时恢复尚未公开的草稿。

tag 逐版推送并等本版完成后再推下一版。Release 使用全局并发组且不取消正在运行的发布，但 GitHub 默认只保留一个等待中的 run；它不是无限发布队列。

main bump 后 dev 版本短暂落后是分支内容一致约定的临时例外。下一次 dev 开发前将 main 合回 dev，使版本文件重新对齐。

## 说明来源与格式

新版本 Release 正文以 annotated tag 附注为唯一来源。工作流通过 GitHub API 读取 tag 对象 `message` 并原样传递；轻量 tag 或空附注会失败。附注作为数据编码传递，不插进 shell 命令。手动重试同样只能在 tag ref 上运行：`gh workflow run release.yml --ref vX.Y.Z`；在分支上手动运行会失败。

```text
FluxReader vX.Y.Z

一句话概览：这一版给用户带来了什么。

新功能：
- …

问题修复：
- …

改进：
- …

其他：
- …
```

按实际内容取舍小节，不写提交数、门禁数字、审查流程或 TASK/REQ 编号；内部证据留在对应验收记录。版本范围或提交数如用于内部核对，必须用 Git 实测，不能写进用户发布说明充当收益。提交信息格式见 [AGENTS.md](../AGENTS.md)。

## 草稿与失败恢复

固定版本的 tauri-action 只构建并提供安装包路径，不创建或更新 Release。脚本先验证本版各一个非空 x64 `setup.exe` 和 `.msi`；全部构建成功后才创建或恢复同 tag 草稿。恢复仅删除这两包的同名冲突资产和未完成上传的 `starter` 资产，保留其他文件；发现其他版本或架构的安装包则拒绝公开，交维护者检查。

每次恢复元数据、删除或上传资产、最终公开前，都按 Release ID 重新读取并确认 ID、tag 和草稿状态。读取失败、身份不符或已公开时立即停止后续写入；恢复元数据不发送 `draft: true`，避免把已公开版本撤回草稿。

上传后逐包核对名称、`uploaded` 状态、大小和 SHA-256。GitHub 未提供 digest 时，下载该资产计算 SHA-256；缺失 digest 不能视作校验成功。两包验证完成后才将草稿改为公开，随后再次读取 Release 和资产核对标题、正文、tag 与内容。

构建失败不会先删除已有资产，上传或校验失败不会由本流程公开草稿，修复原因后可在同 tag 上重试。发现已公开版本时拒绝重写，尤其不会覆盖历史版本的正文例外。若公开请求的响应失败，结果可能未知，先查看该 Release 的实际公开状态和资产，再决定下一步；已公开时不要重打 tag 或以重跑覆盖。

工作流运行期间，不得在 GitHub 页面或其他客户端同时修改或公开同 tag 的 Release。并发组只串行本工作流，无法锁住外部客户端；每次 HTTP 读取和随后的写入也不是原子操作，重新核对能拦住已观察到的状态变化，但不能消除检查与写入之间的全部竞态。

发布脚本的本地回归命令为 `node --test tools/release/release.test.mjs`，使用隔离文件与 API 替身；它验证失败关闭和恢复规则，不替代 GitHub runner、真实构建、上传及安装验证。

## 历史版本的例外

不重写历史提交哈希或重打历史 tag。这样会破坏既有 SHA 引用，并可能触发旧 Release 重构建、覆盖既有资产；旧提交也未必通过当前门禁。

2026-11-02 曾对 `v0.8.0`–`v0.18.7` 的 28 个 Release web 正文作面向用户的规范化，历史 tag 附注未动。因此这些版本允许 web 正文与旧附注不同；“来源单点”约束后续新发布，不能倒称旧文本一直相同。该历史决定见[文案与提交史边界](../.agents/notes/implemented/process/2026-11-02-历史文案规范化与提交史边界.md)。本次文档整理没有重新编辑这些外部 Release。

需要明确授权修正某个旧版 web 正文时使用 `gh release edit vX.Y.Z --notes-file <正文文件>`，不重打 tag。原由见[发布说明来源决定](../.agents/notes/implemented/process/2026-09-29-发布说明取自-tag-附注.md)。

# 发布流程与说明文案

## 分支与发布

`dev` 承接调整，提交推送并验证后常规合并进 `main`；版本 bump 与发布 tag 在 `main`。两个分支项目内容同源，不能为发布剔除 `.agents/notes/`。安装包为 Windows x64 `setup.exe` 与 `.msi`，发布在 [Releases](https://github.com/Dchean/fluxreader/releases)。

发布顺序：

1. 在 `tmp/release-notes-vX.Y.Z.md` 起草附注，只写本版真实用户变化。
2. 在 `main` 做独立版本 bump 提交，推送并等待该 SHA 的 CI 通过。
3. 以 main tip 的 bump 提交创建 annotated tag：`git tag -a vX.Y.Z <bump提交> -F <附注文件>`，再推送 tag。
4. Release 工作流检查 tagged commit 的 `ci.yml` 结果，随后构建资产并读取 tag 附注生成正文。

`ci.yml` 对 main/dev push、PR 和 `v*` tag 触发；tag CI 只提供门禁，不直接发布。`release.yml` 要求同 SHA 的 CI 全部成功，失败或超时拒绝发布。发布前仍须满足 [AGENTS.md](../AGENTS.md) 的审计与验证要求；本文件不是执行发布的授权。

main bump 后 dev 版本短暂落后是分支内容一致约定的临时例外。下一次 dev 开发前将 main 合回 dev，使版本文件重新对齐。

## 说明来源与格式

新版本 Release 正文以 annotated tag 附注为唯一来源。工作流通过 GitHub API 读取 tag 对象 `message`；轻量 tag 或空附注会失败，后续步骤再次核对并回写正文。手动 `workflow_dispatch` 保留静态说明回退，不能把它称为已读取 tag。

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

按实际内容取舍小节，不写提交数、门禁数字、审查流程或 TASK/REQ 编号；内部证据留在对应验收记录。版本范围或提交数如用于内部核对，必须用 Git 实测，不能写进用户发布说明充当收益。

## 历史版本的例外

不重写历史提交哈希或重打历史 tag。这样会破坏既有 SHA 引用，并可能触发旧 Release 重构建、覆盖既有资产；旧提交也未必通过当前门禁。

2026-11-02 曾对 v0.8.0–v0.18.7 的 28 个 Release web 正文作面向用户的规范化，历史 tag 附注未动。因此这些版本允许 web 正文与旧附注不同；“来源单点”约束后续新发布，不能倒称旧文本一直相同。该历史决定见[文案与提交史边界](../.agents/notes/implemented/process/2026-11-02-历史文案规范化与提交史边界.md)。本次文档整理没有重新编辑这些外部 Release。

需要明确授权修正某个旧版 web 正文时使用 `gh release edit vX.Y.Z --notes-file <正文文件>`，不重打 tag。原由见[发布说明来源决定](../.agents/notes/implemented/process/2026-09-29-发布说明取自-tag-附注.md)。

## 提交信息

`<type>(<scope>): <中文主题>`，type 为 feat / fix / docs / style / refactor / perf / test / build / ci / chore。主题一行说清内容，细节与动机放正文；破坏性变更以 `BREAKING CHANGE:` 开头说明。

# 发布流程与说明文案

## 分支与发布

- `main`：项目代码主分支。版本 bump 与发布 tag 都在这里。
- `dev`：开发分支。一切调整先在 `dev` 提交、推送并验证，通过后合并进 `main`。
- 安装包：GitHub [Releases](https://github.com/Dchean/fluxreader/releases) 提供 Windows x64 `setup.exe` 与 `.msi`。

发布序列：

```text
1. 起草附注到 tmp/release-notes-vX.Y.Z.md
   提交数用 git rev-list vX.Y.Z-1..HEAD --count 实测，不估
2. 在 main 上做 bump 提交，单独推送，等 CI 绿
3. git tag -a vX.Y.Z <bump提交> -F <附注文件>，推送 tag
4. Release 工作流自动构建（Release 正文取 tag 附注）
```

**tag 必须指向 main tip 的 bump 提交。** 版本号、合并记录与发布内容在这一提交上对齐。`ci.yml` 现在同时监听 `push: tags: ["v*"]`，tag 指向的提交会在该 SHA 上触发一次 CI；`release.yml` 仍要求该 SHA 上 `ci.yml` 全部成功，失败或超时都会拒绝发布。`ci.yml` 的 tag 触发只补门禁 run，不自动发布；发布由 `release.yml` 执行。

**历史提交哈希不重写。** 重打历史 tag 会逐个重新触发 Release 工作流，重建并覆盖既有发布资产；旧提交也未必能通过当前门禁。

### 发布窗口内的版本号差异

版本 bump 在 `main` 上完成，`dev` 仍停留在上一版本，两分支的版本文件会短暂不同。这是 [AGENTS.md](../AGENTS.md)「两个分支项目内容保持一致」的唯一临时例外。下一次在 `dev` 开发前，先把 `main` 合回 `dev`，让版本重新对齐，再继续改动。

## 发布说明只有一处来源

Release 正文 = annotated tag 的附注。`release.yml` 通过 GitHub API 读 tag 对象的 `message` 作为正文，遇到轻量 tag 或空附注**直接失败拒绝发布**，随后还有一步「Ensure release body matches tag annotation」作为双保险回写。手动触发（`workflow_dispatch`）时回退静态说明。

理由与取舍见 [发布说明取自 tag 附注](../.agents/notes/implemented/process/2026-09-29-发布说明取自-tag-附注.md)。

## 附注结构

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

附注面向**用户**，不面向项目内部：不写提交数、门禁数字、测试与审查流程、Note 引用等过程性内容（这些留在 Notes 与 roadmap），只写用户可感知的变化。小节按实际内容取舍，没有的不写。2026-11-02 起 28 个版本的 Release web 正文已全部按此口径重写；tag 附注属历史产物不再回改，此后新版本的 tag 附注直接采用本结构。

## 存量说明

2026-11-02 前，`v0.8.0`–`v0.16.0` 的 GitHub Release web 正文是旧的 `tauri-action` 静态文案，`v0.16.1`–`v0.18.7` 混有内部进度式表述；当日起全部 28 个版本的 web 正文已按上面的用户口径重写。此后需要修正某个版本的 web 正文时：

`gh release edit vX.Y.Z --notes-file <正文文件>`（历史 tag 附注不回改；旧版 web 正文与 tag 附注的分叉由正文内的溯源说明声明，2026-11-02 重写稿不再携带溯源注）。

## 提交信息约定

`<type>(<scope>): <中文主题>`，type 取 feat / fix / docs / style / refactor / perf / test / build / ci / chore。主题一行说清改了什么，细节与动机写正文，破坏性变更以 `BREAKING CHANGE:` 开头说明。

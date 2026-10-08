# 发布流程与说明文案

## 分支与发布

- `main`：项目代码主分支。版本 bump 与发布 tag 都在这里。
- `dev`：开发分支。一切调整先在 `dev` 提交、推送并验证，通过后合并进 `main`。
- 安装包：GitHub [Releases](https://github.com/Dchean/fluxreader/releases) 提供 Windows x64 `setup.exe` 与 `.msi`。

发布序列：

```text
1. 起草附注到 tmp/release-notes-vX.Y.Z.md
   提交数用 git rev-list vX.Y.Z-1..HEAD --count 实测，不估
2. bump 提交单独推送，等 CI 绿
3. git tag -a vX.Y.Z <bump提交> -F <附注文件>，推送 tag
4. Release 工作流自动构建（Release 正文取 tag 附注）
```

**tag 必须指向 main tip 的 bump 提交。** 门禁步骤要求「tagged commit 上 `ci.yml` 全绿」，而 `ci.yml` 只触发于 `push: branches: [main, dev]` 与 PR → main，没有 `push: tags: ["v*"]` 触发。tag 指向非 main tip 的提交时，该 SHA 上永远不会有 CI 运行，门禁会空等到 45 分钟超时后报错。这条是纪律不是机制，修法见 [roadmap.md](roadmap.md)。

**历史提交哈希不重写。** 重打历史 tag 会逐个重触发 Release 工作流，而旧提交上没有 CI 记录，门禁必然超时失败。

## 发布说明只有一处来源

Release 正文 = annotated tag 的附注。`release.yml` 通过 GitHub API 读 tag 对象的 `message` 作为正文，遇到轻量 tag 或空附注**直接失败拒绝发布**，随后还有一步「Ensure release body matches tag annotation」作为双保险回写。手动触发（`workflow_dispatch`）时回退静态说明。

理由与取舍见 [发布说明取自 tag 附注](../.agents/notes/implemented/process/2026-09-29-发布说明取自-tag-附注.md)。

## 附注结构

```text
FluxReader vX.Y.Z

距 v(X-1).Y.Z 共 N 个提交，主题是「…」。

行为修复：
- …

健壮性（Rust）：
- …

一致性统一（UI）：
- …

测试与基建：
- …

门禁数字与独立审查结论
已知限制（沿承上版）
```

附注结构与 `v0.16.0` 起的历次发布一致（`v0.17.0` 改用主题式叙述，是唯一一次偏离）。格式是书面惯例而非强制，没有脚本核对。

## 存量说明

`v0.8.0`–`v0.16.0` 的 GitHub Release web 正文仍是旧的 `tauri-action` 静态文案；这些版本的完整说明以 tag 附注为准。需要修正某个版本的 web 正文时：

```bash
gh release edit vX.Y.Z --notes-file <(git tag -l --format='%(contents:subject)%0a%0a%(contents:body)' vX.Y.Z)
```

## 提交信息约定

`<type>(<scope>): <中文主题>`，type 取 feat / fix / docs / style / refactor / perf / test / build / ci / chore。主题一行说清改了什么，细节与动机写正文，破坏性变更以 `BREAKING CHANGE:` 开头说明。

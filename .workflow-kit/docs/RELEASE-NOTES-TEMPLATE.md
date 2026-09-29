# 发布说明模板（v0.16.0 起）

Release 正文 = annotated tag 附注（release.yml 已改为自动提取附注；手动触发回退静态说明）。
今后发布说明只维护 tag 附注一处，不再有「tag 附注 / Release 正文」双轨。

## 附注文件结构

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
- cargo N 用例 / 0 failed / 9 ignored、前端回归 N 项、fmt/clippy/lint/build 全绿；
  独立审查（全新实例）PASS findings=0。

已知限制（沿 vX.Y.Z）：…
```

分节按当版实际内容取舍（无则省略），但「门禁数字与独立审查结论」和「已知限制」两段必须有。

## 流程

1. 起草说明到 tmp/release-notes-vX.Y.Z.md（提交数用 `git rev-list vX<上版>..HEAD --count` 实测）。
2. bump 提交单独推送，等 CI 在该提交上绿。
3. `git tag -a vX.Y.Z <bump提交> -F tmp/release-notes-vX.Y.Z.md` 并推 tag。
4. Release 工作流构建并以附注为 Release 正文。

## 存量说明

- v0.8.0..v0.16.0 的 GitHub Release web 正文是 tauri-action 旧静态文案，本机无 gh/token 不能改；
  完整变更说明以各版 tag 附注为准（git tag -n99 vX.Y.Z 可读）。owner 如需改 web 正文：
  `gh release edit vX.Y.Z --notes-file <(git tag -l --format='%(contents:subject)%0a%0a%(contents:body)' vX.Y.Z)`。
- 历史提交哈希不重写：改写祖先提交需重打全部 9 个 tag 并强推，重推 v* tag 会逐个重触发 Release
  （旧提交无 CI 运行记录，门控必超时失败），且 .workflow-kit 证据引用的哈希全部失效。规范自即日起生效。

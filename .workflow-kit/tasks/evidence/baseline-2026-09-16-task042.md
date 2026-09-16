# 基线：TASK-042 起点（2026-09-16，CI 门禁修复后）

- 代码基线：main @ `28d4bf7`（fix(ci): 修复 fmt 行宽与两处 clippy 失败）。历史重写前的提交号与当前提交号的对照见 `docs/BASELINE.md` 文末「提交号对照」。
- 前置修复：CI 自 2026-09-16T08:24Z 起连续 5 次失败的根因与修复见 `docs/FINDINGS-CI-GATE.md`（rustfmt 行宽 + 两处 clippy，后者被前者掩盖）。

## 门禁实测（本机，2026-09-16）

| 检查 | 结果 | 退出码 |
| --- | --- | --- |
| `npm run lint`（oxlint） | Found 0 warnings and 0 errors（23 文件） | 0 |
| `npm run build`（tsc -b + vite build） | 构建通过 | 0 |
| `npm run test:frontend`（状态机回归） | 26/26 通过（S-1..S-5） | 0 |
| `cargo fmt --all -- --check` | 无 diff | 0 |
| `cargo clippy --all-targets -- -D warnings` | 全目标零告警 | 0 |
| `cargo test` | 全部 `test result: ok`，0 failed | 0 |
| CI 选定的 6 个 mock e2e（`-- --ignored`） | 全部通过 | 0 |

CI：运行 35085964805（push `28d4bf7`）两个 job 的全部步骤成功——其中 clippy 与 `cargo test` 步骤是本次修复后首次真正执行（此前被 rustfmt 步骤挡住从未运行）。

## 已知局限（不得当作已覆盖）

- 前端没有组件级行为测试：`tools/frontend-regression.mjs` 是状态机级回归（26 项），不检查过渡/动画/可见性细节，也不使用任何用户可见中文串做断言。
- 静帧截图无法证明动态过渡：动画类交付必须区分「代码路径核对」与「实机操作观察」两类结论。
- 依赖本地 mock 服务器的 e2e 默认 `#[ignore]`（sync_e2e、sync_phases_e2e 等），需显式 `-- --ignored` 才运行；CI 有单独步骤覆盖。
- 沿用批次 4 基线的未决项：P2-9「Miniflux 兜底路径」确认未实现——`db::feeds_origin_remote` 与 `db::feeds_fetch_failed_bound` 仅被 re-export，全仓无调用点，`SyncReport.fallback_entries` 恒为 0。
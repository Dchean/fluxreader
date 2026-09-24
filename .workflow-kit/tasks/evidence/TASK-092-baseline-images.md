# TASK-092 基线：内容图片位取图方式与失败表现（2026-09-24，主控只读核对）

## 修前门禁（同一工作区，脚本 tmp/verify-20260924/run_gates.sh）

| 门禁 | 结果（照抄输出） |
| --- | --- |
| cargo fmt --check | exit 0 |
| cargo clippy --all-targets -- -D warnings | exit 0 |
| cargo test | 各二进制 test result 行求和 213 passed / 0 failed / 9 ignored，exit 0 |
| npm run lint | exit 0（oxlint：Found 0 warnings and 0 errors） |
| npm run build | exit 0 |
| npm run test:frontend | 「前端逻辑回归合计 348/348 通过」，exit 0 |

后端 77 个文件与已验收候选 TASK-091（9eacd271…）逐个 sha256 一致。

## 修前代码事实（Grep `proxyImageUrl|referrerPolicy|onError|fetchImage|report_broken_cover`，src/）

- `proxyImageUrl` 唯一组件调用点：`src/components/Timeline.tsx:565`（画廊卡片）。
- 直连 + `referrerPolicy="no-referrer"` 且无 `onError` 的内容图片位：
  - 文章卡封面 `src/components/Timeline.tsx:374`
  - 播客卡封面 `src/components/Timeline.tsx:685`
  - 迷你播放条 `src/components/PlayerBar.tsx:216`
  - 全屏播放器 `src/components/PlayerBar.tsx:332`
  - 灯箱大图 `src/components/Overlays.tsx:338`
- 全仓唯一图片 `onError`：`src/components/Sidebar.tsx:232`（favicon，不在本卡范围）。
- 前端对后端命令 `report_broken_cover`（TASK-091 新增并已验收）零调用。

## 已知限制（本卡不改）

- miniflux 源封面不参与补全（DEC-next-batch-covers-reachability-20260923 明确不做）。
- `first_image` 取图启发式（审计 round-4 §1-E）不在本卡。

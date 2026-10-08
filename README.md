# 存档分支：工具与调研报告

本分支**只有存档材料，不含 `src/` 项目代码**，也不参与构建与发布。它的存在是为了让两类东西在主线（`main`）清理后仍可查：

1. 一次性工具——为具体任务写过、任务结束后不再有调用方；
2. 整理前的调研报告原文——已按主题改写进主线的 `.agents/notes/` 与 `docs/`，此处保留写入时的原始形态（含当时的工作流编号 TASK-/REQ-/DEC-/RUN-），供追溯细节与核对改写是否失真。

`main` 分支同一次清理的提交信息里记录了本分支的来源提交。

## 目录

| 路径 | 内容 |
| --- | --- |
| `archive/tools/` | 一次性探针与测量工具 |
| `archive/reports/` | 整理前的审计、决定、发现、界面契约与自检报告 |
| `archive/measurements/` | 上述报告引用的原始测量数据与审查包 |

### archive/tools/

| 文件 | 用途 |
| --- | --- |
| `t059_cdp.mjs` | Chrome DevTools Protocol 连接与截图辅助（被 `t059_ui_*` 与 `phase4_measure.mjs` 复用） |
| `t059_ui_e2e.mjs`、`t059_ui_probe.mjs` | Endpoint 自动适配（TASK-059）的桌面 UI 实机验证与侦察 |
| `t059_check_real_db.py`、`t059_restore_db.py`、`t059_wal_recover.py` | 真实数据库检查、备份还原与 WAL 恢复辅助 |
| `phase4_measure.mjs`、`phase4_seed.py`、`phase4_checklist.md` | 大库性能测量电池、合成规模数据注入与 owner 实机操作清单 |
| `mock_greader_ui_server.py` | 模拟 FreshRSS / Miniflux 两种 API 布局的本地后端，供实机验证「只填域名即可连接」 |
| `task-spec-guard.py`、`task-spec-guard-test.py` | 为绕开已删除工作流引擎的 prepare 缺口而写的任务规格守卫（13 例自测）；守卫对象随该工作流一并消失，故移到此处 |

### archive/reports/

审计 6 份、决定 4 份、发现 4 份、界面契约 16 份、自检 3 份、技术评估 2 份、测试基线与发布模板各 1 份，以及 5 份工作流工具自身的缺陷记录。

`BASELINE.md` 里的**提交号对照表**是解读 2026-09-16 历史重写前旧提交号的唯一线索（重写工具未留映射），故一并留在本分支；主线 `docs/roadmap.md` 说明了它的位置。

### archive/measurements/

按报告分组：`audit-20261007/`（第三方审计原文 REVIEW.md、行为探针与探针结果）、`phase4/`（测量电池的原始 JSON 输出与探针脚本）、`refactor-20261005/`（各任务的 spec、审查包与审查报告）。

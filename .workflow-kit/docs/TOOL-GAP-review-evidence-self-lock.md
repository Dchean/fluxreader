# 工具缺陷：审查证据自锁（review evidence changed after approval）

日期：2026-09-16（本地 09-17）。范围：仅工具脚本，不含业务代码。

## 现象

`review_quality_digest`（project_workflow.py:339-378）会把 `review_checks[].evidence_files`
里的**每个非候选文件按 sha256 绑定**进摘要，`check_project`（:772-773）随后重算并比对，
一旦不一致就报 `TASK-xxx: review evidence changed after approval`。

问题在于：`review()` 自己会改写其中一类文件。执行顺序是

1. `review_quality_digest(...)` 先算摘要（:1016）——此刻任务记录还是旧内容；
2. 紧接着 `start_run()` → `task_save()` 把 `status`、`review_run` 等写回
   `.workflow-kit/tasks/items/TASK-xxx.json`（:1017、:1037）；
3. 最后 `check_project`（:1038）重算摘要——同一文件已被自己改写，于是必然不一致。

也就是说：**只要审查报告把 `.workflow-kit/tasks/items/TASK-xxx.json` 列进 `evidence_files`，
这次审查就永远无法通过**，且与报告结论对不对无关。

本次实际触发：TASK-042 第二轮独立审查（结论 PASS、0 findings）把该任务记录列为证据，
记录时被判上述错误。

## 为什么容易被踩到

`review-packet` 交给审查者的输入里就包含完整任务记录（`task_packet.task`），
审查者自然会引用它作为「需求与验收标准」的证据。项目里其它任务没踩到，只是恰好没把
任务记录列进 `evidence_files`（例如 TASK-041 的 19 个证据文件里，任务记录与卡片为 0 个）。

## 不可恢复

失败后 `review()` 走 :1040，把任务置为 `status=blocked, failure_kind="evidence"`。
而 `begin`（:729）、`verify`（:939）与重试循环（:1357）的放行集合都不含 `evidence`，
`recover` 又只接受 `outcome=running` 的 run。于是它与「protocol」一样是**不可自愈的终态**，
`next` 只会给出无法执行的建议。

## 本次的处置（owner 授权的台账订正）

1. 让原审查者在**不改结论、不重跑门禁**的前提下重发报告：仅从 `evidence_files` 中移除
   该任务记录路径，其它证据保留，并说明原因；新报告写在独立文件里，旧报告原样留存。
2. 在 owner 授权下做一次**仅限记录**的订正：把任务从 `blocked/evidence` 恢复为 `review`，
   原任务记录与该 run 另存为 `…-task-asrecorded.json` / `…-run-asrecorded.json`，
   订正依据与失效证据路径记入 `run["ledger_correction"]` 与
   `task.evidence.ledger_corrections`。
3. 随后用修正后的报告重新记录审查。

代码、候选与审查结论全程未改动。

## 建议的工具修复（未实施）

择一：

1. **最小且对症**：`review_quality_digest` 在收集证据时，跳过「工具自己会改写的台账文件」——
   即 `.workflow-kit/tasks/` 下除 `evidence/` 以外的路径（`items/**`、`cards/**`、
   `PROJECT.json`、`DECISIONS.json`、`BACKLOG.md`、`IN_PROGRESS.md`、`PROJECT_STATE.md`）。
   它们不是证据，只是账本。
2. 或在 `review()` 里把摘要计算挪到自身写盘**之后**再算一次，并让任务记录不参与绑定。
3. 或在 `review_packet` 的审查者指引中明确禁止把 `items/`、`cards/` 下文件列入
   `evidence_files`，并在校验时给出可读的报错而不是 digest 静默不一致。
4. 同时把 `evidence` 纳入 `begin` 的放行集合，避免这类记录问题直接报废任务。

## 边界

订正不放宽任何门禁：纠正后的报告仍须覆盖 5 个审查域与全部 ui_checks，仍需独立上下文，
候选仍须与实际文件一致。
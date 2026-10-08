# 工具缺口：failure_kind=protocol 没有任何 CLI 恢复入口

日期：2026-09-16（本地 09-17）。范围：仅工具脚本，不含业务代码。

## 现象

TASK-042 的修复轮在 `finish` 时被判 `failure_kind=protocol`（原因：worker 声明的 `changed_files`
漏列了被删除的 `src/hooks/useEnteringClass.ts`）。工作本身已经完成、代码也已提交，但账本停在
`blocked / protocol`，CLI 上无路可走：

- `RETRYABLE = {"test_failure", "review_failure"}`（workflow_runtime.py:31），**不含 protocol**；
- `begin` 的放行集合是 `RETRYABLE | {network, interrupted, environment, budget}`（:729），
  `verify` 同（:939）——`protocol` 不在其中，因此拒绝「Resolve this blocker explicitly before resuming」，
  却没有任何命令能清除它；
- `recover` 要求 `run["outcome"] == "running"`（:1515-1516），而 `block` 已把该 run 置为 `blocked`
  → 直接报 “Run is already closed; use next”；
- `finish` 要求 `run["outcome"] == "running"` 且 `kind ∈ {implementation, repair}`（:813），
  已关闭的 run 无法重跑；
- `extend` 只追加时间与修复轮额度（:1542），**不**清除 `blockers` / `failure_kind`。

结论：`status=blocked` + `failure_kind=protocol` 是一个不可自愈的终态。`next` 只会给出
`resume_with=begin` 这类实际会被守卫拒绝的建议。

## 为什么不能靠「重新 finish」自救

`finish` 的 `changed` 由 begin 时快照与当前 `inventory()` 之差得出（:820-822）。而本次 begin 之后，
**`block` 与 `extend` 都会改写 `.workflow-kit/tasks/items/TASK-042.json`**（写 checkpoint 与扩展记录）。
任务文件既不在该任务的 `allowed_paths`（`src/**`、`tools/frontend-regression.mjs`），又落在
`protected_paths`（`.workflow-kit/tasks/**`）里，因此任何重跑都会先被 “Out-of-scope changes” 拦下，
**与 worker 声明是否补全无关**。也就是说：只要 `block` 写过一次 checkpoint，这条路径在结构上就不可达。

## 本次的处置（owner 明确授权的台账订正）

在 owner 明确授权（“授权台账订正，并继续”）下做了一次**仅限记录**的订正，源码与候选均未改动：

1. 把 worker 声明的 `changed_files` 由 7 补到工具自己观测到的 8 个（补上被删除的文件）。
   原始声明与原始 run 记录另存为
   `RUN-…-worker-result-asrecorded.json` / `RUN-…-run-asrecorded.json`。
2. 该 run 的 `outcome` 由 `blocked` 改为 `completed`；原始 `failure_kind` / `failure_reason`、
   原始 `finished_at_utc`、订正依据与授权来源全部存入 `run["ledger_correction"]`，不删除历史。
3. 任务回到 `verifying`，清除 `failure_kind` / `resume_phase`，同样在
   `task.evidence.ledger_correction` 留痕。
4. 订正后 `verify` 重跑，产出的候选摘要与订正前手工计算的一致（`f3f07446…`），
   可核对证明「只改账本、未改代码」。

## 建议的工具修复（未实施）

择一即可，且都应保留旧记录：

1. 把 `protocol` 纳入 `begin` 的放行集合，并让 `begin` 在重启时**重取** scope 快照——
   使「声明写错」不再等价于任务报废；
2. 新增显式的订正命令（`reconcile` / `finish --amend`），要求给出理由与授权来源，
   并强制归档旧记录；
3. 最低限度：让 `next` / `progress` 把 `protocol` 标注为「无 CLI 恢复入口，需 owner 决定」，
   而不是给出会被守卫拒绝的 `resume_with`。

## 边界

订正只补全声明，**不放宽任何门禁**：`verify` / `review` 仍需重跑，候选仍须与实际文件逐一一致，
独立审查仍不可省略，预算与截止时间沿用原记录。

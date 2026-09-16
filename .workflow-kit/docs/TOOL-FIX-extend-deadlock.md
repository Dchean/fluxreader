# 工具缺陷：预算追加（extend）不可达导致死锁

日期：2026-09-15（本地 09-16）。范围：仅工具脚本，不含业务代码。

## 现象

`extend`（预算追加）是工具自己给出的、针对“任务截止时间耗尽”的补救命令。但 `extend_budget` 在写入追加记录后调用 `must_check()`，要求 `check_project` 全绿；而 `check_project` 恰恰把“ACTIVE 任务超过截止时间”记为项目错误（project_workflow.py:674 `deadline exhausted; record blocked instead of continuing`），并把“运行在截止时间之后结束”也记为错误（project_workflow.py:681）。于是：

- 要修好预算，必须先让 check_project 通过；
- 要让 check_project 通过，必须先修好预算。

同理，`verify` 需要 `remaining_seconds > 0`，`review` 结束时需要 check_project 通过，都无法在耗尽状态下完成。

本次实际锁死：

- TASK-040：截止时间 2026-09-15T22:07:10Z 耗尽，且其 blocked 运行于 23:30:33Z 结束（在截止之后）。
- TASK-038：候选快照在验证后被后继任务 TASK-040 改动共享文件（src-tauri/src/commands.rs、src-tauri/src/db/sync_map.rs）而失效，但 `verify` 需要有效截止时间，`extend` 又被上述死锁挡住。

任何变更命令（extend / accept / prepare / begin）都会因 must_check 失败而回滚，流程无法自愈。

## 修复

`extend_budget` 不再要求项目全绿，改为只拒绝“由本次追加新引入”的错误：

1. 追加前采集 `check_project` 的既有错误集合；
2. 写入追加记录后再取一次错误集合；
3. 若出现不在既有集合中的新错误，回滚并向调用者报错；
4. 否则保留追加记录。

保留的安全性质：追加本身引入的任何不一致（例如扩展链不满足 `previous_deadline -> max(previous, recorded) + minutes` 的校验）依旧会回滚；原始 `started_at_utc`、原始截止时间与全部运行历史仍然不被重写。

## 边界

该修复不放松任何业务门禁：任务仍必须走 finish -> verify -> review，候选仍必须与实际文件一致，仍需独立审查，仍需 owner 明确来源才能追加预算或验收。它只解除“补救手段要求问题已解决”这一自指死锁。

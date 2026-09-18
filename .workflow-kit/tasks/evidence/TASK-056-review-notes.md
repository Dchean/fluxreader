# TASK-056 审查关注点与遗留项（原 worker-result 的 requested_actions / unresolved_items）

> 说明：worker-result 契约规定 `requested_actions` / `unresolved_items` **非空即触发
> `failure_kind=action_required` 阻断**（`workflow_runtime.py:855`）。这两个字段的语义是
> 『worker 要求管理者行动』，**不应用于承载审查建议或遗留项清单**。
> 故在 owner 授权的记录级订正（DEC-task056-action-required-unblock-20260918）中将其清空，
> 原文逐条转存于此，**内容未丢失、未修改**。

## 原 requested_actions（审查关注点）

1. 审查请重点核对：(1) 零 folder 新库场景是否真实成立（请确认测试确实清空了 mock 的 folders 且订阅无分类，否则该测试会空转——这正是作者第一次写错的地方）；(2) fail-before 是否可自行复现（回退兜底或一并回退兜底+上报，注意 cargo mtime 指纹需强制重编译确认 Compiling app）；(3) 同类吞错排查的『保留』判定是否合理（特别是 greader_pull/fever_pull 的 if let Ok(n) 状态标记，请独立判断 0 行是否为正常语义）；(4) entries.rs 的同类改动是否引入行为回归

## 原 unresolved_items（遗留项）

1. db::get_first_folder_id 在本任务后生产代码已无调用点（仅剩其自身单测引用）；因是 pub 且被 db.rs re-export，clippy 不报 dead_code。未删除——属独立清理决策，如实记录供后续判断
2. Bug 1（endpoint 填法与失败提示）为同批次 TASK-057，不在本任务范围
3. 未验证真实 FreshRSS 实例的端到端拉取（demo.freshrss.org 的演示凭据不可得，ClientLogin 回 401）；本任务的验证基于 mock 复现 + 用户真实库结构的只读复制件
4. TASK-054 遗留的 P1/P4 弱断言测试仍未补强（独立后续项）

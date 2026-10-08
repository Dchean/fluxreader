# 决策：TASK-100 自检遗留项收口（DEC-task100-selfcheck-remaining-20260929）

- issuer: owner
- status: accepted
- source: owner 2026-09-29 会话指示原话：「修复发现的问题并打tag发布，现在可以使用子代理了，记得按工作流派发子代理审计」
- recorded_at_utc: 2026-09-28T22:34:26Z（见 tasks/DECISIONS.json 注册表条目）

## 决定

1. 三路发布前自检报告（.workflow-kit/docs/selfcheck-20260928/）中 v0.15.0 未修复的遗留项
   全部纳入 TASK-100 收口：前端 P2×2 + P3×9、UI P2×2 + P3×17、Rust P3×7，
   另含 base.css 历史注释乱码重建（73 行/114 处 U+FFFD）。
2. 流程：按 workflow-kit 立卡（本卡）执行——实现复用同领域自检子代理实例
   （owner 复用规则），完成后 finish → verify → 全新独立审查子代理 → 主控验收提交；
   随后发布 v0.16.0：bump 提交单独推送、等 CI 绿、再打 annotated tag（吸取 v0.15.0
   tag 门控错位教训）。
3. 明确不做（记录在案，报告即依据）：
   - 跨布局 J/K 键盘导航（产品决策，本轮只改准快捷键页文案 + 卡片 role/tabIndex 对齐）；
   - --text-tertiary 对比度 token 调整（全局视觉变更需单独决策，仅记录真机复核建议）；
   - get_setting 敏感键边界（R-P3-10，报告结论为不改）；
   - 命令壳测试覆盖清单（R-P3-11，记录为待办池）；
   - 流式翻译 delta 消毒（R-P3-9）：先评估——逐 delta 过 sanitizer 可能切断跨 delta 的
     标签；若无法在不破坏流式渲染的前提下安全实现则记录跳过（CSP script-src 'self' 兜底在案）。
4. 行为变化口径：全部为缺陷修复与既有契约内的一致性统一（REQ-002/004/006/008），
   无破坏性变更；版本号 0.15.0 → 0.16.0（次版本，承载体量较大的修复批次）。

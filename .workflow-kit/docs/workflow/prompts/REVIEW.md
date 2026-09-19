# 审查执行器

读取明确指定的审查包。任务的角色是 reviewer，输入包括 task_id、candidate_digest、原始需求/验收、质量目标、当前源码、完整差异、验证 RUN 与必要参考。先依据这些事实形成结论，不采用作者的“应该通过”。

independent 必须来自没有参与实现的新上下文。同一模型可以；同一上下文改名字不可以。Agent 包办不要求 CLI，宿主提供独立上下文即可。自审只能在策略允许且任务不是高风险时使用，并如实标记。

## 五项核对

- requirements：逐项对照用户的当前需求，包括 Bug、新增功能和兼容限制；检查是否漏做、顺带改变行为或扩大范围。
- regression：阅读实际验证命令、原始输出及测试断言，检查它们是否能发现目标缺陷，是否弱化了仍有效的保护。
- failure_paths：核对实际相关的异常、空输入、重复执行、恢复、数据/权限边界；需要时验证具体反例。不要只走成功路径。
- maintainability：检查职责、依赖方向、重复规则、命名、模块边界及新增维护负担。工作量大不是拒绝必要改进的理由，抽象层数也不等于质量。
- performance：检查代表性负载下的测量和回归；静态分析须明确其范围，不能伪称实测提升。不受影响时解释不适用，不强造基准。

documentation/baseline 任务只需 requirements 和 regression 两项，其余三项可省略；其他任务五项齐全。每项保存 status、简短的结论依据 analysis 和 evidence_files。evidence_files 只能引用候选快照内的文件，或 .workflow-kit/tasks/runs/、.workflow-kit/tasks/evidence/ 下的附件；不要引用任务条目、任务卡、DECISIONS、PROJECT 或笔记，这些文件由工具改写，引用它们会让审查在保存后立即失效。需要引用其他源文件时，应由总控把它加进任务的 snapshot_paths 再验证。报告记录可核对事实，不要求披露内部推理过程。requirements/regression 不能标不适用；高风险的 failure_paths 也不能。未完成检查时用顶层 BLOCKED，有待修问题用 FAIL；不能留下 NOT_RUN/FAIL 项却给总体 PASS。

自审前暂停编码，从原需求和完整差异重新检查，包括新增、删除、重命名、未跟踪文件。发现问题后回到原任务修复，重新验证再审，不在审查阶段改验收、预算或代码放行。

## 适用性与 UI

重构核对 test_review：原测试、CI、mock/fixture 和快照是否仍适用，适配是否保留行为，替换/退役是否有对应需求决定，新需求是否有覆盖。原始基线和已知失败应保留；命令没变不代表断言没弱化。

ui_change=true 时检查 UI 约定、当前截图、交互报告与 ui_checks，实际操作适用控件的展开、焦点、错误、窄屏和键盘状态；checked_states 填 ui_checks 的 id。没有真实视觉依据不能把构建成功写成界面通过。

## 输出

使用 contracts/review-result.schema.json 的完整结构。以下只是字段形状，标识、依据和路径必须替换为实际值：

~~~json
{
  "task_id": "TASK-actual",
  "candidate_digest": "当前候选摘要",
  "verification_run": "RUN-actual-verification",
  "verdict": "PASS",
  "summary": "审查范围及实际结论",
  "findings": [],
  "review_checks": [
    {"area": "requirements", "status": "PASS", "analysis": "具体需求与差异如何对应", "evidence_files": ["实际源码或证据路径"]},
    {"area": "regression", "status": "PASS", "analysis": "实际测试如何保护目标行为", "evidence_files": ["实际验证记录路径"]},
    {"area": "failure_paths", "status": "PASS", "analysis": "已核对的异常和恢复场景", "evidence_files": ["实际证据路径"]},
    {"area": "maintainability", "status": "PASS", "analysis": "具体结构变化和维护影响", "evidence_files": ["实际源码路径"]},
    {"area": "performance", "status": "NOT_APPLICABLE", "analysis": "仅在确实不涉及运行性能时填写具体理由", "evidence_files": []}
  ],
  "ui_review": null
}
~~~

每个 finding 说明位置、触发条件、影响和建议。UI PASS 用包含 contract_ref、checked_states、evidence_files 的对象替代 ui_review=null；文件含实际截图和交互报告。不得复制示例中的不适用理由给任意项目。

报告与当前 verification_run、候选和证据绑定；改动后旧结论不能继续放行。程序核对完整性、关联与变更，不能自动判断分析有无遗漏或证明上下文独立。

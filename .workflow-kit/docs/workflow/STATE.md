# 状态与记录契约 v1

先用接入回执确定当前状态根；新布局位于 .workflow-kit/，经典布局位于原位置。仅有同名 PROJECT.json 不表示符合本契约。绑定记录保存工作流身份、受管理文件摘要和旧来源，不能拿 connected 代替业务授权。

PROJECT 只保存项目阶段和当前任务/批次引用。POLICY 是角色、权限和预算的权威。每个 TASK 只有一个 `status`；不要再加与其冲突的 ready_status / completed / verified 布尔值。文件格式以 `.workflow-kit/tasks/templates/` 为例，模板本身不参与任务调度。

BRIEF 保存分轮问答，confirmations 将真实回答及来源与相应数值绑定；建议保持 proposed。POLICY.intake 引用本轮已确认 brief 所在的用户决定。新增批准需要完整问答，已有项目需有意向、分析及用户选定路线；执行器不能静默切换。旧版已批准记录不清空，补齐缺失选择后再扩展范围，见 [INTAKE](INTAKE.md)。

BRIEF.requirements 是主目标和补充需求的统一清单，in_scope 区分纳入和暂缓，任务用 requirement_refs 关联；quality 记录维护、稳定与性能安排。批准后的 BRIEF 与决定快照一致，不能静默删改。progress、原生面板和 PROJECT_STATE 都从已有事实生成，不作为另一份状态。

## 笔记

notes/JOURNAL.md 是只追加的项目日志：onboard、research、prepare、检查点、阻塞、解锁、验收、续批、追加预算和 Agent 的 note 各占一行。notes/RESUME.md 由记录、检查点和日志生成，供接手者先读；两者都在受保护路径内，工具在改动比对时忽略它们。blocked 任务的处置写入 task.dispositions，取消写入同一字段；review 运行的派生摘要重算记录在 run.recomputed。

## 阶段与任务

项目阶段：intake → discovery → baseline（重构通常需要）→ delivery → release → complete；paused 表示主动停下。新项目可以从 discovery 进入 delivery 建立第一套代码与测试。阶段改变必须有已有授权，不以编辑 stage 代替批准。

任务状态：draft → ready → running → verifying → review → verified → done；blocked / cancelled 是明确分支。

- draft：允许未决字段，不可运行。
- ready：需求、依赖、风险、输入快照、范围、验收、门禁、权限与预算完整；blockers 必须为空。
- running / verifying / review：分别记录实现、工具验证和审查；只有明确问题才进入 blocked。
- verified：适用验证与审查通过，证据绑定当前候选；还没代表合并或发布。
- done：所属交付已验收，所需合并已有证据或确实不适用。
- blocked：必须有原因和恢复动作；解阻后重新核对原记录，不能换 ID 清零。scope/protocol/action_required/evidence 用 unblock 记录处置，其他类别按 run/begin 的既有规则接续。
- cancelled：由 cancel 记录理由与来源；有未完成依赖任务时不能取消。
- done 的可选严格门禁：POLICY.acceptance.require_committed_evidence=true 时，验收记录必须已被 git 跟踪且无未提交改动；accept 本身放行一次并返回 commit_required 清单，之后的 check/next 持续报告直到提交。

## UI 与预览确认

POLICY.ui.mode 保存 none / existing / preview_first；PROJECT.ui_preview_task 指向当前预览任务。preview_first 在该任务实际 accept 前阻止正式实现，文档、基线和 UI 预览可先进行。预览 verified 不能代替用户确认；feedback 将尚未接受的预览交回原任务修复，不清空时钟和历史。

ui_change、ui_contract_ref、ui_checks 和 retry_safe 属于冻结定义。ui_checks 条目可以是 "select.open" 这样的 id 字符串，或 {"id", "description"} 对象；审查的 checked_states 按 id 匹配，描述措辞可以自由写。UI 任务的候选包含约定文件；审查记录 ui_review 的截图、交互报告和覆盖状态摘要。已确认约定改变会重新触发预览确认；纯后台任务不要求视觉产物。

## Ready 条件

POLICY 的 approval 必须引用本项目真实的已接受用户决定。角色、审查模式、必要宿主能力、任务及批次预算已确定；项目阶段允许这类任务。数据、网络、依赖、CI 等权限分别判断。

任务必须有 objective、requirement_refs、acceptance、非空 allowed_paths、适用 gates 和具体输入快照；依赖已 verified/done 且产物在当前候选中可用。scope 使用项目相对路径，拒绝绝对路径和 `..` 越界。

风险 high 必须引用适用的用户决定，不能仅由管理者写 low 来改变实质风险。准备任务包时冻结范围、验收和门禁定义的哈希；实现者只获得获准文件的写入权。

已有项目的实施任务还需要 test_review：区分保留行为与需求变化，引用原始基线证据，将 keep/adapt/add/replace/retire 映射到本任务门禁。变化/删除业务契约需要覆盖相应任务或需求的用户决定。记录随任务定义冻结；基线附件加入快照并受保护。纯文档和基线采集可先进行，具体语义见 [GATES](GATES.md)。

## 输入、候选与证据

`input.manifest` 指向任务开始前的快照，`input.digest` 为其 digest。实现后另建 `evidence.candidate_manifest` / `candidate_digest`，不要覆盖输入快照。辅助工具的 snapshot 输出包含明确 roots 与逐文件哈希；snapshot 只覆盖选择的根，仍需另外检查全仓修改、删除、重命名、未跟踪文件。

验证运行的 kind 为 verification，checks 包含 gate_id、status、exit_code、log 和 candidate_digest。必需门禁只有 PASS 且退出码 0 才满足；不适用项目在派发前标 required=false 并解释理由，不能看到失败后临时改为不适用。

审查运行的 kind 为 review，review 包含 mode（independent / self_review）、verdict、report 和 candidate_digest。independent_required 策略不能被 self_review 满足。独立审查须有分开的上下文证据；脚本不能替你证明上下文独立或审查有效。

task.evidence 只引用 verification_run / review_run 和候选，不抄另一套测试结果。verified/done 的验证与审查摘要必须相同；未经新任务验证的相关改动使证据失效。

后续增量可通过 dependencies 延续同一源码。prepare 将前序快照根和必需回归命令并入新任务，冻结 continuation_of；begin 在前序 evidence 中记录 continued_by。前序 PASS 成为明确的历史记录，最新任务对当前组合版本负责。延续关系不得丢弃前序快照或检查，验收不能只引用过时的前序任务。先验证依赖再 prepare 下一张卡，避免提前冻结输入；总待办范围保存在 PRODUCT/BRIEF。

必需门禁默认继承；唯一例外是经过 test_review 校验、准确指明 from_task/from_gate 的适配、替换或退役。适配/替换必须有新的必需门禁，业务契约变化还须引用相应用户决定。未涉及的回归和原 RUN 保留，不能只编辑后继 gates 把失败检查丢掉。

## 运行历史与预算

每次实际调用或验证都有唯一 RUN ID 和 task_id，kind 为 implementation / repair / probe / verification / review。原始 stdout/stderr、进程结果和脱敏环境信息作为附件；按真实进程记录填写 UTC 起止时间，不从 ID 或估计值构造时间。

网络连续失败次数从同一阶段的原始 RUN 计算；退避时刻来自原失败结束时间，不因换会话推后或清零。只有任务声明可以安全重复、现场未变且原进程退出时才自动重试。CLI 单次时间上限与任务总上限同时有效，见 [RECOVERY](RECOVERY.md)。

同一任务的所有运行记录都必须出现在 run_ids；接手方同时扫描 runs/ 防止遗漏。implementation 是首次实现；重放也记录为新的 implementation，不能抹掉旧调用。repair 只表示反馈代码/审查缺陷后的修复轮；probe 单列但照样计入墙钟时间和可取得的费用。

首次开始时持久化 task.budget.started_at_utc 和 deadline_at_utc。POLICY.budget.clock 决定时钟含义：active（默认）只累计实现/修复/探测运行的时长，等待、断网和只读门禁不计，额度 = max_task_wall_minutes + 各次 extend 的分钟；wall 为旧语义，按截止时刻计。两种时钟都只约束写入阶段；验证和审查过期后仍可运行。恢复不得重设。新决定可通过 extend 追加 extensions，记录原有效期限、追加时间/修复额度及决定 ID；原始字段不变，当前有效期限取最后一条扩展记录。repair_rounds_used 仍等于全部 repair 记录数。追加时间不改变美元限额。

批次 task_ids 与任务的 batch_id 双向一致；已启动任务不从批次移除以释放额度。达到上限后，由 batch 关闭已完成批次再开新批次。明确确认 batch_rollover=allowed 且 financial.mode=none 时 prepare 可自动续批；其余情况使用真实续批决定。自动推进只覆盖已经确认的产品范围。

accept 只接受列出的候选。所有已建任务 done 后仍需核对完整产品范围；只有用户确实确认项目交付，才使用 --project-complete 将 PROJECT.stage 置为 complete。

## 校验工具边界

审查记录 quality_digest 绑定原始报告、当前 verification_run、候选和引用证据。documentation/baseline 任务只强制 requirements 和 regression 两项，其余三项可省略；其他任务五项齐全。证据引用面是封闭的：候选内文件按候选清单取哈希，.workflow-kit/tasks/runs 与 .workflow-kit/tasks/evidence 附件按当前字节取哈希，其他路径拒绝。五项 review_checks 不完整或无实际依据时不能 PASS；高风险必须 independent。这个摘要能发现遗漏字段或变动，不能认证宿主的新上下文或证明分析没有遗漏。工具语义变更导致摘要过期时用 recompute 重算并留痕。

check 的错误以任务 ID 开头时归属该任务；变更命令只被全局错误和本任务的新错误阻止，其他任务的记录问题通过 start/next 的 record_errors 展示。

```text
python .workflow-kit/scripts/project_workflow.py check --root .
python .workflow-kit/scripts/project_workflow.py boards --root .
python .workflow-kit/scripts/project_workflow.py stamp
```

check 只读，检查结构、引用、状态、输入/候选快照与证据、预算及时间的一致性；不是通用 JSON Schema 引擎、权限沙箱、产品测试或用户身份认证。boards 只写带生成标记的三个看板；首次遇到人工写的同名文件拒绝覆盖。stamp 产生真实当前时间与唯一运行 ID，不能替代进程的实际起止记录。

填写具体门禁和结果的字段示例见 [TOOLING](TOOLING.md)。费用校验只能发现已记录估算超限；缺少用量不等于零费用，管理者应在继续付费调用前确认可用的限制能力。

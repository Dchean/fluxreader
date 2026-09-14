# Agent 使用的工具命令

用户只回答问题和验收成果。下列命令由 Agent 运行，JSON 输入也由 Agent 根据真实决定准备。需要 Python 3.10+ 标准库，无额外依赖。

首次使用完整包的 workflow.py bootstrap；接入后使用回执给出的 helper 路径，默认是 .workflow-kit/scripts/project_workflow.py。状态文件也以回执为准，不能猜测旧任务目录。所有路径有空格时加引号，每条命令支持 --help。

## 常用命令

| 命令 | 实际行为 |
| --- | --- |
| bootstrap --root --kind [--host] [--write --source] | 预览或建立独立接入，保留旧文件，给出可核验回执；已有绑定只检查 |
| start / next | 只读返回当前阶段、恢复线索和下一步；不启动聊天窗口 |
| doctor | 只读检查目录、Python、Git、可用 CLI；不调用模型 |
| init | 预览空目录初始化；加 --write 才写入，冲突整体拒绝 |
| adopt | 预览已有项目接入；加 --write 才写入，保留现有代码和文档 |
| onboard --file --source | 保存已确认需求、执行选择与真实决定依据 |
| research --file | 保存参考清单、历史与可读调研报告 |
| prepare --file | 生成任务、输入快照、批次关联和卡片；检查定义与依赖 |
| begin --task --context | 宿主 Agent 开始一项实现，先落盘 RUN、时钟与范围快照 |
| finish --run --file | 核对实现结果与实际改动，保存候选；不会直接标通过 |
| dispatch --task | 使用已选择的 CLI 执行一次编码 |
| verify --task | 实际运行预先定义的测试，保存 stdout/stderr、退出码和候选 |
| review --task --file --context --mode | 记录真实审查报告；mode 为 self_review 或 independent |
| feedback --task --note --source | 记录尚未接受的 UI 预览反馈，在原任务与预算内继续修复 |
| review-cli --task | 在新的 CLI 进程中审查当前候选 |
| run --task | 推进单任务；有限修复和安全网络退避重试，无进展时返回 replan |
| checkpoint --task --note --next-action | 保存已观察事实与下一步，更新任务卡和状态页 |
| recover --run --source | 确认进程退出后关闭中断记录；省略 --run 可恢复孤立控制器锁 |
| extend --task --minutes --source | 依据新决定追加任务时间；可加 --repair-rounds，原时钟与费用限额保留 |
| batch [--source] | 关闭已完成批次并开启下一批，保留全部成员和记录 |
| accept --tasks --source --merge-ref | 记录用户对具体候选的验收；完整项目获确认后加 --project-complete |
| check | 只读核对状态、定义、依赖、历史、证据和预算 |
| cards | 重新生成任务卡、三个看板和 PROJECT_STATE，不覆盖手写文件 |
| boards | 校验后重新生成三个看板 |
| snapshot / definition / stamp | 底层快照、定义摘要和时间辅助；普通任务由 prepare/begin 自动处理 |

## 起步

~~~text
python "【启动包】/workflow.py" bootstrap --root "【目标项目】" --kind refactor --host workbuddy
python "【启动包】/workflow.py" bootstrap --root "【目标项目】" --kind refactor --host workbuddy --source "实际用户选择工作流的指令" --write
python .workflow-kit/scripts/project_workflow.py start --root "【目标项目】"
~~~

新项目将 kind 换成 new。bootstrap 不带 --write 只预览，不能把预览的 ok 当作已连接。默认生成必要文档，--full-docs 才生成额外模板。初次接入另一个项目使用完整包；已接入项目可由其配套工具核对状态。

init/adopt 保留经典布局兼容，但不是识别遗留工作流的默认入口。只有回执显示经典布局时才使用根目录下的相应脚本/任务路径；不要用它们覆盖旧记录或重复初始化。

## 先评估是否值得重构

没有自动判断重构价值的评分命令，主 Agent 按 [REFACTOR](REFACTOR.md) 结合业务和证据给结论。显式使用本包时先接入；仅参考模式需要用户明确选择。kind=refactor 表示已有项目路线，不代表重构方案已批准。

需要持久评估任务时，按实际授权使用文档或 baseline 任务，未批准的业务代码保持 authority.code=false。原测试失败是评估数据，不把它直接设成会驱动自动修复的通过门槛。用户无需编辑这些配置。

存在旧工作流来源时，onboard 必须有 legacy_review。按回执列出的路径读取后，记录保留约束、旧任务处置、当前角色/流程结论和未决冲突；不能复制旧批准替新的工作授权。

评估阶段已经 onboard 后，不再运行一次 onboard。执行范围得到实际授权时，由主 Agent 追加真实决定，更新必要的 POLICY 授权引用和 PROJECT 阶段；保留原任务、时间与证据，预算追加/续批仍按既有命令处理。没有授权变化就沿用现有策略，不制造重复确认。

## 宿主 Agent 的一个任务

~~~text
python .workflow-kit/scripts/project_workflow.py onboard --file .workflow-kit/tasks/evidence/confirmed-brief.json --source "真实会话决定的引用"
python .workflow-kit/scripts/project_workflow.py research --file .workflow-kit/tasks/evidence/research-input.json
python .workflow-kit/scripts/project_workflow.py prepare --file .workflow-kit/tasks/evidence/task-spec-001.json
python .workflow-kit/scripts/project_workflow.py begin --task TASK-001 --context "实际作者上下文标识"
~~~

随后 Agent 修改任务允许的代码，准备符合 worker-result 契约的结果，再运行：

~~~text
python .workflow-kit/scripts/project_workflow.py finish --run "上一步的真实RUN编号" --file .workflow-kit/tasks/evidence/worker-result-001.json
python .workflow-kit/scripts/project_workflow.py verify --task TASK-001
python .workflow-kit/scripts/project_workflow.py review --task TASK-001 --file .workflow-kit/tasks/evidence/review-001.json --context "实际审查上下文标识" --mode self_review
python .workflow-kit/scripts/project_workflow.py next
~~~

self_review 仅在政策允许时使用。JSON 形状见 .workflow-kit/tasks/templates/BRIEF.json、TASK-SPEC.json、RESEARCH.json，以及 .workflow-kit/docs/workflow/contracts/。这些路径在生成的目标项目中存在。输入文件放在 .workflow-kit/tasks/evidence/，避免结果文件本身混进产品差异。

## 有界面的项目

onboard 保存 ui.mode=preview_first/existing/none。有界面的新项目先准备 kind=ui_preview，提供 ui_contract_ref 和 ui_checks；通过测试、真实视觉审查后展示给用户。使用 feedback 记录需调整的内容，使用 accept 记录实际确认，随后才能准备正式实现任务。

UI 变更用 ui_change=true，review 报告提供 ui_review：约定路径、checked_states 和 evidence_files。状态必须覆盖任务声明，文件必须包括实际截图和交互报告；构建日志不能替代它们。正式实现不得擅自改动已确认约定。详见 [FRONTEND](FRONTEND.md)。

依赖任务通过后才 prepare 下一张卡。dependencies 会继承前序候选的快照根与必需回归命令；当前范围不自动扩大写权限。任务粒度以一个可验证增量为准，不为每个小编辑建卡。全部待办范围保留在产品文档，逐项转成可执行任务。

## 恢复与预算

先运行 start/next。仍有活动进程时检查日志并等待；不能启动第二个写入者。只有确认进程已退出，才用 recover 关闭未结束记录。未分配 RUN 的孤立锁也可以恢复。

~~~text
python .workflow-kit/scripts/project_workflow.py recover --run "真实RUN编号" --source "已经核对进程、文件和日志的记录"
python .workflow-kit/scripts/project_workflow.py recover --source "孤立控制器已退出的检查记录"
python .workflow-kit/scripts/project_workflow.py extend --task TASK-001 --minutes 90 --repair-rounds 1 --source "用户明确同意追加额度的决定"
~~~

追加预算保留原始截止时间，通过扩展记录给出新的有效期限；若旧期限已过，从本次决定的记录时间起追加。不能用示例 source 文本代替真实授权。原始费用限额不会随时间追加而改变。

retry_safe 是准备任务时冻结的重复执行判断，默认 false。可重复任务的短时网络错误由 run 自动按原始 RUN 计数并退避，默认额外两次，等待 5、15 秒；不是无限恢复。CLI 单次默认 600 秒且受原任务剩余时间限制。连续相同失败而候选不变时返回 replan，由主 Agent 换方法，细节见 [RECOVERY](RECOVERY.md)。

每批任务数达到上限时，prepare 可复用 batch_rollover=allowed 且无额外美元上限的明确策略自动续批。其余情况先取得具体续批决定，再运行 batch --source。不能借续批清空未完成任务或未知消费。

## 验收

按可运行功能请用户验收，accept 记录其实际决定。没有 Git 合并时，merge-ref 使用说明原因的 not_applicable 值。延续同一源码的任务需包含最新组合候选；只接受过时的前序候选会被拒绝。

已建任务全部 done，只代表这些任务结束。核对 BRIEF 的全部范围后，才用 accept --project-complete 记录项目级验收。

## 实际边界

进程使用参数数组与 UTF-8 输入，记录真实退出并处理超时。范围检查主要是事后检查，不是操作系统安全沙箱。一个项目同一时间只支持一个运行者；没有常驻服务或自动跨会话调度。

金融模式 none 表示不由本工具附加美元硬上限，不表示免费。limit 模式需要执行器支持可核对的调用上限；历史用量未知时阻止继续受限付费调用。不要通过更换认证或删除约束解决失败。

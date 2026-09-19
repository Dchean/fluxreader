# Agent 使用的工具命令

用户只回答问题和验收成果。下列命令由 Agent 运行，JSON 输入也由 Agent 根据真实决定准备。需要 Python 3.10+ 标准库，无额外依赖。

首次使用完整包的 workflow.py bootstrap；接入后使用回执给出的 helper 路径，默认是 .workflow-kit/scripts/project_workflow.py。状态文件也以回执为准，不能猜测旧任务目录。所有路径有空格时加引号，每条命令支持 --help。

## 常用命令

| 命令 | 实际行为 |
| --- | --- |
| bootstrap --root --kind [--host] [--write --source] | 预览或建立独立接入，保留旧文件，给出可核验回执；已有绑定只检查 |
| start / next [--format json/markdown/compact] | 只读返回当前阶段、恢复线索和下一步；日常用 --format compact，返回 read_next 指出本步只需读的文档 |
| resume [--format markdown/json] | 接手一页：紧凑进度、Agent 留下的判断与待办、需要用户决定的事、下一条可直接运行的命令、先读哪些文档 |
| progress [--format markdown/compact/json] | 只读生成可直接展示的进度；json 含 native_tasks、native_plan、compact、needs_user，供有面板的宿主同步 |
| review-packet --task | 只读生成独立审查输入，不复制作者检查点或会话；宿主 Agent 或 CLI 均可使用 |
| doctor | 只读检查目录、Python、Git、可用 CLI；不调用模型 |
| init | 预览空目录初始化；加 --write 才写入，冲突整体拒绝 |
| adopt | 预览已有项目接入；加 --write 才写入，保留现有代码和文档 |
| intake [--file] [--write] | 审计缺项并给出下一轮问题；--write 保存分轮答案，不批准业务工作 |
| onboard --file --source | 保存已确认需求、执行选择与真实决定依据 |
| research --file | 保存参考清单、历史与可读调研报告 |
| prepare --file | 生成任务、输入快照、批次关联和卡片；检查定义与依赖 |
| begin --task --context | 宿主 Agent 开始一项实现，先落盘 RUN、时钟与范围快照 |
| finish --run --file | 核对实现结果与实际改动，保存候选；changed_files 可写 "auto" 由工具按 diff 填入 |
| dispatch --task | 使用已选择的 CLI 执行一次编码 |
| verify --task | 实际运行预先定义的测试，保存 stdout/stderr、退出码和候选；快照远大于实际改动时返回 snapshot_hint 建议收窄 |
| review --task --file --context --mode | 记录真实审查报告；mode 为 self_review 或 independent |
| feedback --task --note --source | 记录尚未接受的 UI 预览反馈，在原任务与预算内继续修复 |
| review-cli --task | 在新的 CLI 进程中审查当前候选 |
| run --task | 推进单任务；有限修复和安全网络退避重试，无进展时返回 replan |
| checkpoint --task --note --next-action | 保存已观察事实与下一步，更新任务卡、状态页和项目日志 |
| note --kind context/decision/todo/lesson/progress --text [--task] | 追加一条 Agent 笔记到 notes/JOURNAL.md，并重新生成 notes/RESUME.md |
| diff --run | 只读列出该运行基线以来的实际改动，按范围内/受保护/越界分组；changed_files 直接取它的结果 |
| unblock --task --source --note [--to ready/verifying] | 记录阻塞处置后让任务回到可执行阶段；保留身份、时钟、历史，不新建任务 |
| cancel --task --reason --source | 作废任务并记录理由；有未完成的依赖任务时拒绝 |
| recompute --task --source | 在有审计依据的工具变更后，用当前函数重算审查派生摘要，不手写摘要 |
| rebind --source [--upgrade-tools] | 入口块或工具被审计修改后重新核验绑定；--upgrade-tools 从新版启动包升级引擎、契约、提示词、流程文档、模板和入口块，记录不动，改动的文档先备份，活动运行存在时拒绝 |
| recover --run --source | 确认进程退出后关闭中断记录；省略 --run 可恢复孤立控制器锁 |
| extend --task --minutes --source | 依据新决定追加任务时间；可加 --repair-rounds，原时钟与费用限额保留 |
| batch [--source] | 关闭已完成批次并开启下一批，保留全部成员和记录 |
| accept --tasks --source --merge-ref | 记录用户对具体候选的验收；完整项目获确认后加 --project-complete。策略 acceptance.require_committed_evidence=true 时返回 commit_required 清单，提交后 check 才通过 |
| check | 只读核对状态、定义、依赖、历史、证据和预算；错误按所属任务分区，其他任务的记录问题不阻止当前任务 |
| cards | 重新生成任务卡、三个看板和 PROJECT_STATE，不覆盖手写文件 |
| boards | 校验后重新生成三个看板 |
| snapshot / definition / stamp | 底层快照、定义摘要和时间辅助；普通任务由 prepare/begin 自动处理 |

## 起步

~~~text
python "【启动包】/workflow.py" bootstrap --root "【目标项目】" --kind refactor --host "【当前宿主名称】"
python "【启动包】/workflow.py" bootstrap --root "【目标项目】" --kind refactor --host "【当前宿主名称】" --source "实际用户选择工作流的指令" --write
python .workflow-kit/scripts/project_workflow.py resume --root "【目标项目】"
~~~

新项目将 kind 换成 new。bootstrap 不带 --write 只预览，不能把预览的 ok 当作已连接。默认生成必要文档，--full-docs 才生成额外模板。初次接入另一个项目使用完整包；已接入项目可由其配套工具核对状态。

init/adopt 保留经典布局兼容，但不是识别遗留工作流的默认入口。只有回执显示经典布局时才使用根目录下的相应脚本/任务路径；不要用它们覆盖旧记录或重复初始化。

## 先评估是否值得重构

主 Agent 先问重构意向，按 [REFACTOR](REFACTOR.md) 分析证据，再提问让用户选路线。没有自动决定重构价值的评分命令。显式使用本包时核验接入；仅参考模式需要用户明确选择。kind=refactor 表示已有项目，不代表方案已批准。

需要持久评估任务时，按实际授权使用文档或 baseline 任务，未批准的业务代码保持 authority.code=false。原测试失败是评估数据，不把它直接设成会驱动自动修复的通过门槛。用户无需编辑这些配置。

存在旧工作流来源时，onboard 必须有 legacy_review。按回执列出的路径读取后，记录保留约束、旧任务处置、当前角色/流程结论和未决冲突；不能复制旧批准替新的工作授权。

评估阶段已经 onboard 后，不再次 onboard。得到实施选择时，按 [DECISIONS](DECISIONS.md) 追加真实决定及已确认快照，更新 BRIEF、POLICY.intake.decision_id、授权引用和必要阶段；保留原任务、时间与证据。预算追加/续批仍按既有命令。没有授权变化就沿用策略。

## 分轮问答与漏项检查

~~~text
python .workflow-kit/scripts/project_workflow.py intake --file .workflow-kit/tasks/evidence/answers-round-1.json --write
python .workflow-kit/scripts/project_workflow.py start
python .workflow-kit/scripts/project_workflow.py intake
~~~

输入允许只有本轮新增内容。回答和推荐分别保存，确认记录引用实际 source/answer；工具将其与数值绑定，变化后旧确认失效。未答完时也能保存并恢复，最多返回三个问题，不会反复问已答主题。refactor_intent 未确定先问意向；assess_existing 表示先做已授权的只读分析；有证据后再询问 refactor_decision。

ready_for_onboard 表示问答字段齐备；还要核对 legacy_review 和最终执行摘要。Agent/CLI、审查、UI、交付、预算和操作边界都不能由默认值充当同意。无界面及无真实服务的分支按 [INTAKE](INTAKE.md) 跳过不适用问题。脚本校验记录，不能认证一段引述确实来自用户。

requirements 收集主目标和 Bug/功能/其他补充，条目包含 id、kind、description、in_scope、acceptance；即使暂无补充也需要实际回答。quality 保存维护性、稳定性与性能目标；性能需要代表负载和目标，或具体的不适用理由。批准后的 BRIEF 与所引用决定快照必须一致，不能直接删除已确认需求。

## 向用户展示进度

~~~text
python .workflow-kit/scripts/project_workflow.py progress --root .
~~~

把结果直接展示在对话中；宿主有实际可用的原生计划/任务工具时，用 JSON 输出的 presentation.native_plan 和 tasks 同步面板。start/next 也返回 presentation。不存在通用的“写入 Markdown 就自动出现卡片”机制，不能伪称已在宿主面板显示。

PROJECT_STATE.md 使用相同数据，完整任务仍保存在原 JSON；进度视图不制造第二套状态。已建任务数不等于全部工作量，未拆分的已确认需求继续显示，最终完成检查会核对需求是否关联到已验收任务。规则见 [PRESENTATION](PRESENTATION.md)。

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

Agent 包办不强制自审。需要独立审查时先运行 review-packet --task，把中立输入交给未参与实现的新 Agent 上下文（同模型可用），再由总控以实际审查上下文 ID 记录 review --mode independent。高风险任务也要求独立。不能只改 --context 的字符串，也不能把作者会话复制后称为新审查。

所有通过报告提供 verification_run 和五项 review_checks：requirements、regression、failure_paths、maintainability、performance，每项给 status、analysis、evidence_files。PASS 必须有真实证据，未检查项不能靠空摘要放行。evidence_files 只能是候选快照内的文件，或 .workflow-kit/tasks/runs、.workflow-kit/tasks/evidence 下的附件；引用任务条目、卡片、决定或笔记会被拒绝，因为这些文件由工具改写，会让审查自我失效。报告与当前验证和候选绑定，规则及完整结构见 [REVIEW](prompts/REVIEW.md)。

## 已有项目的测试适用性

重构实施任务需要 test_review。先审查旧测试、CI/门禁和相关夹具，根据已确认需求分组记录处置。baseline.evidence_ref 指向实际、不可覆盖的基线附件或已结束 RUN；结果可以是 PASS、FAIL、BLOCKED 或 NOT_RUN，summary 解释范围、已知失败和缺口。记录不等于执行，也不把未运行写成通过。

例如用户已经确认改变某项行为，需要替换前序门禁时，可在任务规格中增加以下结构（标识和证据必须来自实际项目）：

~~~json
{
  "test_review": {
    "behavior": "change",
    "baseline": {
      "status": "PASS",
      "evidence_ref": ".workflow-kit/tasks/evidence/baseline-original.json",
      "summary": "旧排序检查通过；用户已选择新的默认排序，筛选行为继续保留"
    },
    "decision_ids": ["DEC-sort-order"],
    "actions": [
      {
        "target": "默认排序契约",
        "action": "replace",
        "reason": "已确认的新默认排序与旧断言冲突",
        "from_task": "TASK-001",
        "from_gate": "sort-order",
        "gate_ids": ["new-sort-order"]
      }
    ]
  }
}
~~~

DEC-sort-order 的 scope 须覆盖本任务 ID 或 requirement_refs 中的需求，source 引用实际回答；已存在的适用决定可以直接用，不重复询问。new-sort-order 必须是本任务 gates 中的 required=true 命令。新接手项目的旧检查还不是本包 TASK 时，target 指明实际测试/套件，省略 from_task/from_gate，说明与新门禁的映射即可。

behavior=preserve 表示原有行为保持，可新增覆盖或以 adapt 调整测试入口/夹具，无需再要求用户批准技术适配。replace/retire 用于改变/删除业务契约，需要对应用户决定；retire 的 gate_ids 为空，其他必需检查不能消失。纯新项目可继续用普通 gates；该字段默认 null，已有项目在实施前由 Agent 填写。

prepare 检查这份记录、固定原始基线附件并冻结计划，只对准确列出的前序门禁停止继承，其他检查继续带入。检查只能核对引用与结构；审查者还须判断新测试是否真正证明需求，没有删掉仍有效的保护。prepare 一次返回全部问题并给出字段的正确形态；决定的 scope 写成字符串会按单元素列表处理并在报错中显示双方实际值。已知会失败的门禁参数（如 `npm build`、`cargo run test`）在 prepare 阶段就被拒绝。

## 有界面的项目

onboard 保存 ui.mode=preview_first/existing/none。有界面的新项目先准备 kind=ui_preview，提供 ui_contract_ref 和 ui_checks；通过测试、真实视觉审查后展示给用户。使用 feedback 记录需调整的内容，使用 accept 记录实际确认，随后才能准备正式实现任务。

UI 变更用 ui_change=true，review 报告提供 ui_review：约定路径、checked_states 和 evidence_files。状态必须覆盖任务声明，文件必须包括实际截图和交互报告；构建日志不能替代它们。正式实现不得擅自改动已确认约定。详见 [FRONTEND](FRONTEND.md)。

依赖任务通过后才 prepare 下一张卡。dependencies 继承前序快照根和仍有效的必需回归命令；经 test_review 明确适配/替换/退役的门禁除外。历史原始结果保留，当前范围不自动扩大写权限。任务粒度以一个可验证增量为准，不为每个小编辑建卡。

## 阻塞的出口

失败类别有两族。test_failure / review_failure / network / interrupted / environment / budget 由 run 或 begin 按既有规则接续；scope / protocol / action_required / evidence 需要一次明确处置：先读任务卡最近检查点和 `<RUN>-changes.json`，撤销越界改动或确认归属，再运行：

~~~text
python .workflow-kit/scripts/project_workflow.py unblock --task TASK-001 --source "用户或总控的决定" --note "核对了什么、为何可以继续"
~~~

unblock 默认把任务放回 ready（重新 begin/finish），evidence 类默认回到 verifying；可用 --to 指定。解锁或修复轮的 begin 记住本任务先前的基线（中间没有其他任务运行时），diff 同时给出本任务累计改动 changed_files 和本次运行改动 changed_this_run，finish 接受其中任一列表。放弃任务用 cancel。

finish 的 protocol 报错会直接列出漏报（含删除）和多报的文件；先运行 `diff --run` 再填 changed_files 可以避免这类往返。工具自己改写的记录（任务条目、卡片、决定、日志、笔记）、各级目录 .gitignore 命中的路径（支持 ! 否定和目录规则）、常见构建目录都不计入改动。

## 控制上下文消耗

大项目最容易失败的不是某个命令，而是一个会话里读了太多东西。规则：日常轮次用 `next --format compact`，不贴完整 JSON、整份 BRIEF 或所有任务卡；start/next 的 read_next 只列本步需要的文档，其余按需再读；任务包和审查包只带本任务相关的需求、验收、兼容和质量目标；一个会话建议只做一张任务卡，每完成可恢复步骤就 checkpoint，影响后续判断的事实用 note 记下，然后新会话用 `resume` 接手。笔记和卡片都有长度上限：RESUME 最近 12 条事件、任务卡最近 8 个检查点、compact 十行。

## 恢复与预算

先运行 resume 或 start/next。仍有活动进程时检查日志并等待；不能启动第二个写入者。只有确认进程已退出，才用 recover 关闭未结束记录。未分配 RUN 的孤立锁也可以恢复。

~~~text
python .workflow-kit/scripts/project_workflow.py recover --run "真实RUN编号" --source "已经核对进程、文件和日志的记录"
python .workflow-kit/scripts/project_workflow.py recover --source "孤立控制器已退出的检查记录"
python .workflow-kit/scripts/project_workflow.py extend --task TASK-001 --minutes 90 --repair-rounds 1 --source "用户明确同意追加额度的决定"
~~~

追加预算保留原始截止时间，通过扩展记录给出新的有效期限；若旧期限已过，从本次决定的记录时间起追加。不能用示例 source 文本代替真实授权。原始费用限额不会随时间追加而改变。默认时钟按活动时间计（clock=active）：只有实现/修复运行在跑时才消耗额度，断网、等待用户和只读门禁不计；extend 追加的分钟直接加到额度上。时钟只约束写入阶段：过期后 verify 和 review 仍可对已有候选运行，begin 需要先 extend。任务卡显示已用/额度。

retry_safe 是准备任务时冻结的重复执行判断，默认 false。可重复任务的短时网络错误由 run 自动按原始 RUN 计数并退避，默认额外两次，等待 5、15 秒；不是无限恢复。CLI 单次默认 600 秒且受原任务剩余时间限制。连续相同失败而候选不变时返回 replan，由主 Agent 换方法，细节见 [RECOVERY](RECOVERY.md)。

每批任务数达到上限时，prepare 可复用 batch_rollover=allowed 且无额外美元上限的明确策略自动续批。其余情况先取得具体续批决定，再运行 batch --source。不能借续批清空未完成任务或未知消费。

## 验收

按可运行功能请用户验收，accept 记录其实际决定。没有 Git 合并时，merge-ref 使用说明原因的 not_applicable 值。延续同一源码的任务需包含最新组合候选；只接受过时的前序候选会被拒绝。

已建任务全部 done，只代表这些任务结束。核对 BRIEF 的全部范围后，才用 accept --project-complete 记录项目级验收。

## 实际边界

进程使用参数数组与 UTF-8 输入，记录真实退出并处理超时。范围检查主要是事后检查，不是操作系统安全沙箱。一个项目同一时间只支持一个运行者；没有常驻服务或自动跨会话调度。

金融模式 none 表示不由本工具附加美元硬上限，不表示免费。limit 模式需要执行器支持可核对的调用上限；历史用量未知时阻止继续受限付费调用。不要通过更换认证或删除约束解决失败。

# 让用户在 Agent 中看见进度

任务文件和用户可见进度是两件事。生成卡片或更新 JSON 后，必须在当前对话展示关键变化，不能只说“已经保存，请自行查看文件”。

## 展示方式：按宿主能力选最高一档

工具输出 presentation 时同时给出四种形态，宿主用得上哪种就用哪种，不猜工具名：

| 形态 | 内容 | 用途 |
| --- | --- | --- |
| native_tasks | 每个任务的 id、title、status（pending / in_progress / completed / blocked）、detail、next | 有原生任务/Todo/计划面板的宿主直接同步，一条任务对应一条面板项 |
| native_plan | 阶段列表及 completed / in_progress / pending | 面板支持步骤或计划视图时同步阶段 |
| compact | 约十行：阶段、活动任务、计数、阻塞、下一步、需要用户决定的事 | 没有面板时的日常轮次；有面板时作为对话里的变化说明 |
| markdown | 完整看板：阶段表、验收目标、质量目标、任务表、未拆分需求 | 首次、阶段切换、验收、阻塞、恢复时 |

宿主有面板就同步面板并在对话里只说变化；没有面板就贴 compact，关键节点贴 markdown；只能输出纯文本的宿主同样用 compact/markdown。needs_user 列出所有等用户决定的事项（待回答的问题、待验收、待追加预算、待解锁），非空时单独列出，用户不必翻对话找。

项目任务记录是事实来源；原生面板、对话展示、PROJECT_STATE.md 和 notes/RESUME.md 都是视图，不分别维护完成状态。

例如应让用户看到：

> 当前阶段：界面预览
>
> 本阶段目标：验证核心页面、下拉展开和键盘交互，确认后沿用前端实现。
>
> 当前任务：统一公共控件，进行中。
>
> 已完成：基础布局与主题；下一步：检查弹层和窄屏。
>
> 需要你确认：预览完成后确认外观与交互；现在无需操作。

真实项目使用实际事实，不复制示例成绩。

## 必须展示的信息

- 完整项目目标、当前阶段、该阶段的交付物与完成条件。
- 当前任务、已完成内容、尚未完成内容、阻塞和下一步。
- 已确认但尚未拆分的 Bug、功能或其他需求；暂缓内容与当前范围区分。
- 需要用户决定的具体事项；没有则说明可在现有范围继续。
- 任务已验证、等待验收、已验收分别显示。已建任务全部完成不等于整个目标完成，不凭任务数量编造百分比。

先给整体阶段路线，再逐步建立可执行任务。小步实施用于隔离风险与验证结果，不代表缩减完整范围。阶段目标跟随真实需求，不能把“做一个 MVP”当作默认最终交付。

## 何时更新

首次启动、确认范围后、进入新阶段/任务、验证或审查结果返回、出现阻塞、等待验收、断线恢复和最终交付时更新。首次及阶段切换展示阶段表；平常只更新当前任务和变化，不反复贴全量历史。

长命令执行前先标明运行中。宿主允许异步读取时，结合真实进程和日志给简短进展；没有新事实时不编造完成度。会话中可持续通信时，长时间工作约每分钟给一次简短更新；不可通信的阻塞调用应提前说明正在执行什么，返回后立即更新。

## 使用现有工具

~~~text
python .workflow-kit/scripts/project_workflow.py progress --root .                 # 完整看板
python .workflow-kit/scripts/project_workflow.py next --root . --format compact    # 日常十行
python .workflow-kit/scripts/project_workflow.py progress --root . --format json   # native_tasks / native_plan / needs_user
python .workflow-kit/scripts/project_workflow.py resume --root .                   # 接手一页：状态、待办、下一条命令、先读哪些文档
~~~

实际 helper 路径以接入回执为准，新布局通常位于 .workflow-kit/scripts/。progress 只读；start/next 也返回 presentation。PROJECT_STATE.md 使用相同数据生成，卡片仍在 .workflow-kit/tasks/cards/，不是另起一套项目管理系统。

暂停、断网、缺权限、预算到限或独立审查能力不足时，展示原任务及可恢复下一步。不能因 UI 面板不支持而停止展示，也不能因为显示了卡片就宣称宿主原生面板已经适配。

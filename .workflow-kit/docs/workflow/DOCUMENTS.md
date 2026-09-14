# 最少记录，按需扩展

新接入使用 bootstrap，将本包的工具和记录隔离在 .workflow-kit/，根入口为 WORKFLOW-KIT.md。下表中的职责不变，实际文件路径以回执为准；原项目的业务资料通过引用保留，原任务不会被当成新工作流状态。

事实只维护一份：需求进入 PRODUCT/BRIEF，进度进入 TASK/RUN，技术取舍进入已有 ADR/Notes。用户不用管理目录或编辑 JSON。

## 默认创建

| 内容 | 职责 |
| --- | --- |
| WORKFLOW、AGENTS、CLAUDE | 同一套工作入口；旧项目原规则继续有效 |
| .workflow-kit/docs/PRODUCT.md | 范围、验收、约束和待定项；小项目可合并功能与测试计划 |
| .workflow-kit/docs/HANDOFF.md | 必要的接手补充，不抄任务状态 |
| .workflow-kit/docs/ADR/ | 无既有体系时使用的决策模板；已有 Notes/ADR 则沿用 |
| .workflow-kit/tasks/PROJECT、POLICY、DECISIONS | 项目阶段、策略、真实用户决定 |
| .workflow-kit/tasks/BRIEF、REFERENCES | 确认后的需求快照及参考来源，由 onboard/research 创建 |
| .workflow-kit/tasks/items、batches、runs、evidence | 唯一任务记录、批次、全部尝试与真实证据 |
| .workflow-kit/tasks/PROJECT_STATE、cards、三个看板 | 从上述记录生成的视图，不手工维护第二套状态 |
| .workflow-kit/docs/workflow、scripts | 可移植的流程与工具副本 |

重构保留并核对已有基线，缺少时在本次工作流记录中建立。bootstrap 识别同名但不同格式的旧状态并隔离保存新记录；旧版 workflow-kit 的迁移则保留原有任务与预算，不复制成新任务清零。经典 init/adopt 仅用于已确认的对应布局。

## 有需要才增加

API、DATA-MODEL、USER-FLOWS、独立 TEST-PLAN、RELEASE-ROLLBACK 等只有承担实际职责时才创建。可以从完整包取模板；初始化时 --full-docs 也可生成全套，但模板存在不代表内容已完成。

有界面时需要一份 UI 约定，可沿用已有设计规范或创建 .workflow-kit/docs/UI.md。约定包含主题、公共组件、控件状态和交互；截图及报告进入任务证据目录。UI 确认用原有任务验收和真实决定记录，不新增另一套批准系统。

memory/ 和 lessons/ 是可选的整理方式。已有 PRODUCT、架构文档和 ADR/Notes 时，优先引用它们，不把相同事实抄成四份 Memory。没有值得复用的教训时不写 Lessons；任务结果不是每次都要新增一篇决策。

生成的 .workflow-kit/docs/README.md 是简短文档地图；新增独立文档时同步其入口即可。旧项目沿用既有组织方式，不迁移代码目录来迎合模板。

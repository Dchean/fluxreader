# OPT-016C：前端状态回归按领域拆分

Status: verified-local
父卡OPT016。现有787条的行为保护完整保留，不凭行数/总数相同就验收。与016B的component-tests/harness不同写集。

## 写集

- tools/frontend-regression.mjs（变为简短编排入口）
- tools/frontend-tests/（新共享harness和领域模块，按功能名而非历史TASK号命名）
- tools/test-loader.mjs / ui-loader.mjs 仅真正必要路径解析适配；同一个store别名必须保留。不写component-regression/component-tests/package/CI（另卡review中）
- .agents/notes/implemented/testing/2026-09-16-测试门禁与断言纪律.md（仅本分域事实，保留016B新增段）
- docs/optimization/OPT-016C.md主控写，执行者不得改
- tmp/optimization-20261008/OPT-016C/RESULT.md、逐断言前后映射/原基线快照

## 结构与约束

入口只创建harness→按明确顺序await域模块→统一汇总/非0退出；推荐domains bootstrap-and-query、reader-and-body、mutations-and-sync、settings-and-feeds、ui-contracts等，每模块可独立理解/运行测试组（允许共享harness注入），不要“part1/2/3”复制巨大global拼接来假分域。不是每条断言一个文件。

旧顶层mutable articleRow/getArticlesBehavior/failBootstrap/invokeCalls等归一个harness/context，声明读写接口避免模块import复制两份store。临时替换__INVOKE__/Date/window设置必须finally复位，不让不同域隐式依赖上一个域恰好留下的fixture；必要顺序（首次bindAppStore未初始化探针）显式保留。模块级变量跨作用域需求用context传递/最小重建固定输入，不把整个源码eval进newFunction。

所有787条标识/描述/条件原样迁移优先，允许import.meta.url路径随位置正确适配，旧mock原执行结果不变。不得删除/跳过/降低已有期望、将原异步行为简化为静态contains、不可用统计硬编码787骗通过。域自己的结果也有明确信息。

## 验收

1. 拆前实际run保存每条结果名称；AST解析逐条assert/check调用映射和总量，拆后完整run同样787且覆盖名称集合一致，已识别仅注释/导入路径改变。
2. 提供至少3个域独立执行选项（例如--domain ...）且无其他域状态前置；完整默认仍所有域顺序执行。
3. 对读状态回滚、空态/分页、源码转义至少各一个真实变异仍报非0（临时/内存，生产源码保持），原有mutation harness可复用但不得只校验mock。
4. npm run test:frontend、npm run test:components、lint/build全绿；组件harness不被更改。
5. 修改实际非平凡测试策略必须Note说清数据所有权/隔离与例外；RESULT如实哪些独立域能跑和路径/断言映射，不能部分迁移后宣布完成。

No codegen运行时动态截取旧文本、无新增依赖；一次性脚本tmp不能提交主线。禁止memory/Git/代理，CodeBuddy直接编辑交独立review。

## R1 待修

Euclid只读审查确认787条件/路径与九域拆分保真；唯一P2是临时window/confirm/__INVOKE__等覆盖没有finally还原（smoke还成功遗留confirm）。按照本卡原契约补精确descriptor保存/恢复，原不存在属性finally删除；失败/await reject也必须恢复，不能留后续候选。

## R2验收

Euclid独立PASS，787个实际条件/145路径保真、9域独立和3类变异证据复用；新增finally复位在3个实际异常probe复核通过，旧不存在属性删除/descriptor还原。负责人最终全回归/组件/lint/build日志已对应交付，主控按新规范不重复同命令。

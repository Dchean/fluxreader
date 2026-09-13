# Agent Note: 消除 Reader/Overlays 的三条 react-hooks 告警

Status: proposed

## Problem

`npm run lint` 现有 6 条警告中 3 条来自产品组件（其余 3 条来自技能副本脚本，不在范围）：

1. `src/components/Reader.tsx:112:6` react-hooks(exhaustive-deps)：`markReadOnScrollBottom` 效果缺依赖 `art`（效果体读 `art` 做守卫与取 `articleId`，依赖数组却写 `art?.id`）。
2. `src/components/Overlays.tsx:72:7` react(set-state-in-effect)：搜索效果在空查询分支同步调用 `setResults([])` / `setSearching(false)` / `setSearchError(false)`（首次报 72:7；同效果体的 `setSearching(true)` 属同类）。
3. `src/components/Overlays.tsx:175:19` react(set-state-in-effect)：`useEffect(() => setCursor(0), [debounced, items.length])` 在效果体同步重置光标。

React 官方指引：效果应只用于与外部系统同步；同步 setState 会引发级联渲染，并使 React Compiler 跳过优化（lint 输出原文）。

## Proposal

逐条修复，原则是消除同步 setState / 依赖失配，同时不改变用户可见行为：

1. **Reader.tsx**：依赖数组 `art?.id` 改为 `art`。效果体本就以 `art` 为守卫并提取 `articleId`；`art` 对象变化（含正文/翻译更新替换对象）时重挂载 scroll 监听，监听器逻辑不变（重新 add/removeEventListener），现有 `art?.content` 依赖的"内容变化后重判滚动位置"意图被 `art` 依赖覆盖。若 lint 随后把 `art?.content` 判为多余依赖，一并移除（`art` 已覆盖），保留 `settings.markReadOnScrollBottom`、`isRawRenderMode`、`isShowingTranslatedProse`。
2. **Overlays.tsx 搜索效果**：把"空查询时的状态归零"从效果体移到渲染期派生——新增 `resultsEffective = debounced ? results : []`、`searchingEffective = debounced !== '' && searching`、`searchErrorEffective = debounced ? searchError : false`，渲染与 items useMemo 改用派生值；效果体空查询分支直接 `return`，不再 setState。`setSearching(true)` 移入 250ms setTimeout 回调开头（真正的异步搜索发起时）。then/catch/finally 内的 setState 保持不变（异步回调不属本规则）。
   行为影响（可接受，如实记录）：搜索指示器从"查询变化即亮"变为"防抖 250ms 后搜索真正发起时亮"；清空查询时结果/错误从"状态立即重置"变为"渲染派生为空、状态延迟清理"——显示结果二者一致。
3. **Overlays.tsx 光标重置**：`useEffect(() => setCursor(0), …)` 改为 React 认可的"渲染期调整派生状态"模式——用一个小状态记录上次 `(debounced, items.length)` 键，渲染期发现键变化即 `setCursor(0)` 并更新键。重置时机从"提交后"提前到"渲染期"，可见行为不变（查询/列表变化后光标回到 0）。

每处修复点留一行反向注释指向本 Note。

## Alternatives considered

### 不做：6 条警告里 3 条是技能副本的，产品代码这 3 条也"能跑就行"

最强理由是警告不阻塞 CI，修复涉及 effect 时序，回归风险真实存在。

不采用的理由：用户已批准本任务；这三处正是 React Compiler 优化被跳过的位置，且 set-state-in-effect 是级联渲染的已知来源。修复方向全部是 React 官方文档推荐写法，风险可通过 tsc + 8/8 回归 + 构建控制。

### 用 eslint-disable 注释压掉告警

最强理由是零行为风险、改动最小。

不采用的理由：项目约定禁止关闭规则让门禁变绿；且文件里已有一处现存的 `eslint-disable-next-line exhaustive-deps`（Reader.tsx:95，非本任务范围），不应再增加同类欠账。

### 把搜索状态整体迁移到 React Query / useSyncExternalStore 等方案

最强理由是架构上更现代，彻底消除手写 effect 状态机。

不采用的理由：超出告警修复的授权范围，引入新依赖属高风险边界；本任务保持最小变更。

## Acceptance criteria

- `npm run lint` 警告 6 → 3（仅剩技能副本 3 条），退出码 0，无规则关闭或新增 disable 注释（Reader.tsx:95 的既有 disable 不动）。
- `tsc -b`、`npm run build` 通过；`npm run test:frontend` 仍 8/8（断言名称与顺序不变）。
- 仅 `src/components/Reader.tsx` 与 `src/components/Overlays.tsx` 变化。
- 三处修复点各留一行指向本 Note 的反向注释。

## Risks

- 搜索指示器时机的 250ms 变化与派生值方案对 `results` 陈旧状态的处理，若有遗漏的消费点（渲染或 useMemo 之外的引用）会露出旧数据——实现时须全文件检索 `results` / `searching` / `searchError` 的全部消费点并改用派生值。
- Reader 依赖改 `art` 后，若 store 高频替换 article 对象，监听器重挂频率上升——重挂本身开销极小（remove/add listener），但需确认没有依赖"效果体只在文章切换时运行"的隐式逻辑。
- 前端回归 8/8 不覆盖命令面板（Overlays），验证以类型、构建、lint 与人工渲染推理为准；无法自动化的部分如实记录。

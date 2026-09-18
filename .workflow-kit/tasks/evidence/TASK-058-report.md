# TASK-058 实施报告：同步失败对用户可见（前端消费 SyncReport.errors）

- 运行：RUN-5fe04a5e590e47e6a11cd7f0e8d30ff3
- 任务：TASK-058（批次 BATCH-6108ba756ba5490f924879b7f22b2f18）
- 授权：DEC-sync-errors-visibility-20260918（用户指令「按照你的判断进行修复」）
- 日期：2026-09-18

---

## 1. 缺陷

TASK-056 让后端把 pull 失败写入 `report.errors`，但**前端没有任何代码读取该字段**：

```text
后端返回：{"pulled_feeds":0,
           "errors":["拉取订阅 …/uncat.xml 建本地失败: [db] FOREIGN KEY constraint failed", …]}
界面提示：「后端同步完成」      ← 用户看到成功、数据零进来
```

**根因**：`phases.rs` 在 `errors` 非空时仍返回 `Ok(report)`（有意语义：单项失败不中断整链），
故 `.catch()` 对该路径永不触发；两处 `syncPhase` 调用点都**丢弃**了返回的 report。

## 2. 改动

| 文件 | 改动 |
| --- | --- |
| `src/store/syncErrors.ts` | **新增**：`syncFailureMessage` / `hasSyncFailures`（纯函数，无 DOM 依赖，便于直接断言） |
| `src/components/settings/SyncTab.tsx` | 保存并同步链：两阶段各读一次 report，收集失败；末端有失败则提示「有 N 项失败」，否则**逐字**保持 `后端同步完成` |
| `src/store/slices/sync.ts` | `triggerManualSync`：同上；并把同步失败与既有「N 个源直连失败」**共存**（此前只报后者） |
| `tools/frontend-regression.mjs` | 新增 13 条 `(f1)`–`(f4)` 断言 |
| `tsconfig.test.json` | include 加入 `syncErrors.ts` |

**核心设计**：helper 在有失败项时返回提示串、**无失败项时返回 `null`**——
调用方据此在成功路径上**完全走原分支**，从结构上保证既有文案不被改写。

## 3. 验证

### 3.1 单元断言（修前失败 / 修后通过）

把 helper 模拟为「任何输入都返回 null」（= 修复前的静默行为）后跑套件：

```text
[A] exit=1 | failing new assertions = 5
    ❌ (f1) 后端返回非空 errors ⇒ 产生「有 N 项失败」提示（修前该值为 null，失败静默）
    ❌ (f1) 提示必须含具体失败原因可定位，不得只给一个孤立数字
    ❌ (f1b) 失败项过多时只列前若干条，但仍给出总数
    ❌ (f1b) 单条失败也正常报出，不出现多余分隔
    ❌ (f3) errors 非空但内容全空白 ⇒ 仍提示有失败，不退回静默
```

再把**两处调用点**的 `syncFailureMessage(...)` 去掉（模拟「只改一处/一处没改」）：

```text
[B2] only SyncTab reverted (half done) -> exit=1 | failing=1
    ❌ (f4) 两处调用点都消费了 report 的 errors（只改一处不算完成）
```

即「只改一处」会被断言挡住。还原后 **268/268 通过**。

### 3.2 端到端验证（契约要求必做，已完成）

用**真实运行的应用** + 本地 GReader 协议服务端（仅 `/api/greader.php` 提供服务，
与真实 FreshRSS 一致）+ 测试库 `BEFORE INSERT` 触发器**注入真实插入失败**：

**失败场景**（触发器在）——实测 toast：

```text
同步完成，但有 2 项失败：拉取订阅 http://127.0.0.1:8898/uncat.xml 建本地失败: [db] t058 injected insert failure；
拉取订阅 http://127.0.0.1:8898/cat.xml 建本地失败: [db] t058 injected insert failure
```

**证据**：`TASK-058-e2e-failure-visible.png`（深色）、`TASK-058-ui-light-failure.png`（浅色）。

**成功场景**（移除触发器）——实测 toast：

```text
[0.7s] 已拉取订阅源，正在同步文章状态…
[2.8s] 后端同步完成
success text present: True
failure text present: False (expected False — sync succeeded)
=> success path UNCHANGED
```

**即：出错时报失败、正常时逐字不变。**

### 3.3 一次自查发现并修正的问题（如实记录）

第一次抓浅色证据时，我用 `updateSettings({themeMode:'light'})` 切主题，脚本打印
`theme: dark`，**但截图仍被命名为 `*light*.png`**。我核对了字节：

```text
same bytes as dark evidence? True      ← 与深色证据完全相同的字节，即标签是错的
```

**处置**：删除该文件，改从**真实 UI 控件**（外观 → ☀️ 浅色模式）切换，
并**在脚本里加硬校验**：若 `data-theme !== 'light'` 就直接中止且不留证据文件。

**根因**：`App.tsx:169-180` 的 `data-theme` 由 React `themeMode` state 驱动，
我直接改 store 的调用方式未能生效（且我没在写文件前校验结果）。

**教训**：**生成证据时必须校验「证据描述的状态」确实成立**，
否则会产出「标签正确、内容不对」的假证据——这与本会话此前的宽度误判同源：
**我记录的是我以为的状态，而不是我验证过的状态。**

## 6. 独立审查（第 1 轮 PASS）与审查驱动的两处修复

第 1 轮独立审查判定 **PASS**（10 项核对全通过；审查者独立重算候选摘要
`f8df22ec…` 与 133/133 文件哈希一致，独立复现 5 条失败断言与**双向**半改场景，
并按像素而非文件名核实两主题标签正确）。同时**指出两处我未报告的缺陷**，均已修复：

### 修复 A（审查 FINDING 1）：失败信息被排在末尾，最该被看到的反而最先被挤掉

**问题**：`showToast` 只保留最后 4 条（`store/slices/ui.ts` 的 `.slice(-4)`）且每条 2.2s 消失。
我原先把手动链写成 `parts = [已刷新…, N 个源直连失败, ...syncFailures]`，
**失败排在最后**——一次同步链连发多条提示时，失败最先被挤出可视窗口。
**那等于让本任务要解决的问题在提示层复活。**

**修法**：失败**前置**，且**不改变任何既有文案本身**（只调先后）：

```ts
const base = summary
  ? summary.failed_feeds > 0
    ? `已刷新，新增 ${summary.new_articles} 条，${summary.failed_feeds} 个源直连失败`
    : `已刷新，新增 ${summary.new_articles} 条`
  : '已刷新，无新文章';
get().showToast(syncFailures.length > 0 ? `${syncFailures.join('，')}，${base}` : base);
```

三个成功分支与改动**逐字相同**（已用源码比对 + 运行时断言双重锁定）。

> **审查关于「被截断」的说法我未能复现**：审查者按 219px 内容盒推断长失败提示会截断，
> 但那是 **Endpoint 输入框**的宽度，不是 toast。我在**运行中的应用**里实测
> （真实 `.toast-pill` 元素 + 真文案）：
> `1 条失败 666px`、`2 条失败 1142px`、`手动链 2 条 883px`，
> 三者 `scrollWidth == clientWidth`（**未截断**）。故仅采纳其「顺序」结论，不采纳「截断」结论。

### 修复 B：（f1）里一条断言是同义反复

审查者指出 `(f1) 修前行为可复现` 断言的是 `syncFailureMessage(undefined) === null`——
该式在**修复前后都成立**，等于什么都没验证。已改为模拟旧调用点的真实形态
（回调不收参数 ⇒ 丢弃 report ⇒ 拿不到任何信号），使其只在修复后成立。

### 修复 C：补上运行时覆盖（审查者指出 `triggerManualSync` 此前零运行时覆盖）

新增 `(f5)` 5 条**运行时**断言：用 invoke mock 真实驱动 `triggerManualSync`，
捕获它实际发出的 toast 文本并与旧模板逐字比对：

```text
(f5) 运行时·无失败：最终 toast 逐字为「已刷新，新增 3 条」（与改动前相同）
(f5) 运行时·无同步失败但有直连失败：逐字为「已刷新，新增 3 条，2 个源直连失败」
(f5) 运行时·无新文章：逐字为「已刷新，无新文章」
(f5) 运行时·同步有失败：最终 toast 确实含失败（修前此处只有「已刷新…」）
(f5) 运行时·失败文案前置：失败出现在「已刷新」之前
```

### 修复 D（**第 2 轮审查 FINDING，我自己的测试 bug**）

第 2 轮审查判定 **FAIL**，指出我上面这组 `(f5)` **自身有缺陷**：

```js
// 错误写法（我原来的）
const which = (args && args.args && args.args.which) || 'feeds';
```

`api.syncPhase` 的调用形态是 `inv('sync_phase', { which, full })`——
**第二参数就是 args 对象本身**（不像 `list_articles` 那样再包一层 `{ args }`）。
我套用了 `list_articles` 的写法，于是 `args.args` 恒为 `undefined`，
**两个阶段都回落到 `'feeds'`**——`states` 阶段的消费点**从未被 (f5) 覆盖**。

审查者的实证：**删掉 `sync.ts` 里 states 的消费代码后，全套仍 `275/275 exit 0`**。
我复核源码确认属实（`api.ts:485-488`），并复现同样结果。

**危害**：`(f5)` 正是我为回应第 1 轮「该链零运行时覆盖」而加的补救，
却因为参数解包写错而**只驱动了一个阶段**——**与我正在修的缺陷同类：
「测试没有测它声称要测的东西」**。

**修法**（两处）：
1. 解包改正为 `(args && args.which) || 'feeds'`；
2. **新增 `(f5-st)` 2 条专门覆盖「仅 states 阶段失败」**的用例
   （`states.errors = ['状态阶段失败: …']`），确保 states 消费点被真正驱动。

**闭环验证**（我自查）：修好后再把 states 的消费删掉：

```text
[states consumption REMOVED] exit=1 | 合计 275/277 通过 | failing = 2
    ❌ (f5-st) 运行时·仅 states 阶段失败也必须被呈现（防 mock 只驱动 feeds 的盲区）
    ❌ (f5-st) 仅 states 失败时，成功子句仍完整保留在后（未相互吞掉）
```

即该盲区**已被关闭**（修前同样改动为 275/275 全绿）。

### 修复 E（**第 2 轮审查 FINDING 2**）：窄窗口把失败提示推出屏幕

第 2 轮审查另指出：`.toast-pill` 是 `white-space: nowrap` 的**右对齐**元素，
且它与 `.toast-layer` 都无宽度约束 → 长提示**向左溢出视口**。

**我在运行中的应用实测确认，且比审查报告更严重**（窗口宽度 / 视口宽度 → 左侧溢出）：

```text
窗口 980px（应用 tauri.conf.json 的 minWidth）→ 视口 847px  → 溢出 315px
窗口 1100px                                  → 视口 953px  → 溢出 209px
窗口 1250px                                  → 视口 1084px → 溢出  78px
```

**溢出的恰是左侧**，即「同步完成，但有 2 项失败：」这个**标记本身被推到屏幕外**——
**本任务要交付的「失败可见」在窄窗口下完全失效**。这不是纯观感问题，
而是同一目标的第二处破口（内部未截断，`scrollWidth == clientWidth`，审查这点判断正确）。

**修法**（`src/styles/base.css`，两处最小改动）：

```css
/* layer：保持既有 460px 上限，同时允许收窄到视口内 */
max-width: min(460px, calc(100vw - 40px));

/* pill：由 nowrap 改为可换行（短提示在 460px 内仍是一行） */
white-space: normal;
max-width: 100%;
overflow-wrap: anywhere;
```

**修后实测**（同一运行中的应用）：

```text
窗口 980px  → pillWidth 460px, leftOverflow 0   （修前 315px）
窗口 1100px → pillWidth 460px, leftOverflow 0   （修前 209px）
窗口 1250px → pillWidth 460px, leftOverflow 0   （修前  78px）

短提示未回归：`后端同步完成` 宽 116px（与改动前实测一致）、`已刷新，新增 12 条` 宽 147px；
长失败提示：宽 460px、约 3 行（第 2 轮审查实测 2.96 行；本报告早先写「4 行」不准确，已订正）、溢出 0。
```

**窄窗口实机证据**：`TASK-058-ui-narrow-980.png`——应用最小宽度（980px 窗口 / 847px 视口）下
经**完整端到端流程**（真实触发器注入失败 + 真实点击「保存并同步」）捕获的真实 toast：
宽度 459px、`overflowLeft: 0`、未截断，换行 3 行，失败标记完整可见。

> **取证方法上的一处坑（如实记录）**：我最初想用 CDP 里 `import('/src/store.ts')` 调 `showToast` 造提示，
> 结果 store 更新了但**页面始终没有 pill**。原因是**动态 import 得到的是另一个模块实例**，
> 不会驱动正在运行的 React 树。改为走**应用自身的代码路径**（真实点击触发同步）后才捕获成功。
> 前两次尝试产出的 `TASK-058-ui-narrow-980.png`（无 toast）因此是**无效证据**，已覆盖为当前有效版本。

并新增 `(f6)` 3 条断言锁定（含「修前形态可复现」的对照项）。

### 修复后的承重性验证（我自查，双向）

```text
[B] 把失败移回末尾  -> exit=1，失败 2 条
    ❌ (f4) 有同步失败时，失败文案前置于「已刷新…」
    ❌ (f5) 运行时·失败文案前置
[A] 手动链不再消费 errors -> exit=1，失败 3 条
    ❌ (f4) 两处调用点都消费了 report 的 errors
    ❌ (f5) 运行时·同步有失败：最终 toast 确实含失败
    ❌ (f5) 运行时·失败文案前置
[C] 仅删 states 消费（第 2 轮 FINDING 1）-> exit=1，失败 2 条  ← 修 D 后新增的守卫
```

修复后门禁：**frontend 280/280**、cargo 141/0/9、lint 0/0、build exit 0；
端到端**重跑仍通过**（失败提示照常出现，用户库哈希一致）。

## 7. 门禁结果

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **141 passed / 0 failed / 9 ignored**（未受影响，证明 Rust 侧零改动） |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**280/280**（既有 26 + 新增 254） |

## 8. 边界遵守

- **`src-tauri/**` 零改动**（`git diff --stat -- src-tauri` 为空）——本任务纯前端消费；
- 未把「errors 非空」改成抛错（`Ok(report)` 是有意语义）；
- **成功路径文案与顺序逐字未变**：既由源码模板比对锁定
  （三条模板与 `git show 4b496c0:` 的原文逐字相同），又由 `(f5)` **运行时**断言锁定；
- 未引入新依赖（`package.json` 零改动）；
- 未改工作流脚本；
- **用户真实数据库零残留**：端到端测试前备份、测后逐字节还原，
  实测 `restore check: {'db': True, '-wal': True}`（哈希一致）。

## 9. 遗留与已知边界

- **本任务只解决「可见性」，不解决「失败本身」**：例如磁盘满导致插入失败时，
  用户现在**能看到**失败原因，但仍需自行排查环境。这符合本任务范围。
- 「查看完整错误明细」目前是**提示内联列出前 2 条 + 总数**（契约要求「至少首条原因，
  或可展开」，当前满足前者）。失败项很多（>2）需完整清单时应另立任务。
- TASK-054 遗留的 P1/P4 弱断言测试仍未补强（独立后续项）。

## 10. 本任务中我自己的失误（汇总留痕）

| # | 失误 | 谁发现 | 教训 |
| --- | --- | --- | --- |
| 1 | 浅色证据截图实际是深色（主题未生效）却按 `light` 命名 | 自查（字节比对） | 产出证据前必须**校验证据描述的状态确实成立** |
| 2 | 手动链把失败文案排在末尾，最该被看到的反而最先被挤掉 | 第 1 轮审查 | 「让失败可见」不只是「把它写进提示」，还要考虑它在**提示队列里的位置** |
| 3 | `(f1)` 里写了同义反复断言（修复前后都成立） | 第 1 轮审查 | 断言必须能在**缺陷态**失败，否则等于没写 |
| 4 | **(f5) mock 参数解包写错**（`args.args.which` vs `args.which`），两个阶段都回落 `feeds`，states 消费点零覆盖 | 第 2 轮审查 | **我在为「零覆盖」打的补丁里又留下了零覆盖**——测试本身也要被验证（我事后用「删掉 states 消费应致失败」闭环） |
| 5 | 窄窗口下长提示溢出屏幕左侧 315px，把失败标记本身推出视野 | 第 2 轮审查 | 修复的效果要在**边界尺寸**下验证，不能只在默认窗口看一次 |

**共性**：与本次会话此前几次同源——**把「我以为成立的」当成「已验证的」**。
第 4 条尤其值得记：**修复覆盖率的动作本身也需要覆盖率验证**。

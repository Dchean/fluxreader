# Agent Note: translating 终态复位与全局单布尔

Status: implemented

## Problem

全局 `translating` 单布尔的置位只发生在「发起时拥有 Reader 视图」（`runReaderTranslation` 的 `showReaderTranslation`），而三处终态复位（done / 流内 error / promise catch）全部被 `ownsView` 门控——发起后用户切到别的文章或关闭阅读器，三处复位永不执行，布尔卡在 `true`：新打开文章的自动翻译触发（Reader.tsx 守卫 `!st.translating`）被永久阻断，翻译错误行也被吞。`ownsView` 门控的初衷是 A/B 并发场景下「A 的迟到结算不得清掉 B 的在途布尔」，但这个保护同时制造了卡死：复位的执行条件（结算时仍拥有视图）与置位条件（发起时拥有视图）不对称，切走后无人负责复位。

## Decision

**终态复位无条件执行（`translating: false` 不随 ownsView 门控）；`isShowingTranslatedProse` 等展示态维持 ownsView 语义不变。** 共享布尔被并发的他篇迟到结算提前归 false 是可接受的失真：按 id 的在途真值在 `translatingIds`（五态 generating 判定、流式纯文本渲染、自动触发的同篇防双发守卫都消费它），不随布尔失真。

**打开/关闭阅读器即复位布尔**（`openArticle` / `clearReaderSelection`）：布尔不跨文章继承——他篇在途不该拦住新文章的自动翻译，且该 effect 依赖不含 `translating`，打开瞬间被拦即整篇错过（不复跑）。同篇已在途的情形由 `translatingIds[id]` 守卫，复位不构成双发窗口。

并发回归的三条旧断言（reader-and-ai.mjs「OPT-020 reader translation A/B」：A 迟到 error/reject/done 不得清 B 的全局布尔）改锁新语义：布尔复位 + B 的 `translatingIds`/展示态不受影响。

## Alternatives considered

- **声明集/引用计数（谁置位谁释放，迟到结算只放自己的 claim）**：能同时保住「A 迟到不清 B」与「最终归 false」，但布尔语义实际变成「至少一个声明者在途」——他篇在途仍会拦住新文章的自动翻译（openArticle 复位要么违反计数语义、要么需要例外路径），且引入模块级簿记让「单布尔」事实上多写者。复杂度与收益不成比例，否。
- **把布尔改成派生值（selector 按 activeArticleId + translatingIds 计算「当前文章翻译在途」）**：语义最准确，但这是消费面契约变更（Reader.tsx 的订阅、ui-contracts 的源码守卫都要跟着动），牵动面大于本缺陷所需；留作后续方向。
- **只在 openArticle / clearReaderSelection 补复位、终态维持 ownsView 门控**：能修「切走后重开」的路径，但布尔在「切走 + 在途流挂起（无终态）」时依旧卡 true；终态复位才是根治，开关复位只是触发时机的兜底。

## Consequences

A/B 并发时，先结算的一方会把共享布尔提前归 false——Reader 顶部的错误行显示门控与自动翻译守卫短暂失真，五态与流式渲染不受影响（按 id 判定）；迟到结算方自己的产物/错误/展示态变更全部照旧。旧断言翻转是语义变更而非回归，由改写后的三条 A/B 断言与 b3-6 互斥回归共同钉住。

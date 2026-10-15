# OPT-017F：重复实现与零消费入口清理

Status: queued
父卡：OPT-017。

## 主控读码确认的事实（2026-10-16）

- 前端存在两份同形 id 解析实现：`src/store/selectors.ts::numericId` 与
  `src/store/internals.ts::scopeNumericId`（后者注释自述为避免循环依赖的私有副本）。
- 后端存在同名协议解析：`greader.rs::parse_feed_numeric_id` 与
  `sync/entries.rs::item_numeric_id`（用途不同，需逐个核对，不预设重复）。
- 父卡要求：清理前必须**先搜实际消费者并锁行为**；不得删除「暂时未调用但有
  public 契约」的接口。

## 交付

1. **重复实现收敛**：对 `numericId`/`scopeNumericId` 做等值断言后收敛为单一实现
   （抽取共享模块并打破循环依赖），或给出必须保留两份的证据与注释；后端同名
   解析逐个核对后按证据处理。
2. **零消费清单**：产出「零消费者 API/事件/命令」清单（含消费搜索结果与判定），
   区分为：a) 可删的内部死代码（删除 + 回归）；b) public 契约/预留（保留并注明）。
3. **回归**：删除项若无测试覆盖先补行为反例；收敛后前端域/组件回归与 Rust 相关
   测试全绿。
4. **文档**：归属 Note 记录清理边界与保留清单。

## 写集

`src/store/internals.ts`、`src/store/selectors.ts`、必要时 `src/store/slices/*`；
`src-tauri/src/greader.rs`、`sync/entries.rs`（仅在证据支持时）；对应测试；
归属 Note。不删除 IPC 命令签名与事件名（除非确认内部且无兼容义务并有证据）。

## 强制反例（先红后绿）

1. `numericId`/`scopeNumericId` 等值：对全部 scope 形态（all/category/feed/
   非法/负数/空串）先断言两实现同结果，再收敛。
2. 每个被删 API/命令/事件：先搜全仓消费者日志，删除后编译与回归全绿；
   保留项列出消费者或契约依据。

## 门禁

`npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`；
触及 Rust 时补 `cargo fmt/clippy/test`。

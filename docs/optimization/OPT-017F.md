# OPT-017F：重复实现与零消费入口清理

Status: verified-ci
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

## 主控验收记录（2026-10-19）

- 编码会话 `code-opt017f-20261016`：2026-10-16 在 `cargo test` 中途被宿主超时杀掉；
  2026-10-19 续跑补完门禁与 `RESULT.md`。独立只读复审 `audit-opt017f-20261019`：
  **PASS WITH FINDINGS**（P0/P1/P2 = 0，P3×5），5 项 P3 已在原会话 tidy 收口
  （单点守卫口径收窄、消费断言改独立期望表、`sync_now` 计数改为 33 处、事件配对
  日志按终态重生成、旧 Note 补「后续结局」）。报告与交付：
  `tmp/optimization-20261008/OPT-017F/{RESULT,REVIEW}.md`。
- 实现：新增零 import 叶模块 `src/store/scopeId.ts` 承载唯一 `numericId`；
  `selectors.ts` 保持 re-export 公共面、`internals.ts` 删私有 `scopeNumericId`；
  后端 `parse_feed_numeric_id`（`feed/<十进制>`）与 `item_numeric_id`
  （委托 `parse_item_id`）经证据确认是不同协议形态，保留不合并。
- 零消费清理（先搜消费者、后删除）：前端包装 `api.feedCounts`/`api.scopeCounts`/
  `api.setFeedAiFlags`，Rust `KEPT_UNREAD`/`client_login_url`，事件 `close-resolved`
  emit。保留：`feed_counts`/`scope_counts`/`set_feed_ai_flags` 三个 IPC 签名
  （非范围条款 + Rust 单测）、`sync_now`（33+5 处测试消费）、`origin_was_local`
  （no-op 列，留 `docs/roadmap.md`）。
- 门禁终态全绿：frontend 1188/1188（新增 scope-id 域 10/10）、components 44/44、
  lint/build exit 0；Rust `cargo fmt`/`clippy` exit 0，`cargo test` 分批 15 个 chunk
  全 exit 0，合计 **725 passed / 0 failed / 0 ignored**（live 三套件按 `#[ignore]`
  约定未纳入统一门禁）。
- 归属 Note：`.agents/notes/implemented/simplification/2026-10-16-零消费入口清理与id解析单点.md`；
  反向注释落在 `scopeId.ts`、`api.ts`、`lib.rs`、`greader.rs`。
- 提交 `d6b2b85` 推送 dev；提交级 CI run 231
  （[39185086423](https://github.com/Dchean/fluxreader/actions/runs/39185086423)）
  completed success（8m45s）。遗留：同形复制守卫只覆盖同名同形副本；改名/换写法的
  语义级重复需评审发现；`origin_was_local` 删列需新卡。

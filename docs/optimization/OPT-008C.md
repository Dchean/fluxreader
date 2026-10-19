# OPT-008C：内容所有权收口取证与身份不变量

Status: queued
父卡：OPT-008。前置：OPT-008A、OPT-008B（均 verified-ci）。

## 主控读码确认的事实（2026-10-19，独立只读审计）

审计报告：`tmp/optimization-20261008/OPT-008/codebuddy-closeout-audit-20261019.log`。

- 当前迁移末版是 **v30**；OPT-008B 末节写的 v21 是当时边界，不是现状。v22 建
  `contents` 并在同一迁移内回填 `content_id`、DROP 旧正文列；v29 删除
  `articles_fts`，改建 `articles_trigram_idx`（FTS5 trigram、contentless、候选语义）；
  生产搜索权威是 LIKE + trigram 候选（OPT-017B 结论）。
- 生产读路径已统一 `LEFT JOIN contents`，`articles` 旧正文列已不存在
  （`contents_ownership_e2e.rs::articles_table_has_no_legacy_content_columns`）。
- **缺口 1（代码不变量）**：`db/contents.rs::repair_extracted_body_if_mismatch` 就地改
  `body_text` 并 `revision+1`，但 `compute_identity` 含 body；修复后 identity 与字节
  不再一致，后续等价内容会新建行（欠共享），当前无测试锁定该取舍。
- **缺口 2（迁移反例）**：v21 升级夹具只有一篇文章；没有「两源同 URL、不同正文、
  不同 AI」的真实旧库夹具，也没有 `PRAGMA foreign_key_check` 断言。
- **缺口 3（AI 清理分叉）**：共享内容「一条旧、一条年轻」的 AI 年龄清理没有测试；
  清理回执数量/文案没有断言。
- **缺口 4（五布局读藏）**：五布局只覆盖正文修订失效；`is_read`/`is_starred` 隔离
  只覆盖 article/social 两布局。
- **缺口 5（口径）**：父卡字面要求 FTS，v29 已改为 LIKE + trigram 候选，需在
  父卡/子卡/Note 正式改写，避免后续审计按字面误判。

## 交付

1. **identity 不变量裁决与落地**（二选一，必须在 Note 记录理由与反例）：
   a) 重算 identity；唯一冲突时按「同 source + 同 HTML + 同 body」等价合并：
      重指全部引用条目、递增目标行 revision，再删除原行；调用方若持有旧
      `content_id`，必须拿到新 id 或重新读取。
   b) 明确 identity 是**不可变创建键**：修复只推进 revision、不重算；后续等价
      内容允许欠共享（宁欠共享、不误合并）。
   控制器倾向 a（同等价合并语义安全：identity 含 source，冲突只可能是同 source）；
   若 a 会触碰提取保护标志或跨来源语义，停止并回报证据，再退 b。
   无论选哪支，都要补测试：构造 legacy 提取行 body 与 HTML 不同源 → 修复 →
   再以同字节调用 `ensure_content`，断言「不复用错误行」且行为与裁决一致。
2. **v21 迁移无损反例**：两个 feed、同一 `url_norm`、两条 v21 `articles`，正文与
   AI 产物均不同，各自 `read_version`/`star_version`、各自 `sync_queue`。断言：
   article id / feed / guid / remote 全保留；不同正文产生两条 `contents`；两条 AI
   各自保留；队列行数与归属不变；旧正文列不存在；`PRAGMA foreign_key_check` 为空；
   重开幂等。保留现有单条夹具。
3. **AI 年龄清理共享兄弟分叉**：共享同一 `content_id` 的两条文章，一条 40 天前、
   一条 3 天前，均有 AI。`cleanup(30, "ai")` 后：旧条目 AI 清空且 epoch 推进、
   年轻条目 AI 保留、`contents` 保留；回执 `ai_cleared` 计数与实际清理对象一致，
   并断言回执文案口径（`commands/sync.rs:670-678`）。
4. **五布局读藏隔离**：五种 layout 各自 `is_read`/`is_starred` 独立可解释；布局
   过滤不吞掉其他布局条目；共享正文修订不改变未变化兄弟的读藏状态。
5. **文档口径**：在 `docs/optimization/OPT-008.md`、`OPT-008B.md` 与归属 Note 记录
   「搜索权威 = LIKE，`articles_trigram_idx` 是可重建候选索引」；把「当前 v21」类
   表述标注为历史边界。同步更新 `docs/optimization/README.md` 的 OPT-008 行由主控
   在登记时处理，执行者不改 README。

## 写集

`src-tauri/src/db/contents.rs`、`src-tauri/src/db/articles.rs`（仅 identity 合并
所需的调用点适配）、`src-tauri/tests/contents_ownership_e2e.rs`、
`src-tauri/src/db/commands_extraction_tests.rs`（或既有五布局测试落点）、
`src-tauri/src/commands/sync.rs`（仅回执断言暴露缺口时，需先回报）、
`docs/optimization/OPT-008.md`、`docs/optimization/OPT-008B.md`、归属 Note。

不重写历史章节，只补「现状/收口」段落与过时口径。不删除旧迁移与旧测试夹具。

## 强制反例（先红后绿）

- 迁移反例：先补夹具并以「审计列出的缺失断言」为红（缺 `contents` 两行 / 外键
  未校验 / AI 丢失任一即红），再改实现或夹具到绿。
- identity：先以当前实现跑出「修复后 identity 与字节不一致、二次 `ensure_content`
  新建行」的红日志，再按裁决落地。
- AI 清理：若现状已正确，补测试即绿并如实记录「无需改实现」；不要为制造红而改错
  行为。
- 五布局：缺断言先红；若现状已正确，补覆盖即绿并注明。

## 门禁

`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test`（分批执行，单次不得超宿主 10 分钟上限）；触及前端时补
`npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`。
证据存 `tmp/optimization-20261008/OPT-008C/evidence/`。

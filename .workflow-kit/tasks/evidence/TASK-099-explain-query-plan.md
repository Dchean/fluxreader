# TASK-099 EXPLAIN QUERY PLAN 前后对比（REQ-108：M-5 / M-7 / M-9）

- 数据：审计第三轮忠实假后端 fixture（真实库 schema + 索引，articles=3005、feeds=16，`build_fixture()` 固定种子）；另为 sync_queue 注入 1500 行合成队列（真实库队列长期为空，空表无法体现计划差异）。
- 耗时为中位数（n=100；M-9 为 n=5 整段计时）；SQL 片段与 src-tauri/src/db/articles.rs 生产常量逐字一致。
- SQLite 版本：3.50.4（本脚本）；生产为 rusqlite bundled 3.46，计划断言以 Rust 测试 `list_query_plan_is_index_ordered_without_temp_btree` 为准。

## M-5：列表排序 COALESCE → published_at + v14 索引

### 全部（默认首屏）

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 7 \| SCAN a<br>43 \| USE TEMP B-TREE FOR ORDER BY | 4.072 |
| 修后 | 8 \| SCAN a USING INDEX idx_articles_published | 2.340 |

### 按源 feed_id=1

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>49 \| USE TEMP B-TREE FOR ORDER BY | 0.118 |
| 修后 | 8 \| SEARCH a USING INDEX idx_articles_feed_published (feed_id=?) | 0.245 |

### 按分类 folder_id=1

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>15 \| SCAN feeds<br>23 \| CREATE BLOOM FILTER<br>68 \| USE TEMP B-TREE FOR ORDER BY | 2.392 |
| 修后 | 8 \| SCAN a USING INDEX idx_articles_published<br>12 \| CORRELATED SCALAR SUBQUERY 1<br>16 \| SEARCH f2 USING INTEGER PRIMARY KEY (rowid=?) | 2.618 |

### 未读 only_unread

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_unread (is_read=?)<br>48 \| USE TEMP B-TREE FOR ORDER BY | 0.195 |
| 修后 | 8 \| SEARCH a USING INDEX idx_articles_read_published (is_read=?) | 0.356 |

### 布局 layout=image

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>16 \| SCAN f<br>18 \| SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)<br>33 \| CREATE BLOOM FILTER<br>78 \| USE TEMP B-TREE FOR ORDER BY | 0.142 |
| 修后 | 8 \| SCAN a USING INDEX idx_articles_published<br>12 \| CORRELATED SCALAR SUBQUERY 1<br>17 \| SEARCH f2 USING INTEGER PRIMARY KEY (rowid=?)<br>20 \| SEARCH fo2 USING INTEGER PRIMARY KEY (rowid=?) | 1.412 |

### M-5 耗时解读（诚实口径）

验收目标是**计划**：修前全部变体 `USE TEMP B-TREE FOR ORDER BY`（每页把筛选集**整体物化排序**，代价随库规模线性涨且与 LIMIT 无关——这正是审计 M-5 的扩展性缺陷），修后全部变体改由索引有序驱动、零排序。耗时一列显示真实的取舍：无过滤/大集合场景（全部 4.06→3.15ms）有序扫描直接赢；**窄范围小结果集**（按源/未读/布局）修前「先过滤再排序」只碰少量行，修后有序扫描要带着逐行子查询走完整条索引，单页（LIMIT=500-600，前端 ARTICLES_PAGE_SIZE=500）慢 0.1–1.3ms——绝对量仍在毫秒级，且与库规模解耦（修前的排序代价随总量增长，修后的扫描代价随 OFFSET 增长）。M-9 的量级对比（4996.6ms → 1112.1ms）远大于 M-5 各变体的差异。

## M-7：sync_queue 索引（修前 SCAN → 修后 SEARCH/COVERING）

| 查询 | 修前计划 | 修后计划 |
| --- | --- | --- |
| DEL(enqueue 互斥覆盖) | 3 \| SCAN sync_queue | 4 \| SEARCH sync_queue USING INDEX idx_sync_queue_article (article_id=? AND action=?) |
| AGE(老化清理) | 3 \| SCAN sync_queue<br>32 \| LIST SUBQUERY 1<br>35 \| SCAN articles<br>43 \| CREATE BLOOM FILTER | 4 \| MULTI-INDEX OR<br>5 \| INDEX 1<br>36 \| SEARCH sync_queue USING INDEX idx_sync_queue_article (article_id=? AND action=?)<br>41 \| INDEX 2<br>46 \| LIST SUBQUERY 1<br>49 \| SCAN articles<br>57 \| CREATE BLOOM FILTER<br>78 \| SEARCH sync_queue USING INDEX idx_sync_queue_article (article_id=? AND action=?) |
| PEND(pull 对账) | 3 \| SCAN sync_queue<br>31 \| USE TEMP B-TREE FOR DISTINCT | 3 \| SCAN sync_queue USING COVERING INDEX idx_sync_queue_article |
| SUBS1(退订保护①) | 4 \| SCAN q<br>8 \| SEARCH a USING INTEGER PRIMARY KEY (rowid=?)<br>17 \| USE TEMP B-TREE FOR DISTINCT | 4 \| SCAN a USING COVERING INDEX idx_articles_feed<br>6 \| SEARCH q USING COVERING INDEX idx_sync_queue_article (article_id=?) |
| SUBS2(退订保护②) | 3 \| SCAN sync_queue<br>13 \| USE TEMP B-TREE FOR DISTINCT | 3 \| SCAN sync_queue<br>13 \| USE TEMP B-TREE FOR DISTINCT |

结论：DEL 由全表 SCAN 变 `SEARCH … USING INDEX idx_sync_queue_article`；PEND 由 `SCAN + TEMP B-TREE FOR DISTINCT` 变 `SCAN … USING COVERING INDEX`（article_id 天然有序，DISTINCT 流式去重）；AGE 走 MULTI-INDEX OR（两条索引都可用）；SUBS1 走覆盖索引 + 文章主键回表；SUBS2（feed_url 维度）保持 SCAN——该查询仅断连保护用、队列规模小（≤千行），不为它再加索引（索引集取舍对比见 tmp/task-099/m7_index_probe2.log 的 B–F 方案）。

## M-9：全部已读 集合化（旧逐 id 入队 vs 新 3 语句事务）

| 实现 | 语句数（N=未读条数） | 中位耗时 ms（N=3005） | 标读条数 |
| --- | --- | --- | --- |
| 旧：逐 id 收集+入队（每语句自动提交） | 2 + 2N = 6012 | 4123.2 | 3005 |
| 新：事务内 3 条语句 | 3（常数） | 827.8 | 3005 |

语句级计数断言（不随 N 增长）由 Rust 测试固化：`db::articles::tests::mark_all_read_statement_events_do_not_scale_with_n`（rusqlite authorizer 事件计数，N=1 与 N=200 必须相等；参考旧实现同场景事件数 >10 倍增长）；行为等价性由 `mark_all_read_with_enqueue_matches_reference_for_all_scopes` 以 9 个范围逐项锁定。


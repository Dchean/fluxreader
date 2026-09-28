# TASK-094 EXPLAIN QUERY PLAN 前后对比（新增布局谓词 vs 既有列表查询）

- 数据：审计第三轮忠实假后端 fixture（真实库 schema + 索引，articles=3005、feeds=16；`build_fixture()`，随机种子固定）。
- 谓词：`LAYOUT_FILTER_SQL`（src-tauri/src/db/articles.rs，mark_all_read / list_unread_ids_scoped / article_where 三处共用常量），仅当 `layout=Some` 时拼接。
- 耗时为中位数（n=200，同一热连接）。

## 结论

1. **既有调用零变化（构造性成立）**：`layout=None` 时不拼接任何子句，修前/修后 SQL 逐字节相同（脚本内 A1 assert 通过）——查询计划与耗时 therefore 完全一致（下表 ms_before 与 ms_after_none 是同一条 SQL 的两次测量）。
2. **新增维度不劣化既有计划**：layout=Some 时 SQLite 自适应为按 feed 索引驱动的 SEARCH（含 BLOOM FILTER，子查询 feeds=16 行 / folders=5 行），排序策略与 None 逐字相同（COALESCE 排序表达式导致的 TEMP B-TREE 是修前既有形态，非本次引入；脚本内 A2 assert）。实测耗时见下表：med(layout) 相对 med(None) 绝对增量 <0.5ms/页（A3 assert 上界 ×5+0.5ms）。
3. **mark_all_read 写入口径**：谓词改为共享常量后 UPDATE 计划不变（同一段 SQL 文本拼接，见明细）。
4. **article_index（锚定分页）**：窗口函数 + 布局过滤计划正常，与 list_articles 同口径。

## 明细

### 全部 · 最新在前（默认首屏）

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 7 \| SCAN a<br>43 \| USE TEMP B-TREE FOR ORDER BY | 4.194 |
| 修后 layout=None | 7 \| SCAN a<br>43 \| USE TEMP B-TREE FOR ORDER BY | 4.216 |
| 修后 layout=Some | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>16 \| SCAN f<br>18 \| SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)<br>33 \| CREATE BLOOM FILTER<br>78 \| USE TEMP B-TREE FOR ORDER BY | 0.138 |

修后 layout=Some 的 SQL（截断 240 字符）：`SELECT a.id, a.feed_id, a.title, a.author, COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet, a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec, a.ai_summary, a.source, a.published_at, a.is_read, a.is_star…`

### 单源 feed_id=10

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>49 \| USE TEMP B-TREE FOR ORDER BY | 0.365 |
| 修后 layout=None | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>49 \| USE TEMP B-TREE FOR ORDER BY | 0.368 |
| 修后 layout=Some | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>18 \| LIST SUBQUERY 1<br>22 \| SCAN f<br>24 \| SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)<br>39 \| CREATE BLOOM FILTER<br>82 \| USE TEMP B-TREE FOR ORDER BY | 0.046 |

修后 layout=Some 的 SQL（截断 240 字符）：`SELECT a.id, a.feed_id, a.title, a.author, COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet, a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec, a.ai_summary, a.source, a.published_at, a.is_read, a.is_star…`

### 单分类 folder_id=2

| 阶段 | EXPLAIN QUERY PLAN | 中位耗时 ms |
| --- | --- | --- |
| 修前 | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>15 \| SCAN feeds<br>23 \| CREATE BLOOM FILTER<br>68 \| USE TEMP B-TREE FOR ORDER BY | 1.249 |
| 修后 layout=None | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>15 \| SCAN feeds<br>23 \| CREATE BLOOM FILTER<br>68 \| USE TEMP B-TREE FOR ORDER BY | 1.251 |
| 修后 layout=Some | 8 \| SEARCH a USING INDEX idx_articles_feed (feed_id=?)<br>12 \| LIST SUBQUERY 1<br>15 \| SCAN feeds<br>23 \| CREATE BLOOM FILTER<br>36 \| LIST SUBQUERY 2<br>40 \| SCAN f<br>42 \| SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)<br>57 \| CREATE BLOOM FILTER<br>100 \| USE TEMP B-TREE FOR ORDER BY | 0.052 |

修后 layout=Some 的 SQL（截断 240 字符）：`SELECT a.id, a.feed_id, a.title, a.author, COALESCE(NULLIF(a.summary, ''), substr(a.body_text, 1, 280)) AS snippet, a.image_url, a.enclosure_url, a.enclosure_mime, a.duration_sec, a.ai_summary, a.source, a.published_at, a.is_read, a.is_star…`

### mark_all_read（UPDATE + 布局谓词）计划

```
5 | SEARCH articles USING INDEX idx_articles_unread (is_read=?)
13 | LIST SUBQUERY 1
17 | SCAN f
19 | SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)
34 | CREATE BLOOM FILTER
```

### article_index（窗口 + 布局谓词）计划

```
2 | CO-ROUTINE (subquery-2)
5 | CO-ROUTINE (subquery-4)
9 | SEARCH a USING INDEX idx_articles_feed (feed_id=?)
13 | LIST SUBQUERY 1
17 | SCAN f
19 | SEARCH fo USING INTEGER PRIMARY KEY (rowid=?)
34 | CREATE BLOOM FILTER
52 | USE TEMP B-TREE FOR ORDER BY
66 | SCAN (subquery-4)
102 | SCAN (subquery-2)
```

## A1 断言

既有调用（layout=None）SQL 逐字节相同：3/3 场景 assert 通过。

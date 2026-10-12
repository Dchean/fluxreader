# OPT-017A：隔离数据层性能基线

Status: verified-local
父卡OPT017的基线取证，不是性能优化完成。仅隔离 SQLite/生产 DB API 基线，不代表 WebView 或媒体性能验收。

## 目标

用当前生产db API在自建临时库测20k与50k真实长度正文的列表/搜索/计数/标读吞吐，确认FTS触发器无消费的写放大是否值得删除。不能用手写SQL近似查询冒充生产API性能；不碰正式用户DB，不发网络/同步。

## 写集

- src-tauri/examples/db_perf_baseline.rs（一次性driver，最终主控移tmp/归档，不进主线）
- tmp/optimization-20261008/OPT-017A/下结果JSON/seed描述/SQLite文件
禁止任何产品源码、docs/performance.md、schema迁移、Cargo/lock依赖改动。

## 运行设计

Windows release示例（cargo run --release --example ...）直接调用app_lib::db::open/current migration，导入20k/50k条固定合成文章；每篇内容至少2KiB和8KiB两档（四场景，尺寸内存可控），seed事务避免插入时间主导。源/分类多样、日期RFC3339递增且读藏固定分布，无实际外链请求。临时绝对输出目录显式参数并验证在指定tmp，不允许无参创建或写。

记录版本/commit或workingtree state（主控提供HEAD可只readgit）、SQLite库版本、数据条数/正文bytes/磁盘大小/FTS大小（dbstat如可用）；releasewarmup后5-10轮真实db::list_articles（首页与keyset深页）、search_articles（唯一词、无结果中文词、常用词）、feed_counts，给median/p95和行数断言。不要把warmup、seed或计时器精度算到业务结果。

状态写放大比较：同一临时库初始状态，生产set_read_bulk_with_enqueue对100/1000条的事务耗时，有FTS触发器A与仅临时副本删除FTS触发器B对照（不得改变产品库或DDL文件）；重新seed或快照确保两边相同，不已读no-op当优化；队列/读状态行数语义一致，index/FTS内容同步本来无消费者的成本单独说明。

不伪造RSS/堆值，本卡只SQLite/进程探针，不能据此评价媒体解码、画廊DOM、UI切换。内存可记录进程peak但不当WebView堆。超过资源预算停止报告，不能伤真实用户文件。例子需有限退出和错误nonzero、临时句柄关闭再清理。

## 交付

实现与运行同负责人，原始数值JSON+简短RESULT（4规模条件/真实调用/是否超过现有阈值/FTS成本对照），不预设“无需优化”；不在本卡直接改生产性能。独立review确认测量方法后主控据此派实际优化，不重复跑所有数值。无memory/Git/代理。

## 当前进度（2026-10-09 19:28）

隔离合成库基线及独立审查要求的元数据/表述纠错已经交付，纠错会话终态 success；没有重跑或修改样本耗时。结果见 tmp/optimization-20261008/OPT-017A/RESULT.md。主控尚需核对最终纠错与整理长期结论，不再标 coding；父卡 OPT-017 的实际性能优化尚未实施。

## 基线验收（2026-10-12）

独立初审提出的 500 条事务单位、FTS 因果归属及搜索阈值口径已纠正；主控已核对原始 JSON 并重算 50k×8KiB 的三个读中位数、p95=max 与辅助事务差值约 0.779ms/条，结果与修订稿一致。长期结论已整理至 docs/performance.md，保留跨构建、样本规模、未测锁等待/媒体的限制。没有重跑或改写原始耗时。

本子卡按本地基线证据收口；一次性 driver 已在 Rust 检查完成后移至 tmp/optimization-20261008/OPT-017A/source/db_perf_baseline.rs，未进入主线。该归档整理不改变有效基线。重新测量时可临时恢复到 src-tauri/examples 的同名目标并按文件头参数运行，结束后仅移回本任务拥有的探针；新测量必须标明当前构建，不覆盖旧样本。父卡 OPT-017 的实际优化和后续同条件对照仍未完成。

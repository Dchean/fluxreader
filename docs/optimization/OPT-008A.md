# OPT-008A：保守 URL 匹配与版本化重建

Status: verified-local
父卡：OPT-008（内容/条目身份分离仍须后续完成，本子卡不能关闭父卡）。无前置，可与只读review及非同文件任务并行。

## 目标

审计F12的论坛 t/s、group_id、web_chapter_id 等业务参数不能被通用跟踪清单抹掉。不把不同文章/订阅URL映射成同一键；已保存url_norm必须随算法版本重建，不仅改变未来插入。

## 保守规则

通用删除只保留明确归属的营销参数（已列utm_*、gclid/fbclid/mc_*等）；ref/referrer/t/s/group_id/web_chapter_id等通用名字保留。在确切twitter.com/x.com及其标准www/mobile子域、且路径是/<user>/status/<numericID>时才可去t/s/ref_src类已知跟踪参数，任意example.com和伪后缀x.com.evil不能套此规则。保留真实原始URL，不以normalized键对外导航。其余http/www/amp等旧策略本子卡不扩大改动，父卡再区分启发式同文与精确身份。

## 写集

- src-tauri/src/db/url_norm.rs
- src-tauri/src/db/migrations.rs（追加v19迁移或下一个真实版本，不能改旧SQL；ensure_url_norm_backfill版本化）
- src-tauri/src/db/dedup_tests.rs（相关期望按明确规则调整，不删行为保护）
- src-tauri/tests/url_identity_e2e.rs（新增，真实db::open升级回归）
- .agents/notes/implemented/bug-fix/2026-10-08-保守URL匹配与重建标记.md（新Note）
- tmp/optimization-20261008/OPT-008A/RESULT.md

禁止改articles/sync_map/entries/feeds/public库API（其他卡写集）。如某老测试绑定被移除的业务参数策略，必须先向主控报告待扩写集，不自行全库批量改。

## 迁移/恢复

旧url_norm_backfill_done仅说明旧算法回填过，不能使新算法升级永远跳过。追加SQL清除旧标记并让现有原始url重新算键；数据回填与新版本标记在同一事务内，失败可重试。旧deduped_urls只保存旧规范键已丢原始URL，不能编造反向恢复；只清这类内容去重墓碑允许重拉（不是用户退订/目录删除墓碑，绝不删除后者）。已被误合并/未存入的历史正文不能由迁移凭空恢复，在Note与RESULT明确后续需重拉；父卡负责匹配/条目模型。

## 必测

1. /viewtopic.php?t=123与?t=456、?s=foo/bar、?group_id=1/2、?web_chapter_id=1/2、?ref=first/second在普通站点均不同。
2. 明确utm/fbclid变体相同；X status允许跟踪变体相同；x.com.evil等伪域名不相同。
3. 临时旧库先旧marker和旧url_norm，保留两个不同原始topic URL、读/藏状态，生产open升级后二者分开；marker只能成功后写，失败回滚可重启补跑。
4. 修改前后的URL原串保持，原查分页/状态索引不受影响；本地true去重依旧有效。
5. cargo test新增suite+db dedup相关、clippy/格式本卡，不以-A作为通过，不依赖真实网络。

## 纪律

先行为反例再实现，代码编写均由CodeBuddy完成；禁止Git/worktree/全局配置/个人memory writes。只格式化负责文件，最后Note、RESULT后停止交独立review。

## R1 主控扩写集

为了保住用户退订墓碑的真实防复活语义，允许 db/feeds.rs、db.rs必要导出、sync/subscriptions.rs仅墓碑阶段，及当前新e2e实际feeds_phase回归。迁移旧规范化墓碑须带版本命名空间，不恢复不可知原URL、不把旧算法用于新身份匹配。详见本卡review R1三项具体反例。

## R3 与主控验收

Dalton R3 PASS；原X路径/port和legacy墓碑保全、读取错误不放行已关闭。主控独立url_identity_e2e 7/7通过，未知/JSON/BLOB故障和readd事务覆盖。父卡008B仍未完成，无法凭空恢复已丢原文。

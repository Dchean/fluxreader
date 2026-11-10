//! SQLite 数据层：schema + 迁移 + 类型化数据访问。
//!
//! 实现按领域拆分在 db/ 子模块（migrations / folders / feeds / articles /
//! contents / identity / url_norm / settings / sync_queue / sync_map / outbox）；本文件
//! 只保留共享 import、子模块声明与再导出，使 crate::db:: 的既有调用路径保持不变。
//! 当前格式独立建库并拒绝旧格式；当前格式内的后续升级仍须保留已发布历史。

use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use crate::error::AppResult;

/// R3 数据变化判定的共享归一：可空文本字段的**空串 ≡ NULL**（COALESCE/回填
/// 语义下两者不产生可见差异）。供 articles / sync_map 的前后值比对复用，
/// 避免各处自写一份归一逻辑。
fn null_if_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

// Note: db/ 按领域拆分子模块，对外接口保持 crate::db:: 不变（拆分是纯搬运，
// 调用方无需改动）；延续「单体按领域拆分」的既有做法。

mod ai_segments;
mod articles;
mod backup;
mod contents;
mod feeds;
mod folders;
mod identity;
pub use identity::remote_identity_guid;
mod migrations;
mod outbox;
mod settings;
mod sync_map;
mod sync_queue;
mod url_norm;

pub use ai_segments::{
    ai_segment_stats, delete_stale_ai_segments, list_ai_segments, upsert_ai_segment,
    AI_PRODUCT_TRANSLATION,
};
pub use articles::{
    article_ai_commit_token, article_content_commit_token, article_group_has_pending,
    article_index, article_index_in_window, articles_without_cover, cleanup_cache,
    clear_article_ai_product, clear_article_cover_if_matches, clear_dedup_tombstones,
    counts_snapshot, feed_counts, get_article, get_article_updates, get_articles,
    list_article_member_ids, list_articles, list_articles_in_window, list_read_target_ids_scoped,
    mark_all_read, mark_all_read_with_enqueue, prune_retained_confirmed, purge_remote_data,
    refresh_bound_remote_article, remote_ids_with_body, replace_article_content_checked,
    scope_counts, search_articles, set_article_ai_fields, set_article_ai_fields_checked, set_read,
    set_read_bulk_with_enqueue, set_read_with_enqueue, set_starred, set_starred_with_enqueue,
    upsert_article_with_feed, upsert_article_with_feed_source, upsert_article_with_feed_tracked,
    upsert_remote_article, AiCommitToken, AiProduct, ArticleListItem, ArticleQuery, ArticleRow,
    ArticleUpdates, ArticleUpsert, ContentCommitToken, CountsSnapshot, FeedCounts, NewArticle,
    ReadWriteReceipt, ScopeCounts, ScopeCountsDetail, ScopedCounts,
};
pub use backup::{
    backup_dir_for_db, create_manual_backup, database_path, user_version, BackupInfo,
    BACKUP_RETENTION,
};
pub(crate) use backup::{create_mirror_backup, sanitize_log_text, take_startup_backup_warning};
/// OPT-008B 第二阶段：contents 单一正文所有权（常量/判空辅助供 db 各子模块与
/// 命令层经 `crate::db::` 路径消费；写入路径经 `super::contents::` 直接调用）。
pub use contents::{content_non_empty, SOURCE_READABILITY, SOURCE_RSS};
pub use feeds::{
    add_feed_tombstone, begin_feed_refresh, binding_incomplete, binding_incomplete_key,
    clear_binding_incomplete, clear_binding_incomplete_attempts, clear_feed_tombstones,
    clear_zero_binding_discovery_attempts, delete_feed, explicit_feed_title, feed_folder_label,
    feed_remote_info, feed_tombstones, feed_url_by_id, feed_url_tombstoned, feeds_all_candidates,
    feeds_all_ids, feeds_due_for_refresh, feeds_due_for_refresh_candidates, find_feed_by_url,
    has_remote_feed_binding, insert_feed, insert_feed_origin, insert_feed_origin_modes,
    legacy_feed_tombstones, list_article_sources, list_feeds, mark_binding_incomplete,
    mark_zero_binding_discovery_attempt, prune_feed_tombstones, remote_feed_binding_count,
    remove_feed_tombstone, set_feed_ai_flags, set_feed_ai_modes, set_feed_custom_title,
    set_feed_fetch_state, set_feed_fetch_state_if_generation, set_feed_title_and_icon,
    unbound_remote_feeds, update_feed, update_feed_full, update_feed_layout,
    zero_binding_discovery_due, zero_binding_discovery_key, FeedAiMode, FeedRefreshAttempt,
    FeedRefreshCandidate, FeedRow, BINDING_INCOMPLETE_KEY_PREFIX,
    ZERO_BINDING_DISCOVERY_INTERVAL_SECS, ZERO_BINDING_DISCOVERY_KEY_PREFIX,
};
pub use folders::{
    add_folder_tombstone, create_folder, create_folder_with_sync_id, delete_folder, folder_members,
    folder_name, folder_tombstones, folder_tombstones_for, list_folders, prune_folder_tombstones,
    rename_folder, set_folder_ai_flags, set_folder_ai_flags_partial, set_folder_ai_modes,
    set_folder_collapsed, set_folder_sync_id, update_folder_layout, FolderRow, FolderTombstone,
};
pub(crate) use identity::{is_synthetic_guid, weak_observation_guid};
pub use migrations::open;
#[cfg(test)]
pub(crate) use migrations::MIGRATIONS;
/// OPT-009 阶段 A：类型化订阅动作 outbox 数据层（与 article 状态队列
/// sync_queue 分离）。本阶段只交付表/语义/数据层入口——推送执行、GReader
/// wire、Fever local-only 命令层接线不在阶段 A。
pub use outbox::{
    abandon_stale_outbox, adopt_remote_id_for_pending, advance_subscribe_stage,
    advance_unsubscribe_resolution, confirm_subscribe_if_desired_unchanged, enqueue_edit,
    enqueue_subscribe, enqueue_unsubscribe, mark_outbox_failed, mark_outbox_local_only,
    mark_outbox_session_blocked, outbox_has_pending, outbox_pending_by_id, outbox_pending_for_url,
    pending_outbox, prune_outbox, rebase_successor_edit_prev, rebind_outbox_generation,
    rebind_outbox_generation_for_ids, subscription_outbox_changed_payload,
    subscription_outbox_stats, update_pending_subscribe_desired, SubscriptionOutboxItem,
    SubscriptionOutboxStats, ACTION_EDIT, ACTION_SUBSCRIBE, ACTION_UNSUBSCRIBE, STAGE_PENDING,
    STAGE_REMOTE_CREATED, STATUS_LOCAL_ONLY, STATUS_PENDING,
};
pub use settings::{app_settings_bool, app_settings_str, get_setting, set_setting};
/* OPT-014 R1：账号提交事务内核/可注入写入——crate 内部（命令层）使用，
不是 IPC 面，也不对集成测试开放（测试在同模块单测内做）。 */
pub(crate) use articles::purge_remote_data_in;
pub(crate) use settings::set_setting_with;
pub use sync_map::{
    article_by_remote_id, article_field_versions, backfill_article_content, bump_sync_generation,
    confirm_article_remote_origin, count_unbound_local_feeds, ensure_uncategorized_folder,
    export_feeds_with_folders, feed_exists_by_url, feed_id_by_url, feed_id_by_url_normalized,
    fever_history_state, find_folder_by_name, folder_exists, get_article_content_html,
    get_article_for_summary, get_article_for_translation, get_article_remote_id, get_article_url,
    get_first_folder_id, last_sync_entry_id, last_sync_ts, list_unbound_local_feeds,
    list_unread_ids_scoped, match_remote_feed, reset_fever_history_state, set_article_remote_id,
    set_feed_remote_id, set_fever_history_complete, set_fever_history_pending,
    set_last_sync_entry_id, set_last_sync_ts, sync_generation, sync_mark_read_if_unread,
    sync_mark_read_if_unread_at_version, sync_mark_starred_if_unstarred,
    sync_mark_starred_if_unstarred_at_version, sync_mark_unread_if_read,
    sync_mark_unread_if_read_at_version, sync_mark_unstarred_if_starred,
    sync_mark_unstarred_if_starred_at_version, sync_match_maps, sync_set_article_status,
    sync_set_read_at_version, sync_set_star_at_version, update_article_fulltext,
    update_article_image_if_empty, update_feed_title_if_empty, verified_remote_binding,
    FeverHistoryState, SyncMatchMaps, VerifiedRemoteBinding,
};
pub use sync_queue::{
    article_queue_identity,
    enqueue_sync,
    mark_push_blocked,
    mark_push_failed,
    mark_push_verification_failed,
    mark_queue_bound_pending,
    mark_queue_identity_waiting,
    mark_queue_target_missing,
    prune_stale_unbound,
    prune_sync,
    purge_remove_feed_zombies,
    queue_changed_payload,
    retry_expired_intents,
    retry_target_missing,
    sync_queue_stats,
    take_sync_queue,
    ExpiredRetryResult,
    SyncQueueItem,
    SyncQueueStats,
    STATUS_BINDABLE_RETRY,
    STATUS_BOUND_PENDING,
    // Note: outbox 已有同名 STATUS_LOCAL_ONLY；必须保留别名，否则 E0252。
    STATUS_EXPIRED_UNCONFIRMED,
    STATUS_FAILED,
    STATUS_LOCAL_ONLY as SYNC_QUEUE_STATUS_LOCAL_ONLY,
    STATUS_RETRY_WAIT,
};
pub use url_norm::normalize_url;
/* OPT-008A R1：旧算法规范化快照——只供 db/feeds.rs 的 legacy 退订墓碑匹配/回收，
不是身份匹配 API，不对外暴露。 */
pub(crate) use url_norm::legacy_v1_normalize;

#[cfg(test)]
mod auth_probe;
#[cfg(test)]
mod commands_extraction_tests;
#[cfg(test)]
mod dedup_tests;
#[cfg(test)]
mod sync_extraction_tests;
#[cfg(test)]
pub(crate) use auth_probe::with_count;

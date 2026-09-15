//! SQLite 数据层：schema + 迁移 + 类型化数据访问。
//!
//! 实现按领域拆分在 db/ 子模块（migrations / folders / feeds / articles /
//! url_norm / settings / sync_queue / sync_map）；本文件只保留共享 import、
//! 子模块声明与再导出，使 crate::db:: 的既有调用路径保持不变。
//! 迁移为追加式：已发布的迁移不可修改，只能新增 M::up。

use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use crate::error::AppResult;

// Note: db/ 按领域拆分子模块、对外接口保持 crate::db:: 不变 — 见 .agents/notes/implemented/architecture/2026-09-14-db-module-split.md

mod articles;
mod feeds;
mod folders;
mod migrations;
mod settings;
mod sync_map;
mod sync_queue;
mod url_norm;

pub use articles::{
    article_index, articles_without_cover, cleanup_cache, clear_dedup_tombstones, feed_counts,
    get_article, get_articles, list_articles, mark_all_read, purge_remote_data, search_articles,
    set_article_ai_fields, set_read, set_starred, upsert_article_with_feed, ArticleListItem,
    ArticleQuery, ArticleRow, FeedCounts, NewArticle,
};
pub use feeds::{
    add_feed_tombstone, delete_feed, feed_remote_info, feed_tombstones, feeds_all_ids,
    feeds_due_for_refresh, feeds_fetch_failed, feeds_fetch_failed_bound, feeds_origin_remote,
    find_feed_by_url, insert_feed, insert_feed_origin, list_feeds, remove_feed_tombstone,
    set_feed_ai_flags, set_feed_fetch_state, set_feed_title_and_icon, update_feed,
    update_feed_layout, FeedRow,
};
pub use folders::{
    add_folder_tombstone, create_folder, delete_folder, feed_urls_in_folder, folder_name,
    folder_tombstones, list_folders, remove_folder_tombstone, rename_folder, set_folder_ai_flags,
    set_folder_collapsed, update_folder_layout, FolderRow,
};
pub use migrations::open;
#[cfg(test)]
pub(crate) use migrations::MIGRATIONS;
pub use settings::{get_setting, set_setting};
pub use sync_map::{
    add_article_dup_entry, article_by_remote_id, article_dup_entries, article_has_pending_sync,
    article_id_by_url, article_matches_remote_feed, backfill_article_content,
    count_unbound_local_feeds, ensure_uncategorized_folder, export_feeds_with_folders,
    feed_by_remote_id, feed_exists_by_url, feed_id_by_url, feed_id_by_url_normalized,
    find_folder_by_name, folder_exists, get_article_content_html, get_article_for_summary,
    get_article_for_translation, get_article_remote_id, get_article_url, get_first_folder_id,
    last_sync_entry_id, last_sync_ts, list_unbound_local_feeds, list_unread_ids_scoped,
    set_article_remote_id, set_feed_remote_id, set_folder_remote_id, set_last_sync_entry_id,
    set_last_sync_ts, sync_mark_read_if_unread, sync_mark_starred_if_unstarred,
    sync_mark_unread_if_read, sync_mark_unstarred_if_starred, sync_match_maps,
    sync_set_article_status, update_article_fulltext, update_article_image_if_empty,
    update_feed_title_if_empty, SyncMatchMaps,
};
pub use sync_queue::{
    enqueue_sync, prune_stale_unbound, prune_sync, purge_remove_feed_zombies, take_sync_queue,
    SyncQueueItem,
};
pub use url_norm::normalize_url;

#[cfg(test)]
mod commands_extraction_tests;
#[cfg(test)]
mod dedup_tests;
#[cfg(test)]
mod sync_extraction_tests;

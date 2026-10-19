//! 配置快照的「预览（plan）→ 确认（apply）」引擎（OPT-010）。
//!
//! 产品契约（docs/optimization/OPT-010.md 定版）：
//! - 两种应用模式：`merge`（默认，保守合并：只更新远端列出的对象，本地独有
//!   对象保留）与 `mirror`（镜像：以远端快照为准，删除本地多出的源与分类）。
//! - `config_sync_plan(payload, mode)` 只读：返回结构化 additions/updates/removals
//!   （feed/folder + affectedArticleCount）、快照 hash 与本地配置版本签名；
//!   前端展示预览并让用户确认——取消 = 不调用 apply，零写入。
//! - `apply_payload_checked` 在**单事务内**重新计算计划并复核
//!   expectedSnapshotHash（快照内容）与 expectedVersion（本地配置状态签名），
//!   任一不符返回 `stalePreview` 且零写入；全部成功或全部回滚（复用既有 tx 包装）。
//! - 镜像删除**只删计划列出的对象**：不产生远端退订 outbox（不调
//!   enqueue_unsubscribe）、不直接删 contents（条目级联删除由 FK + 既有
//!   「最后引用才回收」触发器处理）、先移动保留源再删目录（不误级联）；
//!   被删源落删除墓碑（防 pull 复活——墓碑是本地删除意图，与远端退订是两件事）。
//!
//! 版本签名（currentVersion）覆盖「会改变删除后果」的本地状态：分类（含
//! sync_id）、源、白名单 app_settings 投影、被删源的文章数/读藏聚合、被删源
//! 相关的 pending 写回意图行。预览后新增/编辑源、文章新增或读藏翻转、pending
//! 意图变化都会让旧预览失效（stalePreview）；与删除无关的刷新（未涉及源的
//! 抓取状态等）不使预览失效。
//!
//! schema 版本：快照 schema=2 起，分类携带稳定 `sync_id`（本地创建即分配、
//! v26 迁移回填）；匹配优先级 = 身份 → 名称保守兜底（仅当双方其一没有身份时，
//! 并采用远端身份填空）。同名但双方身份不同 = 不同对象：预览显式列冲突
//! （绝不猜「同名即同一对象」）。schema1 快照（无 sync_id/无 mode）按既有
//! URL（源）/名称（分类）保守匹配，继续可合并。
//!
//! Note: 镜像语义与删除保护（tombstone/contents/outbox 保留策略）—
//! 见 .agents/notes/implemented/architecture/2026-10-15-配置快照镜像应用.md

use super::{
    is_local_only_setting, list_folder_id_by_name, merge_app_settings, resolve_spec_modes,
    store_pending_connection, ApplyOutcome, ConnectionConfig, SyncPayload, SCHEMA_VERSION,
};
use crate::db::{self, FeedAiMode, FeedRow};
use crate::error::{AppError, AppResult};
use md5::{Digest, Md5};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 应用模式。wire 形态（serde lowercase）= 前后端契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplyMode {
    /// 保守合并：只更新远端列出的对象，本地独有对象保留（不删除）。
    Merge,
    /// 镜像：以远端快照为准，删除本地多出的源与分类（仅本机，不推远端退订）。
    Mirror,
}

impl ApplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyMode::Merge => "merge",
            ApplyMode::Mirror => "mirror",
        }
    }

    /// 严格解析（未知模式拒绝，不悄悄回默认）。
    pub fn parse(raw: &str) -> AppResult<Self> {
        match raw {
            "merge" => Ok(ApplyMode::Merge),
            "mirror" => Ok(ApplyMode::Mirror),
            other => Err(AppError::new(
                "validate",
                format!("未知的应用模式：{other}（允许 merge / mirror）"),
            )),
        }
    }
}

/// 预览条目：`kind` 区分 feed/folder（serde 内部标签即 wire 契约）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlanItem {
    #[serde(rename_all = "camelCase")]
    Feed {
        url: String,
        title: String,
        /// 目标分类名（merge 的未知归属兜底显示为「导入」）。
        folder: String,
        /// 更新项：发生变化的字段名（title/folder/layout/summaryMode/...）。
        #[serde(skip_serializing_if = "Option::is_none")]
        changes: Option<Vec<String>>,
        /// 删除项：该源将被级联删除的本地文章数。
        #[serde(skip_serializing_if = "Option::is_none")]
        affected_article_count: Option<i64>,
    },
    #[serde(rename_all = "camelCase")]
    Folder {
        name: String,
        /// 更新项且发生改名时：本地当前名字。
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        changes: Option<Vec<String>>,
        /// 删除项：目录内将随目录一并删除的源数。
        #[serde(skip_serializing_if = "Option::is_none")]
        feed_count: Option<usize>,
        /// 删除项：上述源的文章数合计。
        #[serde(skip_serializing_if = "Option::is_none")]
        affected_article_count: Option<i64>,
    },
}

/// plan 的 IPC 返回（serde camelCase = 前端契约）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub mode: ApplyMode,
    /// 快照内容指纹（payload 规范化后的 md5；替换快照即失效）。
    pub snapshot_hash: String,
    /// 本地配置版本签名（会改变删除后果的状态指纹；本地变更即失效）。
    pub current_version: String,
    /// removals 中所有被删源的文章数合计（目录删除只计其成员被删源，不重复计）。
    pub affected_article_count: i64,
    pub additions: Vec<PlanItem>,
    pub updates: Vec<PlanItem>,
    pub removals: Vec<PlanItem>,
    /// 远端存在且与本地不同的白名单设置键（将覆盖本地）。
    pub settings_changed_keys: Vec<String>,
    /// 远端缺失、将被本地同步删除的白名单设置键。
    pub settings_removed_keys: Vec<String>,
    /// 快照含非敏感连接建议（apply 时落为待确认建议，不改活动凭据）。
    pub pending_connection: bool,
    /// 需要用户注意的冲突/保留策略说明（如身份冲突、未推写回意图保留）。
    pub warnings: Vec<String>,
}

/// 已解析的计划（预览与执行共用一份计算，保证「所见即所执行」）。
struct PreparedPlan {
    preview: PlanPreview,
    folder_plans: Vec<FolderPlan>,
    feed_adds: Vec<FeedAdd>,
    feed_updates: Vec<FeedUpdate>,
    feed_removals: Vec<FeedRemoval>,
    folder_removals: Vec<FolderRemoval>,
    pending_connection: Option<ConnectionConfig>,
    /// 远端 app_settings 原文（执行期合并写入；预览只暴露结构化增量）。
    app_settings: Option<String>,
    skipped_feeds: usize,
}

/// 远端分类的落点：匹配到本地既有对象，或新增。
enum FolderPlan {
    Match {
        id: i64,
        target: FolderTarget,
        changes: Vec<String>,
        /// 本地无身份 + 名称命中：采用远端身份（只填空，绝不改写既有身份）。
        adopt_uid: Option<String>,
    },
    New {
        target: FolderTarget,
        sync_id: Option<String>,
    },
}

struct FolderTarget {
    name: String,
    layout: String,
    summary: bool,
    translate: bool,
    position: i64,
}

struct FeedAdd {
    url: String,
    folder: FolderRef,
    layout: String,
    summary: FeedAiMode,
    translate: FeedAiMode,
    site_url: Option<String>,
    favicon_url: Option<String>,
    title_for_display: String,
}

/// 源的目标归属：远端快照中的分类槽位，或 merge 的「导入」兜底。
enum FolderRef {
    Slot(usize),
    Sink,
}

struct FeedUpdate {
    id: i64,
    title: String,
    folder: FolderRef,
    layout: String,
    summary: FeedAiMode,
    translate: FeedAiMode,
    site_url: Option<String>,
    favicon_url: Option<String>,
}

struct FeedRemoval {
    id: i64,
    url: String,
    title: String,
    affected: i64,
}

struct FolderRemoval {
    id: i64,
    name: String,
    feed_count: usize,
    affected: i64,
}

/// 快照内容指纹：payload 规范化序列化（字段顺序固定）后的 md5。
/// md5 在此是**指纹**不是安全原语（与 contents identity / Fever 同口径）。
pub fn snapshot_hash(p: &SyncPayload) -> AppResult<String> {
    let bytes = serde_json::to_vec(p)?;
    Ok(md5_hex(&bytes))
}

fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn stale_preview(what: &str) -> AppError {
    AppError::new(
        "stalePreview",
        format!("预览已失效（{what}在预览后已变化）：请重新下载预览并确认后再应用"),
    )
}

/// 计划（只读）：结构校验 + 匹配 + 版本签名，不产生任何写入。
pub fn plan_payload(conn: &Connection, p: &SyncPayload, mode: ApplyMode) -> AppResult<PlanPreview> {
    Ok(prepare_plan(conn, p, mode)?.preview)
}

/// 应用（带预览 CAS）：expected 为 None 时跳过对应校验（仅 merge 的兼容入口
/// `apply_payload` 使用；镜像在 IPC 层强制要求两者非空，见 config_sync_apply）。
pub fn apply_payload_checked(
    conn: &Connection,
    p: &SyncPayload,
    mode: ApplyMode,
    expected_snapshot_hash: Option<&str>,
    expected_version: Option<&str>,
) -> AppResult<ApplyOutcome> {
    if mode == ApplyMode::Mirror && (expected_snapshot_hash.is_none() || expected_version.is_none())
    {
        return Err(AppError::new(
            "validate",
            "镜像应用必须先预览并携带快照 hash 与配置版本；已拒绝执行（本地未改动）",
        ));
    }
    // 全程单事务：计划（校验+匹配+版本签名）在同一事务读快照内计算，
    // CAS 命中后在同一事务里执行——全部成功或全部回滚。
    let tx = conn.unchecked_transaction()?;
    let plan = prepare_plan(&tx, p, mode)?;
    if let Some(expected) = expected_snapshot_hash {
        if expected != plan.preview.snapshot_hash {
            return Err(stale_preview("远端快照内容"));
        }
    }
    if let Some(expected) = expected_version {
        if expected != plan.preview.current_version {
            return Err(stale_preview("本地配置"));
        }
    }
    let outcome = execute_plan(&tx, &plan)?;
    tx.commit()?;
    Ok(outcome)
}

/* ============================================================
计划：校验 → 匹配 → 结构化预览 + 版本签名
============================================================ */

fn prepare_plan(conn: &Connection, p: &SyncPayload, mode: ApplyMode) -> AppResult<PreparedPlan> {
    if p.schema > SCHEMA_VERSION {
        return Err(AppError::internal(format!(
            "远端配置版本 v{} 高于本客户端支持的 v{}，请升级客户端",
            p.schema, SCHEMA_VERSION
        )));
    }
    // ---- 结构校验（先于一切写入与匹配；未知结构/枚举/URL/范围整体拒绝）----
    let mut folder_uids = HashSet::new();
    for f in &p.folders {
        if f.name.trim().is_empty() {
            return Err(AppError::new("validate", "快照包含空名称的分类，拒绝导入"));
        }
        if let Some(uid) = uid_of(f.sync_id.as_deref()) {
            if !folder_uids.insert(uid.to_string()) {
                return Err(AppError::new(
                    "validate",
                    format!("快照包含重复的分类身份（sync_id={uid}），拒绝导入"),
                ));
            }
        }
    }
    let mut feed_urls = HashSet::new();
    for f in &p.feeds {
        let url = f.url.trim();
        if url.is_empty() {
            return Err(AppError::new("validate", "快照包含空 URL 的源，拒绝导入"));
        }
        if !feed_urls.insert(url.to_string()) {
            return Err(AppError::new(
                "validate",
                format!("快照包含重复的源 URL（{url}），归属不明确，拒绝导入"),
            ));
        }
        // 非法 AI mode 必须在任何写入前整体拒绝（OPT-011 语义沿用）。
        resolve_spec_modes(f)?;
    }
    // app_settings 校验 + 设置变化增量（非法 refreshInterval 整体拒绝）。
    let (settings_changed_keys, settings_removed_keys) = match &p.app_settings {
        Some(remote_raw) => {
            let (_, changed, removed) = merged_app_settings_json(conn, remote_raw)?;
            (changed, removed)
        }
        None => (Vec::new(), Vec::new()),
    };

    let local_folders = db::list_folders(conn)?;
    let local_feeds = db::list_feeds(conn)?;
    // 本地已占用的分类身份：导入新建时不得复用（身份唯一性纪律；碰撞时降级为
    // 随机新身份，绝不覆盖/复用既有对象的身份）。
    let local_uids: HashSet<&str> = local_folders
        .iter()
        .filter_map(|lf| uid_of(lf.sync_id.as_deref()))
        .collect();

    // ---- 分类匹配（身份优先；名称仅在双方其一没有身份时保守兜底）----
    let mut used_folders: HashSet<i64> = HashSet::new();
    let mut folder_plans: Vec<FolderPlan> = Vec::with_capacity(p.folders.len());
    let mut additions: Vec<PlanItem> = Vec::new();
    let mut updates: Vec<PlanItem> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut slot_by_name: HashMap<&str, usize> = HashMap::new();
    for (slot, rf) in p.folders.iter().enumerate() {
        slot_by_name.entry(rf.name.as_str()).or_insert(slot);
        let remote_uid = uid_of(rf.sync_id.as_deref()).map(str::to_string);
        let mut matched: Option<usize> = None;
        if let Some(uid) = remote_uid.as_deref() {
            matched = local_folders.iter().position(|lf| {
                !used_folders.contains(&lf.id) && uid_of(lf.sync_id.as_deref()) == Some(uid)
            });
        }
        if matched.is_none() {
            if let Some(i) = local_folders
                .iter()
                .position(|lf| !used_folders.contains(&lf.id) && lf.name == rf.name)
            {
                let local_uid = uid_of(local_folders[i].sync_id.as_deref());
                if remote_uid.is_none() || local_uid.is_none() {
                    matched = Some(i);
                } else {
                    // 双方都有身份且不同：同名不是同一对象——不猜，显式列冲突。
                    let mirror_note = if mode == ApplyMode::Mirror {
                        "，并将删除本地对象"
                    } else {
                        ""
                    };
                    warnings.push(format!(
                        "分类「{}」与本地同名分类身份不同：按不同对象处理（新增远端对象{mirror_note}）",
                        rf.name
                    ));
                }
            }
        }
        let target = FolderTarget {
            name: rf.name.clone(),
            layout: rf.layout.clone(),
            summary: rf.auto_summary,
            translate: rf.auto_translate,
            position: rf.position,
        };
        match matched {
            Some(i) => {
                let lf = &local_folders[i];
                used_folders.insert(lf.id);
                let mut changes: Vec<String> = Vec::new();
                if lf.name != target.name {
                    changes.push("name".into());
                }
                if lf.layout != target.layout {
                    changes.push("layout".into());
                }
                if lf.auto_summary != target.summary {
                    changes.push("summary".into());
                }
                if lf.auto_translate != target.translate {
                    changes.push("translate".into());
                }
                if lf.position != target.position {
                    changes.push("position".into());
                }
                // P2-1：空身份行采用远端身份前必须查**全局占用**——身份唯一性纪律
                // 与 New 分支同口径：该身份已属于本地另一对象（矛盾混合快照）时
                // 不采用、本地行保持空身份，并显式预警；绝不产生两个对象共享同一
                // 非空 sync_id。
                let adopt_uid = match (uid_of(lf.sync_id.as_deref()), remote_uid.as_deref()) {
                    (None, Some(uid)) if !local_uids.contains(uid) => Some(uid.to_string()),
                    (None, Some(_)) => {
                        warnings.push(format!(
                            "分类「{}」携带的身份已被本地其它对象占用：不采用该身份（不覆盖既有身份）",
                            rf.name
                        ));
                        None
                    }
                    _ => None,
                };
                if !changes.is_empty() {
                    updates.push(PlanItem::Folder {
                        name: target.name.clone(),
                        previous_name: (lf.name != target.name).then(|| lf.name.clone()),
                        changes: Some(changes.clone()),
                        feed_count: None,
                        affected_article_count: None,
                    });
                }
                folder_plans.push(FolderPlan::Match {
                    id: lf.id,
                    target,
                    changes,
                    adopt_uid,
                });
            }
            None => {
                // P0-1：导入新建分类**采用**远端身份（execution 以 create_folder_with_sync_id
                // 直落，不再先随机分配再走「只填空」——那条路径对刚建行恒为 no-op）。
                // 远端无身份（schema1）→ None = 本地随机身份（创建即分配）。
                // 远端身份已被本地其它对象占用（快照与本地对象图冲突）→ 降级随机身份
                // 并显式预警：绝不改写既有身份、也不复用同一身份给两个对象。
                let new_sync_id = match remote_uid.as_deref() {
                    Some(uid) if local_uids.contains(uid) => {
                        warnings.push(format!(
                            "分类「{}」携带的身份已被本地其它对象占用：按新身份建立（不覆盖既有身份）",
                            rf.name
                        ));
                        None
                    }
                    Some(uid) => Some(uid.to_string()),
                    None => None,
                };
                additions.push(PlanItem::Folder {
                    name: target.name.clone(),
                    previous_name: None,
                    changes: None,
                    feed_count: None,
                    affected_article_count: None,
                });
                folder_plans.push(FolderPlan::New {
                    sync_id: new_sync_id,
                    target,
                });
            }
        }
    }

    // ---- 源匹配（URL 为身份，两种 schema 一致）----
    let local_by_url: HashMap<&str, &FeedRow> = local_feeds
        .iter()
        .map(|f| (f.feed_url.as_str(), f))
        .collect();
    let mut removed_feed_ids: HashSet<i64> = HashSet::new();
    let mut feed_adds: Vec<FeedAdd> = Vec::new();
    let mut feed_updates: Vec<FeedUpdate> = Vec::new();
    let mut skipped_feeds = 0usize;
    let mut sink_needed = false;
    let sink_folder_id = list_folder_id_by_name(conn, "导入")?;

    // 镜像：源的分类引用必须在快照分类列表内（不引入本地兜底目录——否则
    // 「导入」目录会被镜像当成远端不存在的对象反复删除/重建）。
    if mode == ApplyMode::Mirror {
        for f in &p.feeds {
            let name = f.folder.trim();
            if name.is_empty() || !slot_by_name.contains_key(name) {
                return Err(AppError::new(
                    "validate",
                    format!(
                        "镜像快照中的源 {} 引用了不存在的分类「{}」，为避免误删拒绝执行（可改用合并应用）",
                        f.url, f.folder
                    ),
                ));
            }
        }
    }

    for f in &p.feeds {
        let (summary, translate) = resolve_spec_modes(f)?;
        let folder_ref = match slot_by_name.get(f.folder.trim()) {
            Some(slot) => FolderRef::Slot(*slot),
            None => {
                // merge 兜底：未知/空归属落「导入」（既有保守语义）。
                sink_needed = sink_needed || sink_folder_id.is_none();
                FolderRef::Sink
            }
        };
        let display_title = if f.title.trim().is_empty() {
            f.url.clone()
        } else {
            f.title.clone()
        };
        let display_folder = match &folder_ref {
            FolderRef::Slot(slot) => p.folders[*slot].name.clone(),
            FolderRef::Sink => "导入".to_string(),
        };
        match local_by_url.get(f.url.trim()) {
            Some(lf) => {
                let target_folder_id: Option<i64> = match &folder_ref {
                    FolderRef::Slot(slot) => match &folder_plans[*slot] {
                        FolderPlan::Match { id, .. } => Some(*id),
                        FolderPlan::New { .. } => None,
                    },
                    FolderRef::Sink => sink_folder_id,
                };
                let mut changes: Vec<String> = Vec::new();
                if lf.title != f.title {
                    changes.push("title".into());
                }
                match target_folder_id {
                    Some(id) if id != lf.folder_id => changes.push("folder".into()),
                    Some(_) => {}
                    None => changes.push("folder".into()),
                }
                if lf.layout != f.layout {
                    changes.push("layout".into());
                }
                if lf.summary_mode != summary.as_str() {
                    changes.push("summaryMode".into());
                }
                if lf.translate_mode != translate.as_str() {
                    changes.push("translateMode".into());
                }
                if lf.site_url != f.site_url {
                    changes.push("siteUrl".into());
                }
                if lf.favicon_url != f.favicon_url {
                    changes.push("faviconUrl".into());
                }
                if changes.is_empty() {
                    skipped_feeds += 1;
                } else {
                    updates.push(PlanItem::Feed {
                        url: f.url.trim().to_string(),
                        title: display_title.clone(),
                        folder: display_folder,
                        changes: Some(changes),
                        affected_article_count: None,
                    });
                    feed_updates.push(FeedUpdate {
                        id: lf.id,
                        title: f.title.clone(),
                        folder: folder_ref,
                        layout: f.layout.clone(),
                        summary,
                        translate,
                        site_url: f.site_url.clone(),
                        favicon_url: f.favicon_url.clone(),
                    });
                }
            }
            None => {
                additions.push(PlanItem::Feed {
                    url: f.url.trim().to_string(),
                    title: display_title.clone(),
                    folder: display_folder,
                    changes: None,
                    affected_article_count: None,
                });
                feed_adds.push(FeedAdd {
                    url: f.url.trim().to_string(),
                    folder: folder_ref,
                    layout: f.layout.clone(),
                    summary,
                    translate,
                    site_url: f.site_url.clone(),
                    favicon_url: f.favicon_url.clone(),
                    title_for_display: display_title,
                });
            }
        }
    }

    // merge 的「导入」兜底目录：缺失时作为新增对象列出（执行期按需创建）。
    if sink_needed {
        additions.push(PlanItem::Folder {
            name: "导入".into(),
            previous_name: None,
            changes: None,
            feed_count: None,
            affected_article_count: None,
        });
    }

    // ---- 镜像删除集（只列删除；merge 恒为空）----
    let mut removals: Vec<PlanItem> = Vec::new();
    let mut feed_removals: Vec<FeedRemoval> = Vec::new();
    let mut folder_removals: Vec<FolderRemoval> = Vec::new();
    if mode == ApplyMode::Mirror {
        for lf in &local_feeds {
            if !feed_urls.contains(&lf.feed_url) {
                removed_feed_ids.insert(lf.id);
            }
        }
        let removal_ids_vec: Vec<i64> = removed_feed_ids.iter().copied().collect();
        let counts = article_counts_for(conn, &removal_ids_vec)?;
        let folder_name_of = |folder_id: i64| -> String {
            local_folders
                .iter()
                .find(|f| f.id == folder_id)
                .map(|f| f.name.clone())
                .unwrap_or_default()
        };
        for lf in &local_feeds {
            if removed_feed_ids.contains(&lf.id) {
                feed_removals.push(FeedRemoval {
                    id: lf.id,
                    url: lf.feed_url.clone(),
                    title: lf.title.clone(),
                    affected: counts.get(&lf.id).copied().unwrap_or(0),
                });
            }
        }
        for fr in &feed_removals {
            let folder_id = local_feeds
                .iter()
                .find(|f| f.id == fr.id)
                .map(|f| f.folder_id)
                .unwrap_or_default();
            removals.push(PlanItem::Feed {
                url: fr.url.clone(),
                title: fr.title.clone(),
                folder: folder_name_of(folder_id),
                changes: None,
                affected_article_count: Some(fr.affected),
            });
        }
        for lfr in &local_folders {
            if !used_folders.contains(&lfr.id) {
                let members: Vec<&FeedRemoval> = feed_removals
                    .iter()
                    .filter(|fr| {
                        local_feeds
                            .iter()
                            .find(|f| f.id == fr.id)
                            .is_some_and(|f| f.folder_id == lfr.id)
                    })
                    .collect();
                folder_removals.push(FolderRemoval {
                    id: lfr.id,
                    name: lfr.name.clone(),
                    feed_count: members.len(),
                    affected: members.iter().map(|m| m.affected).sum(),
                });
            }
        }
        for fr in &folder_removals {
            removals.push(PlanItem::Folder {
                name: fr.name.clone(),
                previous_name: None,
                changes: None,
                feed_count: Some(fr.feed_count),
                affected_article_count: Some(fr.affected),
            });
        }
        // 未推写回意图的保留策略提醒（意图不随镜像删除变化，也不被清除）。
        let pending_urls = outbox_urls_with_pending(conn, &feed_removals)?;
        if !pending_urls.is_empty() {
            warnings.push(format!(
                "{} 个被删除的源存在未推送的远端写回意图：意图保留、不因镜像删除而推送退订，也不会被清除",
                pending_urls.len()
            ));
        }
    }
    let affected_article_count: i64 = feed_removals.iter().map(|r| r.affected).sum();

    // ---- 版本签名（同一事务读快照内计算；apply 用同函数复核）----
    let removal_ids: Vec<i64> = {
        let mut ids = removed_feed_ids.iter().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    };
    let current_version = config_state_version(conn, &removal_ids)?;

    let preview = PlanPreview {
        mode,
        snapshot_hash: snapshot_hash(p)?,
        current_version,
        affected_article_count,
        additions,
        updates,
        removals,
        settings_changed_keys,
        settings_removed_keys,
        pending_connection: p
            .connection_config
            .as_ref()
            .is_some_and(|c| c.has_any_value()),
        warnings,
    };
    Ok(PreparedPlan {
        preview,
        folder_plans,
        feed_adds,
        feed_updates,
        feed_removals,
        folder_removals,
        pending_connection: p
            .connection_config
            .as_ref()
            .filter(|c| c.has_any_value())
            .cloned(),
        app_settings: p.app_settings.clone(),
        skipped_feeds,
    })
}

/* ============================================================
执行（调用方保证：已在同一事务内通过 CAS 复核）
============================================================ */

fn execute_plan(tx: &Connection, plan: &PreparedPlan) -> AppResult<ApplyOutcome> {
    // 1) 分类：新建/更新（改名/布局/标志/位置；旧库无身份时采用远端身份填空）。
    let mut slot_ids: Vec<i64> = Vec::with_capacity(plan.folder_plans.len());
    for fp in &plan.folder_plans {
        match fp {
            FolderPlan::Match {
                id,
                target,
                changes,
                adopt_uid,
            } => {
                if !changes.is_empty() {
                    db::rename_folder(tx, *id, &target.name)?;
                    db::update_folder_layout(tx, *id, &target.layout)?;
                    db::set_folder_ai_flags(tx, *id, target.summary, target.translate)?;
                    tx.execute(
                        "UPDATE folders SET position = ?1 WHERE id = ?2",
                        rusqlite::params![target.position, id],
                    )?;
                }
                if let Some(uid) = adopt_uid {
                    db::set_folder_sync_id(tx, *id, uid)?;
                }
                slot_ids.push(*id);
            }
            FolderPlan::New { target, sync_id } => {
                // P0-1：身份在**创建时**落库（远端有身份则直接采用；无身份则随机）。
                // 修前的「先 create_folder 再 set_folder_sync_id」对刚建行恒 no-op，
                // 远端身份永远进不了库 → 重放落入同名不同身份冲突。
                let id = db::create_folder_with_sync_id(
                    tx,
                    &target.name,
                    &target.layout,
                    sync_id.as_deref(),
                )?;
                db::set_folder_ai_flags(tx, id, target.summary, target.translate)?;
                tx.execute(
                    "UPDATE folders SET position = ?1 WHERE id = ?2",
                    rusqlite::params![target.position, id],
                )?;
                slot_ids.push(id);
            }
        }
    }

    // 2) 源：新增 / 更新（更新含移动——必须先于目录删除，保留源不被级联误删）。
    let mut sink_id: Option<i64> = None;
    let mut resolve_folder = |folder: &FolderRef| -> AppResult<i64> {
        match folder {
            FolderRef::Slot(slot) => slot_ids
                .get(*slot)
                .copied()
                .ok_or_else(|| AppError::internal(format!("计划内部错误：分类槽位 {slot} 无落点"))),
            FolderRef::Sink => {
                if let Some(id) = sink_id {
                    return Ok(id);
                }
                let id = match list_folder_id_by_name(tx, "导入")? {
                    Some(id) => id,
                    None => db::create_folder(tx, "导入", "article")?,
                };
                sink_id = Some(id);
                Ok(id)
            }
        }
    };
    for fa in &plan.feed_adds {
        let folder_id = resolve_folder(&fa.folder)?;
        db::insert_feed_origin_modes(
            tx,
            &fa.url,
            fa.site_url.as_deref(),
            &fa.title_for_display,
            fa.favicon_url.as_deref(),
            folder_id,
            &fa.layout,
            fa.summary,
            fa.translate,
            "local",
        )?;
    }
    for fu in &plan.feed_updates {
        let folder_id = resolve_folder(&fu.folder)?;
        tx.execute(
            "UPDATE feeds SET title = ?1, folder_id = ?2, layout = ?3,
                              auto_summary = ?4, auto_translate = ?5,
                              summary_mode = ?6, translate_mode = ?7,
                              site_url = ?8, favicon_url = ?9 WHERE id = ?10",
            rusqlite::params![
                fu.title,
                folder_id,
                fu.layout,
                fu.summary.mirrors_legacy_bool() as i64,
                fu.translate.mirrors_legacy_bool() as i64,
                fu.summary.as_str(),
                fu.translate.as_str(),
                fu.site_url,
                fu.favicon_url,
                fu.id
            ],
        )?;
    }

    // 3) app_settings 字段级合并（白名单；本地专属键保留）。
    if let Some(remote_settings) = &plan.app_settings {
        merge_app_settings(tx, remote_settings)?;
    }

    // 4) 待确认连接建议（OPT-006：绝不写活动凭据）。
    let mut pending_connection = None;
    if let Some(cc) = &plan.pending_connection {
        let version = store_pending_connection(tx, cc)?;
        pending_connection = Some(super::PendingSuggestion {
            version,
            connection: cc.clone(),
        });
    }

    // 5) 镜像删除：墓碑 + 只删列出的对象；先源后目录（目录删除时成员已清空，
    //    不会级联掉任何保留对象）。**不**调 enqueue_unsubscribe（不推远端退订）。
    for fr in &plan.feed_removals {
        db::add_feed_tombstone(tx, &fr.url)?;
        db::delete_feed(tx, fr.id)?;
    }
    for fr in &plan.folder_removals {
        db::delete_folder(tx, fr.id)?;
    }

    Ok(ApplyOutcome {
        imported: plan.feed_adds.len(),
        updated: plan.feed_updates.len(),
        skipped: plan.skipped_feeds,
        pending_connection,
        removed_feeds: plan.feed_removals.len(),
        removed_folders: plan.folder_removals.len(),
        affected_article_count: plan.preview.affected_article_count,
    })
}

/* ============================================================
指纹与查询辅助
============================================================ */

fn uid_of(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|s| !s.is_empty())
}

/// 本地配置版本签名：覆盖「会改变删除后果」的状态——
/// 分类（含身份/名称/布局/标志/位置）、源（含归属/布局/AI 模式/站点字段）、
/// 白名单 app_settings 投影、被删源的文章数/读藏聚合、被删源相关的 pending
/// 写回行。任一变化 → 旧预览失效（stalePreview）。
fn config_state_version(conn: &Connection, removal_feed_ids: &[i64]) -> AppResult<String> {
    let mut buf = String::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, COALESCE(sync_id, ''), name, layout, auto_summary, auto_translate, position
             FROM folders ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(format!(
                "F\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1e}",
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?
            ))
        })?;
        for row in rows {
            buf.push_str(&row?);
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT id, feed_url, title, folder_id, layout, summary_mode, translate_mode,
                    COALESCE(site_url, ''), COALESCE(favicon_url, '')
             FROM feeds ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(format!(
                "D\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1e}",
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?
            ))
        })?;
        for row in rows {
            buf.push_str(&row?);
        }
    }
    buf.push_str(&format!("S\u{1f}{}\u{1e}", settings_projection(conn)?));
    if !removal_feed_ids.is_empty() {
        let counts = article_state_aggregates(conn, removal_feed_ids)?;
        for (feed_id, count, read, starred) in counts {
            buf.push_str(&format!(
                "A\u{1f}{feed_id}\u{1f}{count}\u{1f}{read}\u{1f}{starred}\u{1e}"
            ));
        }
        let urls: Vec<String> = {
            let placeholders = vec!["?"; removal_feed_ids.len()].join(",");
            let sql =
                format!("SELECT feed_url FROM feeds WHERE id IN ({placeholders}) ORDER BY id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(removal_feed_ids.iter().copied()),
                |r| r.get::<_, String>(0),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut outbox = outbox_rows_for_urls(conn, &urls)?;
        outbox.sort();
        for row in outbox {
            buf.push_str(&format!("O\u{1f}{row}\u{1e}"));
        }
    }
    Ok(md5_hex(buf.as_bytes()))
}

/// 白名单设置投影（本地专属键不参与同步，也不参与失效判定）。
/// app_settings 损坏时按原文参与指纹（保守：值得让预览失效的写入就能感知）。
fn settings_projection(conn: &Connection) -> AppResult<String> {
    let Some(raw) = db::get_setting(conn, "app_settings")? else {
        return Ok(String::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(format!("raw:{raw}"));
    };
    match v.as_object() {
        Some(obj) => {
            let mut m = obj.clone();
            m.retain(|k, _| !is_local_only_setting(k));
            Ok(serde_json::to_string(&serde_json::Value::Object(m))?)
        }
        None => Ok(serde_json::to_string(&v)?),
    }
}

/// 被删源的文章状态聚合（数量/已读/收藏）——新增文章或读藏翻转都必须让
/// 「确认前展示的删除后果」失效。
fn article_state_aggregates(
    conn: &Connection,
    feed_ids: &[i64],
) -> AppResult<Vec<(i64, i64, i64, i64)>> {
    if feed_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; feed_ids.len()].join(",");
    let sql = format!(
        "SELECT feed_id, COUNT(*), COALESCE(SUM(is_read), 0), COALESCE(SUM(is_starred), 0)
         FROM articles WHERE feed_id IN ({placeholders}) GROUP BY feed_id ORDER BY feed_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(feed_ids.iter().copied()), |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn article_counts_for(conn: &Connection, feed_ids: &[i64]) -> AppResult<HashMap<i64, i64>> {
    Ok(article_state_aggregates(conn, feed_ids)?
        .into_iter()
        .map(|(id, count, _, _)| (id, count))
        .collect())
}

/// pending 写回意图的被删源 URL（预览提醒用）。
fn outbox_urls_with_pending(
    conn: &Connection,
    removals: &[FeedRemoval],
) -> AppResult<HashSet<String>> {
    if removals.is_empty() {
        return Ok(HashSet::new());
    }
    let urls: Vec<String> = removals.iter().map(|r| r.url.clone()).collect();
    let rows = outbox_rows_for_urls(conn, &urls)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.split('\u{1f}').nth(3).map(str::to_string))
        .collect())
}

/// 与给定 URL 相关的 outbox 行（id/action/status/url 规范化串，排序在调用方）。
fn outbox_rows_for_urls(conn: &Connection, urls: &[String]) -> AppResult<Vec<String>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for chunk in urls.chunks(200) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT id, action, status, feed_url FROM subscription_outbox
             WHERE feed_url IN ({placeholders}) ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok(format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/* ============================================================
app_settings：合并 + 增量（预览与执行共用同一口径）
============================================================ */

/// 计算远端 app_settings 与本地合并后的 JSON 与变化增量（changed/removed 键），
/// 与 [`merge_app_settings`] 的写入语义同源：远端存在的白名单键覆盖本地、
/// 远端缺失的白名单键本地删除、本地专属键永不参与。
pub(super) fn merged_app_settings_json(
    conn: &Connection,
    remote_raw: &str,
) -> AppResult<(serde_json::Value, Vec<String>, Vec<String>)> {
    let remote: serde_json::Value = serde_json::from_str(remote_raw)?;
    let remote_interval = super::validated_remote_refresh_interval(&remote)?;
    let local_raw = db::get_setting(conn, "app_settings")?;
    let mut merged = if let Some(local_raw) = local_raw {
        serde_json::from_str::<serde_json::Value>(&local_raw)?
    } else {
        serde_json::json!({})
    };

    let remote_obj = remote.as_object();
    let mut removed_keys = Vec::new();
    let mut changed_keys = Vec::new();
    if let Some(local_obj) = merged.as_object_mut() {
        if let Some(robj) = remote_obj {
            for k in local_obj.keys() {
                if !is_local_only_setting(k) && !robj.contains_key(k) {
                    removed_keys.push(k.clone());
                }
            }
        }
        // 变化判定以**原始本地值**为底（远端缺键 → 本地同步删除；非对象远端不做删除）
        if let Some(robj) = remote_obj {
            for (key, value) in robj {
                if is_local_only_setting(key) {
                    continue;
                }
                let normalized = remote_value_normalized(key, value, remote_interval);
                if local_obj.get(key) != Some(&normalized) {
                    changed_keys.push(key.clone());
                }
            }
            for key in &removed_keys {
                local_obj.remove(key);
            }
            for (key, value) in robj {
                if is_local_only_setting(key) {
                    continue;
                }
                local_obj.insert(
                    key.clone(),
                    remote_value_normalized(key, value, remote_interval),
                );
            }
        }
    }
    Ok((merged, changed_keys, removed_keys))
}

/// refreshInterval 写回形状与 merge 单点一致（整数值小数归一为整数）。
fn remote_value_normalized(
    key: &str,
    value: &serde_json::Value,
    interval: Option<i64>,
) -> serde_json::Value {
    if key == "refreshInterval" {
        interval
            .map(serde_json::Value::from)
            .unwrap_or_else(|| value.clone())
    } else {
        value.clone()
    }
}

/* ============================================================
单元测试：模式解析、wire 形状契约、版本签名的判别力
============================================================ */

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    /// 模式解析严格：未知/大小写变体一律拒绝（不悄悄回默认）。
    #[test]
    fn apply_mode_parse_is_strict() {
        assert_eq!(ApplyMode::parse("merge").unwrap(), ApplyMode::Merge);
        assert_eq!(ApplyMode::parse("mirror").unwrap(), ApplyMode::Mirror);
        for bad in ["Mirror", "OVERWRITE", "", "merge "] {
            let err = ApplyMode::parse(bad).unwrap_err();
            assert_eq!(err.code, "validate", "非法模式必须 validate 拒绝: {bad:?}");
        }
    }

    /// 快照 hash：对解析后的结构计算（空白差异不敏感），内容变化必变。
    #[test]
    fn snapshot_hash_is_structural_and_content_sensitive() {
        let p1: SyncPayload = serde_json::from_str(
            r#"{"schema":2,"uploaded_at":"t","folders":[],"feeds":[],"app_settings":null,"connection_config":null}"#,
        )
        .unwrap();
        let p2: SyncPayload = serde_json::from_str(
            r#"{ "schema" : 2 , "uploaded_at" : "t" , "folders" : [] , "feeds" : [] ,
                "app_settings" : null , "connection_config" : null }"#,
        )
        .unwrap();
        let p3: SyncPayload = serde_json::from_str(
            r#"{"schema":2,"uploaded_at":"t2","folders":[],"feeds":[],"app_settings":null,"connection_config":null}"#,
        )
        .unwrap();
        assert_eq!(
            snapshot_hash(&p1).unwrap(),
            snapshot_hash(&p2).unwrap(),
            "同一结构的不同空白写法必须同 hash"
        );
        assert_ne!(
            snapshot_hash(&p1).unwrap(),
            snapshot_hash(&p3).unwrap(),
            "内容变化必须换 hash"
        );
    }

    /// 预览条目 wire 形状 = 前后端契约：kind 标签 + camelCase 字段。
    #[test]
    fn plan_item_wire_shape_is_the_contract() {
        let feed = PlanItem::Feed {
            url: "https://a.example/rss".into(),
            title: "源A".into(),
            folder: "分类".into(),
            changes: Some(vec!["title".into()]),
            affected_article_count: None,
        };
        let v = serde_json::to_value(&feed).unwrap();
        assert_eq!(v["kind"], "feed");
        assert_eq!(v["url"], "https://a.example/rss");
        assert_eq!(v["changes"], serde_json::json!(["title"]));
        assert!(
            v.get("affectedArticleCount").is_none(),
            "无删除语义的条目不带该字段"
        );

        let folder = PlanItem::Folder {
            name: "分类".into(),
            previous_name: Some("旧名".into()),
            changes: None,
            feed_count: Some(2),
            affected_article_count: Some(5),
        };
        let v = serde_json::to_value(&folder).unwrap();
        assert_eq!(v["kind"], "folder");
        assert_eq!(v["previousName"], "旧名");
        assert_eq!(v["feedCount"], 2);
        assert_eq!(v["affectedArticleCount"], 5);
    }

    /// 版本签名的作用域：被删源的文章新增/读藏翻转、相关 pending 意图变化、
    /// 任何源/分类编辑都必须改变签名；**与被删对象无关的源**的文章变化不改变
    /// 签名（不制造无谓的预览失效）。
    #[test]
    fn state_signature_scopes_article_aggregates_to_removal_feeds() {
        let conn = test_conn();
        let f = crate::db::create_folder(&conn, "分类", "article").unwrap();
        let doomed = crate::db::insert_feed(
            &conn,
            "https://d.example/rss",
            None,
            "被删",
            None,
            f,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let kept = crate::db::insert_feed(
            &conn,
            "https://k.example/rss",
            None,
            "保留",
            None,
            f,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let insert = |feed: i64, guid: &str| {
            conn.execute(
                "INSERT INTO articles (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![feed, guid, guid],
            )
            .unwrap();
        };
        insert(doomed, "d1");
        insert(kept, "k1");

        let sig = |conn: &Connection| config_state_version(conn, &[doomed]).unwrap();
        let base = sig(&conn);

        // 未被删源（保留源）的新文章：删除后果不变 → 签名不变
        insert(kept, "k2");
        assert_eq!(sig(&conn), base, "与被删对象无关的文章变化不得使预览失效");

        // 被删源读藏翻转：级联删除的状态已变 → 签名必变
        conn.execute("UPDATE articles SET is_read = 1 WHERE guid = 'd1'", [])
            .unwrap();
        let after_read = sig(&conn);
        assert_ne!(after_read, base, "被删源读藏变化必须使预览失效");

        // 被删源的新文章：受影响文章数已变 → 签名必变
        insert(doomed, "d2");
        let after_article = sig(&conn);
        assert_ne!(after_article, after_read, "被删源新增文章必须使预览失效");

        // 相关 pending 写回意图出现：保留策略的输入已变 → 签名必变
        let gen = crate::db::sync_generation(&conn).unwrap();
        crate::db::enqueue_subscribe(&conn, gen, "k", "https://d.example/rss", None, None).unwrap();
        let after_outbox = sig(&conn);
        assert_ne!(
            after_outbox, after_article,
            "被删源的 pending 意图变化必须使预览失效"
        );

        // 任意源的标题编辑：白名单字段变化 → 签名必变（保守）
        conn.execute(
            "UPDATE feeds SET title = '改名' WHERE id = ?1",
            rusqlite::params![kept],
        )
        .unwrap();
        let after_retitle = sig(&conn);
        conn.execute(
            "UPDATE feeds SET title = '保留' WHERE id = ?1",
            rusqlite::params![kept],
        )
        .unwrap();
        assert_ne!(after_retitle, after_outbox, "源编辑必须使预览失效");
        assert_eq!(sig(&conn), after_outbox, "还原后签名复现（确定性）");
    }
}

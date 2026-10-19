use super::*;
use rusqlite::params;
use serde::Serialize;

/* ============================================================
OPT-011（F13）：源级 AI 开关三态 —— 唯一真值与兼容规则都收口在这里。

- 'inherit'：跟随所属分类的布尔默认（分类默认 false）；
- 'on' / 'off'：源的显式选择。显式关闭必须能压过分类开启（不以 OR 合并）。
- 旧 bool 列 auto_summary/auto_translate 保留为**旧格式兼容输入/输出**
  （旧客户端读同一库、或旧调用方继续按 bool 传参时使用），不再是有效配置
  真值：有效配置由前端唯一 resolver（selectFeedConfig）按 mode + 分类 bool
  解析，Rust 侧只存储与序列化。
- 旧 bool 镜像 = mode 的纯函数（仅显式 on 镜像 1；inherit/off 镜像 0）——
  任何写入后两列不可能分叉；inherit 镜像 0 是保守选择（降级读旧库的旧
  客户端不会因继承态而突然消耗付费 AI）。
============================================================ */

/// 源级 AI 开关三态（wire 形态：'inherit' | 'on' | 'off'）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedAiMode {
    Inherit,
    On,
    Off,
}

impl FeedAiMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FeedAiMode::Inherit => "inherit",
            FeedAiMode::On => "on",
            FeedAiMode::Off => "off",
        }
    }

    /// 旧 bool 入口的兼容映射：旧语义是「显式开关」，故 true→on、false→off
    /// （绝不映射成 inherit——那会让老调用方的 false 在新的分类默认下失效）。
    pub fn from_legacy_bool(b: bool) -> Self {
        if b {
            FeedAiMode::On
        } else {
            FeedAiMode::Off
        }
    }

    /// 旧 bool 镜像列的取值 = mode 的纯函数（只有显式 on 才是存在自动请求的
    /// 意图；inherit 由分类决定，镜像 0 保守且不烧钱）。
    pub fn mirrors_legacy_bool(self) -> bool {
        matches!(self, FeedAiMode::On)
    }

    /// 解析 IPC / 配置 payload 的字符串。非法值**拒绝**（Err），不悄悄回默认。
    pub fn parse(raw: &str) -> AppResult<Self> {
        match raw {
            "inherit" => Ok(FeedAiMode::Inherit),
            "on" => Ok(FeedAiMode::On),
            "off" => Ok(FeedAiMode::Off),
            other => Err(crate::error::AppError::new(
                "validate",
                format!("非法的 AI 开关模式：{other}（允许 inherit / on / off）"),
            )),
        }
    }

    /// 行读取：库内受 v21 CHECK 约束，理论必为合法值；损坏数据按旧 bool 列
    /// 兼容映射（读路径不 panic——损坏现场交由上层修复，不把整个列表打挂）。
    fn from_row_or_legacy(raw: Option<&str>, legacy: bool) -> Self {
        match raw {
            Some("inherit") => FeedAiMode::Inherit,
            Some("on") => FeedAiMode::On,
            Some("off") => FeedAiMode::Off,
            _ => FeedAiMode::from_legacy_bool(legacy),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedRow {
    pub id: i64,
    pub folder_id: i64,
    pub feed_url: String,
    pub site_url: Option<String>,
    pub title: String,
    pub favicon_url: Option<String>,
    pub layout: String,
    /// 旧兼容列（见上方模块注释）：不再被真实消费者直读
    pub auto_summary: bool,
    pub auto_translate: bool,
    /// 三态真值（'inherit' | 'on' | 'off'）
    pub summary_mode: String,
    pub translate_mode: String,
    pub fetch_failed: bool,
    pub fetch_error: Option<String>,
    pub last_fetched_at: Option<String>,
}

const FEED_COLS: &str = "id, folder_id, feed_url, site_url, title, favicon_url, layout, auto_summary, auto_translate, summary_mode, translate_mode, fetch_failed, fetch_error, last_fetched_at";

fn feed_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FeedRow> {
    let auto_summary = r.get::<_, i64>(7)? != 0;
    let auto_translate = r.get::<_, i64>(8)? != 0;
    let summary_mode =
        FeedAiMode::from_row_or_legacy(r.get::<_, Option<String>>(9)?.as_deref(), auto_summary);
    let translate_mode =
        FeedAiMode::from_row_or_legacy(r.get::<_, Option<String>>(10)?.as_deref(), auto_translate);
    Ok(FeedRow {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        feed_url: r.get(2)?,
        site_url: r.get(3)?,
        title: r.get(4)?,
        favicon_url: r.get(5)?,
        layout: r.get(6)?,
        auto_summary,
        auto_translate,
        summary_mode: summary_mode.as_str().to_string(),
        translate_mode: translate_mode.as_str().to_string(),
        fetch_failed: r.get::<_, i64>(11)? != 0,
        fetch_error: r.get(12)?,
        last_fetched_at: r.get(13)?,
    })
}

pub fn list_feeds(conn: &Connection) -> AppResult<Vec<FeedRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {FEED_COLS} FROM feeds ORDER BY id"))?;
    let rows = stmt.query_map([], feed_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn find_feed_by_url(conn: &Connection, url: &str) -> AppResult<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            params![url],
            |r| r.get(0),
        )
        .optional()?;
    Ok(id)
}

/// 新增订阅源。参数即 feeds 表列（URL/站点/标题/分类/布局/AI 开关）——
/// 结构与表一一对应，收窄成结构体反而隔着一层，接受 9 参。
#[allow(clippy::too_many_arguments)]
pub fn insert_feed(
    conn: &Connection,
    feed_url: &str,
    site_url: Option<&str>,
    title: &str,
    favicon_url: Option<&str>,
    folder_id: i64,
    layout: &str,
    auto_summary: bool,
    auto_translate: bool,
) -> AppResult<i64> {
    insert_feed_origin(
        conn,
        feed_url,
        site_url,
        title,
        favicon_url,
        folder_id,
        layout,
        auto_summary,
        auto_translate,
        "local",
    )
}

/// 同 insert_feed，带来源标记（'local' 用户直连添加 | 'remote' 服务端拉取）。
/// 断开连接按此列清理服务端来源订阅（换账号不混杂）。
/// OPT-011：旧 bool 入口的兼容语义 = 显式开关（true→on / false→off），
/// 不映射成 inherit——老调用方传 false 的源不得借新语义悄悄开启。
#[allow(clippy::too_many_arguments)]
pub fn insert_feed_origin(
    conn: &Connection,
    feed_url: &str,
    site_url: Option<&str>,
    title: &str,
    favicon_url: Option<&str>,
    folder_id: i64,
    layout: &str,
    auto_summary: bool,
    auto_translate: bool,
    origin: &str,
) -> AppResult<i64> {
    insert_feed_origin_modes(
        conn,
        feed_url,
        site_url,
        title,
        favicon_url,
        folder_id,
        layout,
        FeedAiMode::from_legacy_bool(auto_summary),
        FeedAiMode::from_legacy_bool(auto_translate),
        origin,
    )
}

/// 三态入口：新调用方（add_feed 命令 / 配置导入）以 mode 为真值插入。
/// bool 兼容列由 mode 纯函数镜像（见 FeedAiMode::mirrors_legacy_bool）。
#[allow(clippy::too_many_arguments)]
pub fn insert_feed_origin_modes(
    conn: &Connection,
    feed_url: &str,
    site_url: Option<&str>,
    title: &str,
    favicon_url: Option<&str>,
    folder_id: i64,
    layout: &str,
    summary_mode: FeedAiMode,
    translate_mode: FeedAiMode,
    origin: &str,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO feeds (feed_url, site_url, title, favicon_url, folder_id, layout,
                            auto_summary, auto_translate, summary_mode, translate_mode, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            feed_url,
            site_url,
            title,
            favicon_url,
            folder_id,
            layout,
            summary_mode.mirrors_legacy_bool() as i64,
            translate_mode.mirrors_legacy_bool() as i64,
            summary_mode.as_str(),
            translate_mode.as_str(),
            origin
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_feed(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM feeds WHERE id = ?1", params![id])?;
    Ok(())
}

/// 刷新结果落库：成功清零失败计数并立即可再抓；失败则递增计数并按
/// 指数退避（5min→30min→2h 封顶）推迟下次尝试。
pub fn set_feed_fetch_state(
    conn: &Connection,
    id: i64,
    failed: bool,
    error: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET
            fetch_failed = ?1,
            fetch_error = ?2,
            etag = ?3,
            last_modified = ?4,
            last_fetched_at = datetime('now'),
            fail_count = CASE WHEN ?1 THEN fail_count + 1 ELSE 0 END,
            next_retry_at = CASE
                WHEN ?1 THEN datetime('now', '+' || (CASE fail_count
                    WHEN 0 THEN 5 WHEN 1 THEN 5 WHEN 2 THEN 30 ELSE 120 END) || ' minutes')
                ELSE NULL END
         WHERE id = ?5",
        params![failed as i64, error, etag, last_modified, id],
    )?;
    Ok(())
}

/// 调度器取"到期"的源：超过全局间隔未抓 且 不在退避窗口内。
/// last_fetched_at 为 NULL（从未抓过）的源立即视为到期。
/// 到期源 id。`include_remote = false`（跟随服务端同步模式）时跳过
/// origin='remote' 的源——服务端源的内容由 Miniflux 同步提供，
/// 直连抓取会与服务端状态产生两份不一致的真相。
pub fn feeds_due_for_refresh(
    conn: &Connection,
    interval_min: i64,
    include_remote: bool,
) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM feeds
         WHERE (last_fetched_at IS NULL
            OR ( (julianday('now') - julianday(last_fetched_at)) * 1440.0 >= ?1
                 AND (next_retry_at IS NULL OR julianday('now') >= julianday(next_retry_at)) ))
           AND (?2 OR origin != 'remote')",
    )?;
    let rows = stmt.query_map(params![interval_min, include_remote], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 全部源 id（托盘「刷新全部订阅」与手动全刷入口，忽略到期与退避）。
/// 手动入口始终包含 Miniflux 源（用户显式动作 = 要全部内容）。
pub fn feeds_all_ids(conn: &Connection, include_remote: bool) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM feeds WHERE (?1 OR origin != 'remote')")?;
    let rows = stmt.query_map(params![include_remote], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn set_feed_title_and_icon(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    favicon: Option<&str>,
    site_url: Option<&str>,
) -> AppResult<()> {
    let title = title.filter(|t| !t.trim().is_empty());
    let favicon = favicon.filter(|f| !f.trim().is_empty());
    // 只覆盖非空值：用户手动重命名的标题不被下一次抓取冲掉
    conn.execute(
        "UPDATE feeds SET
            title = COALESCE(?1, title),
            favicon_url = COALESCE(?2, favicon_url),
            site_url = COALESCE(?3, site_url)
         WHERE id = ?4",
        params![title, favicon, site_url, id],
    )?;
    Ok(())
}

pub fn update_feed_layout(conn: &Connection, id: i64, layout: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE feeds SET layout = ?1 WHERE id = ?2",
        params![layout, id],
    )?;
    Ok(())
}

/// 编辑源一次性落库：标题 / 所属分类 / 布局 / AI 开关（单语句，避免多次往返）。
/// OPT-011：旧 bool 参数是兼容输入——写入时映射为显式 on/off 并同步三态列与
/// bool 镜像列（见 update_feed_full）；mode 是新真值入口，两者不可能分叉。
pub fn update_feed(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    folder_id: Option<i64>,
    layout: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
) -> AppResult<()> {
    update_feed_full(
        conn,
        id,
        title,
        folder_id,
        layout,
        auto_summary,
        auto_translate,
        None,
        None,
    )
}

/// 编辑源全量入口（命令/editFeed 共用）：旧 bool 与三态 mode 可同时给出，
/// 合并规则单点在本函数——mode 优先，缺 mode 时由旧 bool 映射显式 on/off，
/// 两者皆缺 = 该字段不动。bool 兼容列恒为 mode 镜像（纯函数，不分叉）。
#[allow(clippy::too_many_arguments)]
pub fn update_feed_full(
    conn: &Connection,
    id: i64,
    title: Option<&str>,
    folder_id: Option<i64>,
    layout: Option<&str>,
    auto_summary: Option<bool>,
    auto_translate: Option<bool>,
    summary_mode: Option<FeedAiMode>,
    translate_mode: Option<FeedAiMode>,
) -> AppResult<()> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let summary_mode = summary_mode.or(auto_summary.map(FeedAiMode::from_legacy_bool));
    let translate_mode = translate_mode.or(auto_translate.map(FeedAiMode::from_legacy_bool));
    // 空标题回退原值（不置 NULL——源名必填）
    conn.execute(
        "UPDATE feeds SET
            title = COALESCE(?1, title),
            folder_id = COALESCE(?2, folder_id),
            layout = COALESCE(?3, layout),
            summary_mode = COALESCE(?4, summary_mode),
            translate_mode = COALESCE(?5, translate_mode),
            auto_summary = COALESCE(?6, auto_summary),
            auto_translate = COALESCE(?7, auto_translate)
         WHERE id = ?8",
        params![
            title,
            folder_id,
            layout,
            summary_mode.map(|m| m.as_str()),
            translate_mode.map(|m| m.as_str()),
            summary_mode.map(|m| m.mirrors_legacy_bool() as i64),
            translate_mode.map(|m| m.mirrors_legacy_bool() as i64),
            id
        ],
    )?;
    Ok(())
}

/// 仅写三态字段（配置导入等「mode 已是真值」的入口）。
pub fn set_feed_ai_modes(
    conn: &Connection,
    id: i64,
    summary_mode: Option<FeedAiMode>,
    translate_mode: Option<FeedAiMode>,
) -> AppResult<()> {
    update_feed_full(
        conn,
        id,
        None,
        None,
        None,
        None,
        None,
        summary_mode,
        translate_mode,
    )
}

/// 旧入口（bool 语义 = 显式开关）：必须写显式 on/off 到三态列并镜像 bool——
/// 旧 bool 与 mode 不留分叉（OPT-011 定版）。
pub fn set_feed_ai_flags(
    conn: &Connection,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    let summary_mode = FeedAiMode::from_legacy_bool(summary);
    let translate_mode = FeedAiMode::from_legacy_bool(translate);
    conn.execute(
        "UPDATE feeds SET
            summary_mode = ?1, translate_mode = ?2,
            auto_summary = ?3, auto_translate = ?4
         WHERE id = ?5",
        params![
            summary_mode.as_str(),
            translate_mode.as_str(),
            summary_mode.mirrors_legacy_bool() as i64,
            translate_mode.mirrors_legacy_bool() as i64,
            id
        ],
    )?;
    Ok(())
}

/* ============================================================
Articles
============================================================ */

/* ============================================================
订阅删除墓碑（A-1）：本地删除后 pull 不得按远端列表复活。
存 app_settings 的 JSON 数组（规范化 URL），避免为一次删除语义新增迁移；
退订成功（远端确认）后清除。

R1（OPT-008A）：URL 规范化算法收敛（通用参数不再全局剥除）后，旧算法时代的
墓碑键与新算法键不再相等——只保字节会在 pull 的 stale 判据下失配被误清，
已删订阅复活。v19 把旧键迁入独立 legacy 命名空间；匹配/回收跨两个命名空间，
旧规则用 legacy_v1_normalize 隔离快照复刻，**绝不用回身份匹配**。
============================================================ */
const FEED_TOMBSTONE_KEY: &str = "feed_tombstones";
/// 旧算法（v1）时代的退订墓碑（v19 从 FEED_TOMBSTONE_KEY 迁入；键为旧算法
/// 规范化产物，原始 URL 已丢）。仅用于防复活的兼容匹配/回收。
const FEED_TOMBSTONE_LEGACY_V1_KEY: &str = "feed_tombstones_legacy_v1";

/// 读墓碑列表：**缺 key = 合法空表**（从未删过订阅）；存在但读/解析失败 =
/// **未知**——必须 Err（R2：退化空表会把用户删除意图当不存在而放行复活）。
/// 调用方不得吞错继续导入；损坏的 setting 原值保持不动，修复后自然重试。
fn read_tombstones(conn: &Connection, key: &str) -> AppResult<Vec<String>> {
    let Some(raw) = super::get_setting(conn, key)? else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<String>>(&raw).map_err(|e| {
        crate::error::AppError::new(
            "db",
            format!("订阅墓碑 {key} 内容损坏（保留原值，等待修复）: {e}"),
        )
    })
}

fn save_tombstones(conn: &Connection, key: &str, list: &[String]) -> AppResult<()> {
    let raw = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    super::set_setting(conn, key, &raw)
}

/// 删除订阅的墓碑 URL 列表（当前算法命名空间，规范化）。
pub fn feed_tombstones(conn: &Connection) -> AppResult<Vec<String>> {
    read_tombstones(conn, FEED_TOMBSTONE_KEY)
}

/// 旧算法命名空间的退订墓碑（见常量注释）。
pub fn legacy_feed_tombstones(conn: &Connection) -> AppResult<Vec<String>> {
    read_tombstones(conn, FEED_TOMBSTONE_LEGACY_V1_KEY)
}

// Note: 墓碑是 pull 防复活的唯一防线；清除判据是「远端列表已不含该 URL」而非 2xx — 见 .agents/notes/implemented/bug-fix/2026-09-18-双向同步的写入接线与墓碑.md
/// 写入删除墓碑（幂等，按当前算法规范化 URL 去重）。
pub fn add_feed_tombstone(conn: &Connection, feed_url: &str) -> AppResult<()> {
    let norm = super::normalize_url(feed_url);
    let mut list = feed_tombstones(conn)?;
    if !list.iter().any(|u| u == &norm) {
        list.push(norm);
        save_tombstones(conn, FEED_TOMBSTONE_KEY, &list)?;
    }
    Ok(())
}

/// 清除删除墓碑（用户显式重新添加同 URL 时调用——commands::persist_new_feed
/// 的真实解除点）：当前与 legacy 两个命名空间一并解除（旧墓碑键只存字节，
/// 不按旧规则重算就无法被显式重加解除）。
/// 退订 2xx / 远端列表滞后等路径不得调用（见 subscriptions::unsubscribe_remote）。
pub fn remove_feed_tombstone(conn: &Connection, feed_url: &str) -> AppResult<()> {
    let norm = super::normalize_url(feed_url);
    let mut list = feed_tombstones(conn)?;
    let before = list.len();
    list.retain(|u| u != &norm);
    if list.len() != before {
        save_tombstones(conn, FEED_TOMBSTONE_KEY, &list)?;
    }
    let legacy_norm = super::legacy_v1_normalize(feed_url);
    let mut legacy = legacy_feed_tombstones(conn)?;
    let before = legacy.len();
    legacy.retain(|u| u != &legacy_norm);
    if legacy.len() != before {
        save_tombstones(conn, FEED_TOMBSTONE_LEGACY_V1_KEY, &legacy)?;
    }
    Ok(())
}

/// 该远端 URL 是否被删除墓碑压制（跨算法版本）：当前命名空间按新算法键比对；
/// legacy 命名空间按隔离的旧算法快照比对。保守权衡——旧键丢失了参数原值，
/// 所有旧键候选（如 ?t=123 与 ?t=456）会一同被抑制，不能猜 raw 恢复。
pub fn feed_url_tombstoned(conn: &Connection, feed_url: &str) -> AppResult<bool> {
    let norm = super::normalize_url(feed_url);
    if feed_tombstones(conn)?.iter().any(|t| t == &norm) {
        return Ok(true);
    }
    let legacy_norm = super::legacy_v1_normalize(feed_url);
    Ok(legacy_feed_tombstones(conn)?
        .iter()
        .any(|t| t == &legacy_norm))
}

/// 墓碑收敛（pull 的「远端列表已不含」判据，跨算法版本）：各命名空间只按
/// 自己的算法全集判 stale——legacy 墓碑只有「远端旧规范全集确实不含该键」时
/// 才清。保留中的墓碑一律不动；除远端事实（与用户显式重加）外不主动丢弃
/// 用户删除意图。
/// R2：先读完两侧再写——任一读取/解析失败在任何 setting 被改动之前返回，
/// 损坏值原样保留（未知状态不产生任何副作用）。
pub fn prune_feed_tombstones(conn: &Connection, remote_urls: &[String]) -> AppResult<()> {
    let mut current = feed_tombstones(conn)?;
    let mut legacy = legacy_feed_tombstones(conn)?;
    let remote_current: HashSet<String> = remote_urls
        .iter()
        .map(|u| super::normalize_url(u))
        .collect();
    let before = current.len();
    current.retain(|k| remote_current.contains(k));
    if current.len() != before {
        save_tombstones(conn, FEED_TOMBSTONE_KEY, &current)?;
    }
    let remote_legacy: HashSet<String> = remote_urls
        .iter()
        .map(|u| super::legacy_v1_normalize(u))
        .collect();
    let before = legacy.len();
    legacy.retain(|k| remote_legacy.contains(k));
    if legacy.len() != before {
        save_tombstones(conn, FEED_TOMBSTONE_LEGACY_V1_KEY, &legacy)?;
    }
    Ok(())
}

/// 清空两个命名空间的退订墓碑（OPT-006：**账号身份变更**的提交事务内调用）。
///
/// 语义边界：墓碑记录的是「用户在本机删过这个订阅」；换到**另一个账号**后，
/// 旧账号的删除意图不再适用——新账号的订阅列表里若有同 URL，应当正常导入，
/// 否则旧墓碑会永久压制新账号的订阅（F05「旧映射不污染新账号」）。
/// **断开连接不调用**（未换账号）：重连同一账号时墓碑仍须防复活（TASK-055）。
pub fn clear_feed_tombstones(conn: &Connection) -> AppResult<()> {
    save_tombstones(conn, FEED_TOMBSTONE_KEY, &[])?;
    save_tombstones(conn, FEED_TOMBSTONE_LEGACY_V1_KEY, &[])?;
    Ok(())
}

/// 订阅的同步信息：URL 与远端绑定 id（删除订阅时用于退订与墓碑）。
pub fn feed_remote_info(conn: &Connection, id: i64) -> AppResult<(String, Option<i64>)> {
    conn.query_row(
        "SELECT feed_url, remote_id FROM feeds WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
    )
    .map_err(Into::into)
}

/// 订阅当前所属分类的 label（移动编辑时用于 wire 的 r= 旧 label；
/// OPT-009 阶段 B：只发 a= 是追加分类，不是移动）。
pub fn feed_folder_label(conn: &Connection, id: i64) -> AppResult<Option<String>> {
    let folder_id: Option<i64> = conn
        .query_row("SELECT folder_id FROM feeds WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    match folder_id {
        Some(fid) => super::folders::folder_name(conn, fid),
        None => Ok(None),
    }
}

/* ============================================================
OPT-011 单元测试：三态写入口——显式开关保留、bool 镜像不分叉、非法值拒绝
============================================================ */
#[cfg(test)]
mod ai_mode_tests {
    use super::*;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        super::super::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn seed(conn: &Connection) -> i64 {
        let folder = super::super::create_folder(conn, "分类", "article").unwrap();
        insert_feed(
            conn,
            "https://f.example/rss",
            None,
            "源",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap()
    }

    fn modes(conn: &Connection, id: i64) -> (String, String, i64, i64) {
        conn.query_row(
            "SELECT summary_mode, translate_mode, auto_summary, auto_translate
               FROM feeds WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }

    /// 旧 bool 入口 insert_feed 的兼容语义：false/true 映射显式 off/on，
    /// 不引入 inherit；bool 镜像与 mode 一致（不分叉）。
    #[test]
    fn legacy_insert_maps_bool_to_explicit_modes() {
        let conn = test_conn();
        let id = seed(&conn);
        assert_eq!(
            modes(&conn, id),
            ("off".to_string(), "off".to_string(), 0, 0),
            "旧入口传 false 必须是显式 off（若写成 inherit，分类开启后旧源会意外烧 AI）"
        );
        let id2 = insert_feed(
            &conn,
            "https://g.example/rss",
            None,
            "源2",
            None,
            1,
            "inherit",
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            modes(&conn, id2),
            ("on".to_string(), "off".to_string(), 1, 0),
            "旧入口传 true 保留为显式 on"
        );
    }

    /// 三态插入（新调用方）：inherit 是真值；bool 镜像保守为 0。
    #[test]
    fn mode_insert_keeps_inherit_and_mirrors_zero() {
        let conn = test_conn();
        super::super::create_folder(&conn, "分类", "article").unwrap();
        let id = insert_feed_origin_modes(
            &conn,
            "https://h.example/rss",
            None,
            "源3",
            None,
            1,
            "inherit",
            FeedAiMode::Inherit,
            FeedAiMode::On,
            "local",
        )
        .unwrap();
        assert_eq!(
            modes(&conn, id),
            ("inherit".to_string(), "on".to_string(), 0, 1),
            "inherit 原样保留（bool 镜像 0），显式 on 镜像 1"
        );
    }

    /// 旧入口 set_feed_ai_flags（bool 语义 = 显式开关）必须写显式 on/off 到
    /// 三态列并同步 bool 镜像——不留「旧 bool 与 mode 分叉」。
    #[test]
    fn set_feed_ai_flags_writes_explicit_modes_and_mirrors() {
        let conn = test_conn();
        let id = seed(&conn);
        // 先写成 inherit，模拟新 UI 默认；旧入口显式关必须能压过它
        set_feed_ai_modes(
            &conn,
            id,
            Some(FeedAiMode::Inherit),
            Some(FeedAiMode::Inherit),
        )
        .unwrap();
        set_feed_ai_flags(&conn, id, true, false).unwrap();
        assert_eq!(
            modes(&conn, id),
            ("on".to_string(), "off".to_string(), 1, 0),
            "旧入口必须落显式 on/off（不是只改 bool 列）"
        );
        set_feed_ai_flags(&conn, id, false, true).unwrap();
        assert_eq!(
            modes(&conn, id),
            ("off".to_string(), "on".to_string(), 0, 1),
            "反向显式选择同样落三态"
        );
    }

    /// update_feed_full：mode 优先于旧 bool；只给 mode 时 bool 镜像跟 mode；
    /// 只给旧 bool 时映射显式 on/off；两者皆缺该字段不动；
    /// 未涉及字段（如 layout）不被连带改动。
    #[test]
    fn update_feed_full_merges_mode_and_legacy_bool_single_truth() {
        let conn = test_conn();
        let id = seed(&conn);
        set_feed_ai_modes(&conn, id, Some(FeedAiMode::On), Some(FeedAiMode::Off)).unwrap();
        assert_eq!(
            modes(&conn, id),
            ("on".to_string(), "off".to_string(), 1, 0)
        );

        // 只改 translate → summary 保持 on
        update_feed_full(
            &conn,
            id,
            None,
            None,
            Some("social"),
            None,
            None,
            None,
            Some(FeedAiMode::Inherit),
        )
        .unwrap();
        let (sm, tm, sb, tb) = modes(&conn, id);
        assert_eq!(
            (sm.as_str(), tm.as_str(), sb, tb),
            ("on", "inherit", 1, 0),
            "只改一个字段不连带另一个；inherit 镜像 0"
        );
        let layout: String = conn
            .query_row("SELECT layout FROM feeds WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(layout, "social", "同语句内的 layout 更新照常生效");

        // mode 与旧 bool 同时给出且冲突 → mode 优先（单点合并规则，逐字段）：
        // summary 给 mode=off 压过 auto_summary=true；translate 未给 mode，
        // 按 auto_translate=true 映射为显式 on。bool 镜像恒随 mode（0/1）。
        update_feed_full(
            &conn,
            id,
            None,
            None,
            None,
            Some(true),
            Some(true),
            Some(FeedAiMode::Off),
            None,
        )
        .unwrap();
        assert_eq!(
            modes(&conn, id),
            ("off".to_string(), "on".to_string(), 0, 1),
            "冲突时 mode 是唯一真值（off 压过旧 bool=true；未给 mode 的字段按旧 bool 映射），bool 镜像随 mode"
        );

        // 只给旧 bool（旧 caller 编辑路径）→ 显式映射
        update_feed(&conn, id, None, None, None, Some(false), Some(true)).unwrap();
        assert_eq!(
            modes(&conn, id),
            ("off".to_string(), "on".to_string(), 0, 1),
            "旧 caller 的 bool 输入保留显式开关语义"
        );
    }

    /// 非法 mode 串在解析层拒绝（不悄悄回默认）——IPC/配置导入共用这一入口。
    #[test]
    fn illegal_mode_string_is_rejected() {
        assert_eq!(FeedAiMode::parse("inherit").unwrap(), FeedAiMode::Inherit);
        assert_eq!(FeedAiMode::parse("on").unwrap(), FeedAiMode::On);
        assert_eq!(FeedAiMode::parse("off").unwrap(), FeedAiMode::Off);
        let err = FeedAiMode::parse("maybe").unwrap_err();
        assert_eq!(err.code, "validate");
        assert!(err.message.contains("maybe"), "错误必须指出非法值: {err}");
    }
}

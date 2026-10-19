use super::*;
use crate::error::AppError;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct FolderRow {
    pub id: i64,
    pub name: String,
    pub layout: String,
    pub auto_summary: bool,
    pub auto_translate: bool,
    pub collapsed: bool,
    pub position: i64,
    /// OPT-010：配置快照的稳定身份（跨设备匹配键；v26 起每行必有）。
    pub sync_id: Option<String>,
}

pub fn list_folders(conn: &Connection) -> AppResult<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, layout, auto_summary, auto_translate, collapsed, position, sync_id
         FROM folders ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FolderRow {
            id: r.get(0)?,
            name: r.get(1)?,
            layout: r.get(2)?,
            auto_summary: r.get::<_, i64>(3)? != 0,
            auto_translate: r.get::<_, i64>(4)? != 0,
            collapsed: r.get::<_, i64>(5)? != 0,
            position: r.get(6)?,
            sync_id: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn create_folder(conn: &Connection, name: &str, layout: &str) -> AppResult<i64> {
    create_folder_with_sync_id(conn, name, layout, None)
}

/// OPT-010（P0-1）：**身份采用**的创建入口——远端快照带 `sync_id` 时直接以它
/// 落库。修前路径是 `create_folder`（先随机分配）再 `set_folder_sync_id`
/// （「只填空」谓词对刚创建的行恒为 no-op）——远端身份永远进不了库，重放时
/// 落入「同名不同身份」冲突。`sync_id = None`（本地手工创建 / 远端 schema1）
/// → 生成随机身份，保持「创建即分配」语义；调用方不得用本函数改写既有行。
pub fn create_folder_with_sync_id(
    conn: &Connection,
    name: &str,
    layout: &str,
    sync_id: Option<&str>,
) -> AppResult<i64> {
    let next_pos: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM folders",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let uid = sync_id.map(str::trim).filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO folders (name, position, layout, collapsed, sync_id)
         VALUES (?1, ?2, ?3, 1, COALESCE(?4, lower(hex(randomblob(16)))))",
        params![name, next_pos, layout, uid],
    )?;
    Ok(conn.last_insert_rowid())
}

/// OPT-010：导入侧「旧库无身份 + 名称命中」的保守采用——只填空，绝不改写
/// 已有身份（同名不同身份是不同对象，由预览显式列出冲突，不在这里猜）。
pub fn set_folder_sync_id(conn: &Connection, id: i64, sync_id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET sync_id = ?1 WHERE id = ?2 AND (sync_id IS NULL OR trim(sync_id) = '')",
        params![sync_id, id],
    )?;
    Ok(())
}

pub fn rename_folder(conn: &Connection, id: i64, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    Ok(())
}

pub fn delete_folder(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn update_folder_layout(conn: &Connection, id: i64, layout: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET layout = ?1 WHERE id = ?2",
        params![layout, id],
    )?;
    Ok(())
}

pub fn set_folder_collapsed(conn: &Connection, id: i64, collapsed: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET collapsed = ?1 WHERE id = ?2",
        params![collapsed as i64, id],
    )?;
    Ok(())
}

/// 分类 AI 默认的整对写入（旧 full 语义，配置导入/旧 caller 使用）。
pub fn set_folder_ai_flags(
    conn: &Connection,
    id: i64,
    summary: bool,
    translate: bool,
) -> AppResult<()> {
    conn.execute(
        "UPDATE folders SET auto_summary = ?1, auto_translate = ?2 WHERE id = ?3",
        params![summary as i64, translate as i64, id],
    )?;
    Ok(())
}

/// OPT-011：分类 AI 默认的单字段 patch（None = 不改该字段）。
///
/// 缺陷背景（成功乱序回退）：旧整对写入要求调用方快照「另一字段」的现值随行——
/// 前端 toggleCatSummary 快照 translate、toggleCatTranslate 快照 summary；两次
/// 切换并发在途且到达乱序时（A(true,false) 迟到于 B(true,true)），整对快照会把
/// B 已写入的另一字段用**旧值覆盖**回退。单字段 patch 后每次写入只携带当前
/// 变更字段：两字段独立落库，乱序到达也不互相覆盖。
///
/// 两字段皆缺 = 无写入目标：拒绝（validate），避免空转写与误用。
pub fn set_folder_ai_flags_partial(
    conn: &Connection,
    id: i64,
    summary: Option<bool>,
    translate: Option<bool>,
) -> AppResult<()> {
    if summary.is_none() && translate.is_none() {
        return Err(crate::error::AppError::new(
            "validate",
            "至少给出一个要更新的分类 AI 开关字段",
        ));
    }
    conn.execute(
        "UPDATE folders SET
            auto_summary = COALESCE(?1, auto_summary),
            auto_translate = COALESCE(?2, auto_translate)
         WHERE id = ?3",
        params![summary.map(|b| b as i64), translate.map(|b| b as i64), id],
    )?;
    Ok(())
}

/* ============================================================
Feeds
============================================================ */

/// 分类名（id → name）：订阅编辑推送远端时用于 `a=` 目标分类参数。
/// P3-2（自检 2026-09-29）：区分「目录不存在」（Ok(None)）与「查询失败」（Err
/// 上抛）——此前 `.ok()` 把 DB 错误也压成 None，push_feeds 会静默把订阅挂到
/// 远端默认分类（目录结构丢失）且现场无线索。Err 的处置权交给调用方。
pub fn folder_name(conn: &Connection, id: i64) -> AppResult<Option<String>> {
    match conn.query_row("SELECT name FROM folders WHERE id = ?1", [id], |r| r.get(0)) {
        Ok(name) => Ok(Some(name)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/* ============================================================
分类墓碑（A-4；OPT-009 阶段 B2 起按账号 key 隔离）
改名/删除分类后，pull 不得按远端旧 label 复活目录——改名会留下重复空目录，
删除会让目录连同其内订阅一起回来。远端 tag/list 不再列出该 label 后清除。

**持久形状（v24）**：settings['folder_tombstones_v2'] =
`[{"account_key": "<协议|规范化服务器根|用户名>", "label": "..."}]`
- `account_key` 非空 = 该墓碑只在**这一账号**的 pull 里生效；账号切换后
  异账号的 pull 既不应用也不删除它（回原账号仍防复活）。旧形状是裸 label
  数组（无归属）：A 删除的分类会压制 B 的同名分类（B2-4 确认的跨账号误抑制）。
- `account_key` 为空串 = **无账号归属**（纯本地用户写的墓碑，或 v24 迁移时库内
  无配置账号）：对任何账号生效——本地删除意图不得因之后连接账号而失效。
- v24 迁移把旧键的每条 label 归属给**迁移时配置的账号**（无配置 → 空 key），
  转换成功后删除旧键（数据已迁移，不是丢弃；失败整体回滚、旧键原样保留）。
- **回收只限归属账号**（R2-1 复审 P2）：非空 key 且等于当前账号的条目，才在
  远端确认缺席后回收；空 key 的全局墓碑**不**被单账号 pull 回收。
============================================================ */
// Note: 账号隔离/迁移归属/读取失败关闭 — 见 .agents/notes/implemented/architecture/2026-10-14-分类墓碑账号隔离.md
/// 账号隔离的分类墓碑持久键（v24 起读写此键）。
pub(crate) const FOLDER_TOMBSTONE_KEY_V2: &str = "folder_tombstones_v2";
/// v24 迁移前的旧键（迁移后不再读写；常量供迁移与升级兼容测试引用）。
pub(crate) const FOLDER_TOMBSTONE_KEY_LEGACY: &str = "folder_tombstones";

/// 一条分类墓碑：label + 写入它的账号 key（空串 = 无账号归属，对所有账号生效）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct FolderTombstone {
    pub account_key: String,
    pub label: String,
}

/// 全部分类墓碑（两种归属都返回；调用方按 [`folder_tombstones_for`] 取生效集合，
/// 或按 [`prune_folder_tombstones`] 回收）。
///
/// 失败关闭：v24 迁移应已消费旧键；旧键**仍残留**说明迁移时内容损坏被原样
/// 保留（见 v24 迁移注释）或用户手工写回了旧键——未知状态不得按空表继续
/// （空表会让已改名/删除的分类全部按远端 label 复活）。读取/解析失败同样上抛。
pub fn folder_tombstones(conn: &Connection) -> AppResult<Vec<FolderTombstone>> {
    if let Some(raw) = super::get_setting(conn, FOLDER_TOMBSTONE_KEY_LEGACY)? {
        if !raw.trim().is_empty() && raw.trim() != "[]" {
            return Err(AppError::new(
                "db",
                format!(
                    "旧分类墓碑未迁移（内容损坏，原值保留于 settings['{FOLDER_TOMBSTONE_KEY_LEGACY}']）；\
                     为保证不复活已删/已改名分类，本轮停止分类导入——请修复或清空该键后重试"
                ),
            ));
        }
    }
    let raw = super::get_setting(conn, FOLDER_TOMBSTONE_KEY_V2)?;
    match raw {
        // R2 纪律（同 feed 墓碑）：损坏值必须可见地上抛，不得按空表继续——
        // 空表会让已改名/删除的分类全部按远端 label 复活。
        Some(raw) => serde_json::from_str::<Vec<FolderTombstone>>(&raw).map_err(|e| {
            AppError::new("db", format!("分类墓碑内容损坏（保留原值，等待修复）: {e}"))
        }),
        None => Ok(Vec::new()),
    }
}

fn save_folder_tombstones(conn: &Connection, list: &[FolderTombstone]) -> AppResult<()> {
    let raw = serde_json::to_string(list).map_err(|e| AppError::internal(e.to_string()))?;
    super::set_setting(conn, FOLDER_TOMBSTONE_KEY_V2, &raw)
}

/// 该账号当前生效的分类墓碑 label（含无归属的空 key 墓碑）。
pub fn folder_tombstones_for(conn: &Connection, account_key: &str) -> AppResult<Vec<String>> {
    Ok(folder_tombstones(conn)?
        .into_iter()
        .filter(|t| t.account_key.is_empty() || t.account_key == account_key)
        .map(|t| t.label)
        .collect())
}

/// 写入分类墓碑（幂等，按 (account_key, label) 去重）。
/// `account_key` 传当前账号 key；纯本地（未配置同步）传空串（对所有账号生效）。
pub fn add_folder_tombstone(conn: &Connection, account_key: &str, label: &str) -> AppResult<()> {
    let mut list = folder_tombstones(conn)?;
    let exists = list
        .iter()
        .any(|t| t.account_key == account_key && t.label == label);
    if !exists {
        list.push(FolderTombstone {
            account_key: account_key.to_string(),
            label: label.to_string(),
        });
        save_folder_tombstones(conn, &list)?;
    }
    Ok(())
}

/// 回收分类墓碑（远端 tag/list 已不含该 label 后调用）：**只回收归属当前账号
/// 且远端确认缺席的墓碑**（`account_key` 非空且逐字等于当前账号）。
/// - 空 key 的**全局**墓碑一律不回收（R2-1 复审 P2）：它对所有账号生效，而单个
///   账号的远端缺席不能证明其它/未来账号也缺席——A 的一次 pull 让全局墓碑消失，
///   B 之后就会把同名分类按远端 label 导入复活（契约要求本地删除意图不得因连接
///   某个账号而失效）。全局墓碑没有自动解除路径，寿命代价见决定笔记。
/// - 异账号墓碑一个字节都不动（B2-4：B 的 pull 不得删除 A 的墓碑——回 A 后仍要
///   防复活）。
///
/// 先读完再写：读取/解析失败在任何写入之前上抛，损坏值原样保留。
pub fn prune_folder_tombstones(
    conn: &Connection,
    account_key: &str,
    remote_labels: &[String],
) -> AppResult<()> {
    let mut list = folder_tombstones(conn)?;
    let before = list.len();
    list.retain(|t| {
        let prunable = !t.account_key.is_empty() && t.account_key == account_key;
        !prunable || remote_labels.iter().any(|l| l == &t.label)
    });
    if list.len() != before {
        save_folder_tombstones(conn, &list)?;
    }
    Ok(())
}

/// 某分类下的订阅成员 (feed_url, remote_id) 快照——**级联删除前**必须先取：
/// 目录删除会级联删 feeds（V2 起含文章），远端退订目标（remote_id）只在删除前
/// 可得（B2-2：每个实际远端成员都要有独立可恢复的退订意图）。
pub fn folder_members(conn: &Connection, folder_id: i64) -> AppResult<Vec<(String, Option<i64>)>> {
    let mut stmt = conn.prepare("SELECT feed_url, remote_id FROM feeds WHERE folder_id = ?1")?;
    let rows = stmt.query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P3-2（自检 2026-09-29）：folder_name 必须区分三种结果——存在（Some）、
    /// 目录不存在（Ok(None)）、查询失败（Err 上抛）。修前 `.ok()` 把 Err 压成
    /// Ok(None)，push_feeds 据此把订阅静默挂到远端默认分类且无线索。
    /// 查询失败用「表改名」注入真实 DB 错误（修前此分支返回 Ok(None)）。
    #[test]
    fn folder_name_distinguishes_missing_from_error() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();

        let fid = create_folder(&conn, "技术", "article").unwrap();
        assert_eq!(
            folder_name(&conn, fid).unwrap(),
            Some("技术".to_string()),
            "存在的目录返回名字"
        );
        assert_eq!(
            folder_name(&conn, 999).unwrap(),
            None,
            "目录不存在是 Ok(None)，不是 Err"
        );

        conn.execute_batch("ALTER TABLE folders RENAME TO folders_backup;")
            .unwrap();
        let err = folder_name(&conn, fid).unwrap_err();
        assert_eq!(
            err.code, "db",
            "查询失败必须上抛 Err（修前被 .ok() 压成 Ok(None) 静默降级）"
        );
        conn.execute_batch("ALTER TABLE folders_backup RENAME TO folders;")
            .unwrap();
        assert_eq!(
            folder_name(&conn, fid).unwrap(),
            Some("技术".to_string()),
            "恢复后查询正常（注入未破坏夹具）"
        );
    }

    /* ---------- OPT-011：分类 AI 单字段 patch（成功乱序不互相覆盖） ---------- */

    fn flags(conn: &Connection, id: i64) -> (i64, i64) {
        conn.query_row(
            "SELECT auto_summary, auto_translate FROM folders WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// 单字段 patch 只动目标字段、另一字段保持原值；旧 full 写入（兼容 caller）
    /// 语义不变；两字段皆缺拒绝。判别力：某字段写入若把另一字段带回旧值
    /// （快照式整对写的行为），下面的中间断言即红。
    #[test]
    fn partial_folder_ai_flag_only_touches_target_field() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        let id = create_folder(&conn, "分类", "article").unwrap();

        // 旧 full caller：整对写入照旧
        set_folder_ai_flags(&conn, id, true, false).unwrap();
        assert_eq!(flags(&conn, id), (1, 0), "旧 full caller 语义保持");

        // 单字段 translate=true → summary 保持 1（不被快照旧值覆盖）
        set_folder_ai_flags_partial(&conn, id, None, Some(true)).unwrap();
        assert_eq!(flags(&conn, id), (1, 1), "只改 translate，summary 保持");

        // 单字段 summary=false → translate 保持 1
        // （等价于乱序场景：各字段独立落库，谁先谁后都不带回对方的旧值）
        set_folder_ai_flags_partial(&conn, id, Some(false), None).unwrap();
        assert_eq!(flags(&conn, id), (0, 1), "只改 summary，translate 保持");

        // 两字段皆缺：validate 拒绝（不空转写、不误当 no-op）
        let err = set_folder_ai_flags_partial(&conn, id, None, None).unwrap_err();
        assert_eq!(err.code, "validate");
        assert_eq!(flags(&conn, id), (0, 1), "拒绝路径不得改动任何字段");
    }

    /* ---------- OPT-010（P0-1）：导入创建的身份采用入口 ---------- */

    /// `create_folder_with_sync_id`：远端有身份 → 直落（不是先随机再「只填空」——
    /// 那对刚创建的行恒为 no-op）；无身份/空白 → 随机生成（创建即分配语义保持）。
    /// 判别力：修前唯一入口随机身份占位，第一条断言（== uid-adopt）必红。
    #[test]
    fn create_folder_with_sync_id_adopts_or_assigns() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();

        let adopted =
            create_folder_with_sync_id(&conn, "采用身份", "article", Some("uid-adopt")).unwrap();
        let uid: String = conn
            .query_row(
                "SELECT sync_id FROM folders WHERE id = ?1",
                [adopted],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(uid, "uid-adopt", "远端身份必须直接在创建时落库");

        let uids = |id: i64| -> String {
            conn.query_row("SELECT sync_id FROM folders WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let random = create_folder_with_sync_id(&conn, "随机身份", "article", None).unwrap();
        let random_uid = uids(random);
        assert!(
            !random_uid.trim().is_empty() && random_uid != "uid-adopt",
            "无远端身份时仍创建即分配（随机且不与采用值重复）: {random_uid}"
        );
        let blank = create_folder_with_sync_id(&conn, "空白身份", "article", Some("   ")).unwrap();
        let blank_uid = uids(blank);
        assert!(
            !blank_uid.trim().is_empty() && blank_uid != "uid-adopt",
            "空白身份按无身份处理（随机生成，不写空白串）: {blank_uid}"
        );
        assert_eq!(uids(adopted), "uid-adopt", "后续创建不得改写既有行的身份");
    }

    /* ---------- OPT-009 B2-R2（R2-1）：墓碑回收只限归属账号 ---------- */

    /// R2-1：`prune_folder_tombstones` 只回收「**归属当前账号**且远端确认缺席」的
    /// 条目——空 key 的全局墓碑不被单账号 pull 回收（它对所有账号生效，单个账号
    /// 的远端缺席不能证明其它/未来账号也缺席；否则 A 的 pull 会让 B 随后按远端
    /// label 把已删分类导入复活）。异账号墓碑不回收；归属账号的墓碑在远端仍列出
    /// 时保留（判据不变）。
    /// 判别力：旧实现把空 key 视同当前账号的活动墓碑——第一条断言（全局墓碑仍在）
    /// 与「A 的 pull 后全局墓碑对其他账号仍压制」的语义必红。
    #[test]
    fn prune_only_reclaims_tombstones_owned_by_current_account() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::MIGRATIONS.to_latest(&mut conn).unwrap();
        add_folder_tombstone(&conn, "", "全局已删").unwrap();
        add_folder_tombstone(&conn, "acct-a", "A已删").unwrap();
        add_folder_tombstone(&conn, "acct-a", "A仍列出").unwrap();
        add_folder_tombstone(&conn, "acct-b", "B已删").unwrap();

        prune_folder_tombstones(&conn, "acct-a", &["A仍列出".to_string()]).unwrap();

        let left: Vec<(String, String)> = folder_tombstones(&conn)
            .unwrap()
            .into_iter()
            .map(|t| (t.account_key, t.label))
            .collect();
        assert!(
            left.contains(&(String::new(), "全局已删".to_string())),
            "全局（空 key）墓碑不得被单账号 pull 回收: {left:?}"
        );
        assert!(
            !left.iter().any(|(k, l)| k == "acct-a" && l == "A已删"),
            "归属当前账号且远端确认缺席 → 回收: {left:?}"
        );
        assert!(
            left.contains(&("acct-a".to_string(), "A仍列出".to_string())),
            "远端仍列出 → 归属账号的墓碑保留（既有判据不变）: {left:?}"
        );
        assert!(
            left.contains(&("acct-b".to_string(), "B已删".to_string())),
            "异账号墓碑不得被本账号 pull 回收: {left:?}"
        );

        // 全局墓碑对所有账号生效（B 的读取集合必含它；B 自己的条目也在）
        let active_b = folder_tombstones_for(&conn, "acct-b").unwrap();
        assert!(
            active_b.contains(&"全局已删".to_string()),
            "空 key 全局墓碑必须继续对所有账号生效: {active_b:?}"
        );
    }
}

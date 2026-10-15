//! 类型化订阅动作 outbox（OPT-009 阶段 A）。
//!
//! 订阅写回（新增/改名/移动/退订）不再依赖「本地改完 best-effort 推一次」：
//! 本地修改与队列入队在同一事务落库，推送失败 = 队项保留重试；远端确认按
//! (id, account_generation) 精确出队。与 article 状态队列 `sync_queue` 分离——
//! feed 删除级联、`purge_remove_feed_zombies` 都不会清掉这里的行（退订目标
//! 必须在本地 feed 消亡后仍存活）。
//!
//! ## 身份与代际
//! - `account_key` = 协议 + 规范化服务器根 + 用户名（持久身份，跨代际不变）；
//! - `account_generation` = 入队时会话代际（同一账号改密码也会推进）。
//!
//! 推送门槛：key 匹配当前会话才可发；key 匹配但代际落后 → 重绑代际后发送
//! （同账号的意图仍然有效）；key 不匹配 → 暂停（换号不得把旧意图发给新账号，
//! 回原账号后恢复可发）。
//!
//! ## 合并与确认
//! 连续修改合并「最后 desired」= 删除同 URL 旧行 + 插入新行。id 为 AUTOINCREMENT
//! 不复用：旧 HTTP 计划按旧 id 确认只会命中「已不存在」的行，不可能误删中途
//! 改写的新目标（F01 的同一纪律，见 db/migrations.rs v18/v23 注释）。
//! 移动的移除基准（wire 的 `r=`）必须指向「远端实际持有的 label」：未发送链折叠
//! 保留链起点；发送成功后由执行器把替代行重定为已发送目标（B1-R3 复审 P2-1）。
// Note: 移除基准/合并与重定语义 — 见 .agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md
// Note: 操作 id 不复用是旧推送计划按 id 精确确认的前提 — 见 .agents/notes/implemented/bug-fix/2026-09-18-状态写入事务化与对账守卫.md

use super::*;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub const ACTION_SUBSCRIBE: &str = "subscribe";
pub const ACTION_EDIT: &str = "edit";
pub const ACTION_UNSUBSCRIBE: &str = "unsubscribe";
/// subscribe 的阶段：远端尚未创建。
pub const STAGE_PENDING: &str = "pending";
/// subscribe 的阶段：quick_add 已成功（remote_id 已带回），待本地绑定/分类挂载确认。
pub const STAGE_REMOTE_CREATED: &str = "remote_created";
pub const STATUS_PENDING: &str = "pending";
/// 远端协议不支持该写回（Fever）：本地已完成、远端明确不宣称——终态，
/// 不再重试，也不能用 Ok 冒充远端完成。
pub const STATUS_LOCAL_ONLY: &str = "local_only";

/// 一条订阅 outbox 队项（类型化 payload；重启后仍完整可用）。
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionOutboxItem {
    pub id: i64,
    pub account_generation: i64,
    pub account_key: String,
    pub action: String,
    pub feed_url: String,
    pub remote_id: Option<i64>,
    /// 恢复路径（`remote_id IS NULL` 的退订）经幂等 `quick_add` 解析出的已验证 id。
    /// 与 `remote_id` 分开：解析结果**不**等于「入队时已知的远端绑定」——恢复路径
    /// 必须在解析之外继续做列表发现（防非幂等服务端的重复创建）。
    /// 在列表/发送之前持久化：列表长期失败或重启后不重复 quick_add（B2-R2）。
    pub resolved_remote_id: Option<i64>,
    pub desired_title: Option<String>,
    pub desired_folder: Option<String>,
    /// 编辑前的分类 label：移动时 wire 需要 r= 移除旧 label（仅追加 a= 不是移动）。
    /// 退订行上 = 删除时快照的「远端当前 label」移除基准：若该退订行被重加替换，
    /// 后继 subscribe 行继承它（否则远端残留旧 label，B2-R2/R2-3）。
    pub prev_folder: Option<String>,
    pub stage: String,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// 源新增入队（本地入库同一事务内调用）。
/// 合并规则：**同账号**（account_key 相同）同 URL 的既有队项一律替换为新意图——
/// 新意图即该账号视角的最新本地状态；异账号同 URL 队项**保留**（换号暂停，
/// 回原账号后仍可发——B 的本地动作不得删除 A 的远端已建意图造成孤儿）。
/// 同时清掉同 URL 的历史 `add_feed` 兼容队项（该队列随账号 purge，无跨账号
/// 归属问题；新 outbox 已接管订阅推送，留着会造成双份 quick_add）。
///
/// `desired_title` = 用户/导入给定的**非空**目标标题（None = 不宣称远端标题）：
/// quick_add 只按 URL 建订阅、远端标题由服务端发现——导入/命名的标题必须作为
/// 目标随队项落库，挂载时用真实 `t=` 写回（B2-5）。空标题不产生 `t=` 目标。
///
/// **被替换的 pending 退订行携带的移除基准必须继承（B2-R2/R2-3）**：删除后快速
/// 重加同一 URL 时，退订很可能尚未被服务端确认——远端订阅仍持有旧 label，而
/// quick_add（幂等）会返回它的 id；若重加行不带 `prev_folder`，挂载只发 `a=` 会
/// 永久残留旧 label。继承只读被替换行**已持久化**的基准，不猜测。
pub fn enqueue_subscribe(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    desired_title: Option<&str>,
    desired_folder: Option<&str>,
) -> AppResult<()> {
    let inherited_prev: Option<String> = conn
        .query_row(
            "SELECT prev_folder FROM subscription_outbox
              WHERE feed_url = ?1 AND account_key = ?2 AND status = 'pending'
                AND prev_folder IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            params![feed_url, account_key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1 AND account_key = ?2",
        params![feed_url, account_key],
    )?;
    conn.execute(
        "DELETE FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, desired_title, desired_folder,
             prev_folder)
         VALUES (?1, ?2, 'subscribe', ?3, ?4, ?5, ?6)",
        params![
            account_generation,
            account_key,
            feed_url,
            desired_title,
            desired_folder,
            inherited_prev
        ],
    )?;
    Ok(())
}

/// 退订入队（本地删除 feed 的同一事务内调用）。
/// `remote_id`：删除时已知的远端绑定；None 时从**同 account_key** 的既有队项
/// 结转（quick_add 已建、本地绑定尚未完成的场景——远端其实存在，退订目标不能丢）。
/// 异账号一律不结转：A 的远端目标不得被 B 的退订借用（与代际屏障同源）。
/// 替换/删除同样只限**同账号**同 URL：B 的退订不得删除 A 的队项。
///
/// **未知远端目标也必须落可恢复意图（B2-R1 复审 P2）**：删除发生在同账号同 URL
/// 仍有 pending 队项时（典型：`quick_add` 已发出、响应在途/丢失/进程重启），远端
/// 可能已经存在这次创建——此刻无从得知 id，但意图必须持久化：写入 `remote_id`
/// 为 NULL 的 unsubscribe 行，由执行器在发送前用**幂等 quick_add-by-URL**解析出
/// 已验证 id 再退订（见 `exec_unsubscribe_item`）。没有创建证据（同账号同 URL 无
/// pending 队项）时不写行——从未推送过，远端不可能有它，写了只会让执行器对不存在
/// 的订阅做一次创建+删除。**删除先于 quick_add 发出**的普通场景正落在这一分支：
/// 无队项可删、无在途请求，本地墓碑已足够。
/// 返回 true = 已写入退订队项（`remote_id` 可能待解析）；false = 无创建证据、未写入。
///
/// `prev_folder` = **删除时快照的「远端当前 label」移除基准**（本地分类 label；
/// 成员级联删除时是被删目录名）：退订未确认前同一 URL 被重加时，后继 subscribe
/// 行继承它做 `r=` 移除基准（B2-R2/R2-3）。纯本地删除/无 label 场景传 None。
///
/// **链起点优先（B2-R2 复审 P3-A）**：被替换的 pending 行若带 `prev_folder`
/// （未发送移动链的起点 = **远端实际持有的 label**，同 B1-R3 的合并教义），它以
/// 调用方快照为准更接近远端真值——「本地已 A→B 但移动未发送」时本地是 B、
/// 远端仍是 A，用 B 做 `r=` 是 no-op、远端 A 永不移除。没有链起点才退回调用方快照。
/// （发送成功后执行器会把替代行的基准重定为已接受目标，见
/// [`rebase_successor_edit_prev`]——本函数只需要给出「此刻的远端状态」。）
pub fn enqueue_unsubscribe(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    remote_id: Option<i64>,
    prev_folder: Option<&str>,
) -> AppResult<bool> {
    let inherited: Option<i64> = conn
        .query_row(
            "SELECT remote_id FROM subscription_outbox
              WHERE feed_url = ?1 AND account_key = ?2 AND remote_id IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            params![feed_url, account_key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let inherited_prev: Option<String> = conn
        .query_row(
            "SELECT prev_folder FROM subscription_outbox
              WHERE feed_url = ?1 AND account_key = ?2 AND status = 'pending'
                AND prev_folder IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            params![feed_url, account_key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    // 创建证据必须在删除之前采集：同账号同 URL 仍有 pending 队项 = 可能已发出
    // quick_add（响应在途/丢失），远端可能已被创建。旧账号的队项不算证据
    // （key 屏障：A 的在途创建与 B 的本地删除无关）。
    let may_exist_remote: bool = conn.query_row(
        "SELECT COUNT(*) FROM subscription_outbox
          WHERE feed_url = ?1 AND account_key = ?2 AND status = 'pending'",
        params![feed_url, account_key],
        |r| r.get::<_, i64>(0),
    )? > 0;
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1 AND account_key = ?2",
        params![feed_url, account_key],
    )?;
    conn.execute(
        "DELETE FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        params![feed_url],
    )?;
    let rid = remote_id.or(inherited);
    if rid.is_none() && !may_exist_remote {
        return Ok(false);
    }
    // 链起点优先于调用方快照（P3-A）：被替换行仍带未发送移动链的起点时，那就是
    // 远端当前持有的 label。没有链起点（如被替换的 subscribe 行尚无 prev）才用快照。
    let base = inherited_prev.or_else(|| prev_folder.map(str::to_string));
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, remote_id, prev_folder)
         VALUES (?1, ?2, 'unsubscribe', ?3, ?4, ?5)",
        params![account_generation, account_key, feed_url, rid, base],
    )?;
    Ok(true)
}

/// 合并基线：既有 pending 行的 (desired_title, desired_folder, prev_folder, action)。
/// 独立命名满足 clippy::type_complexity（与测试里的槽位类型同一惯例）。
type OutboxMergeBaseline = (Option<String>, Option<String>, Option<String>, String);

/// 订阅编辑（改名/移动）入队。`desired_*` 是本字段的新目标（None = 本次不改该
/// 字段）；`prev_folder` 是移动前的分类 label（wire 需要 r= 移除旧 label）。
///
/// 合并纪律（「连续操作合并最后目标」）：与同账号同 URL 的既有 pending 行按
/// **字段**合并——本次给出的字段覆盖旧值、未给出的字段沿用旧行（改名后紧接着
/// 移动，不得丢掉尚未推送的改名目标）。合并换新 id（不复用），旧 HTTP 计划按
/// 旧 id 确认只会落空。
///
/// **移除基准（r=）不是「新操作的 prev」而是「远端实际持有的 label」**：
/// 既有 pending **edit 行仍带未发送的移动目标**（desired_folder 非空）时，
/// 远端尚未收到任何移动——它持有的仍是那条链的**起点**（既有行的 prev_folder）。
/// A→B 未发送时再 B→C：新操作的 prev 是本地已生效的 B，若覆盖链起点，最终只发
/// `a=C,r=B`，远端的 A 永不移除（B1-R3 复审 P2-1）。与既有 subscribe 行合并
/// （remote_created 窗口）或既有行只是改名时，远端持有的就是新操作读到的旧 label，
/// 用新 prev（此时既有行的 prev 为空或不存在）。
///
/// 同时清掉同 URL 的历史 `add_feed` 兼容队项（避免 quick_add + edit 双推送；
/// 该队列随账号 purge，无跨账号归属问题）。异账号同 URL 队项保留。
#[allow(clippy::too_many_arguments)] // 入队参数即 wire 意图字段，拆 struct 只是搬运
pub fn enqueue_edit(
    conn: &Connection,
    account_generation: i64,
    account_key: &str,
    feed_url: &str,
    remote_id: i64,
    desired_title: Option<&str>,
    desired_folder: Option<&str>,
    prev_folder: Option<&str>,
) -> AppResult<()> {
    // 合并基线：同账号同 URL 的既有 pending 队项（若有）。
    let previous: Option<OutboxMergeBaseline> = conn
        .query_row(
            "SELECT desired_title, desired_folder, prev_folder, action FROM subscription_outbox
              WHERE feed_url = ?1 AND account_key = ?2 AND status = 'pending'
              ORDER BY id DESC LIMIT 1",
            params![feed_url, account_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (prev_title, prev_desired_folder, prev_prev_folder, prev_action) =
        previous.unwrap_or((None, None, None, String::new()));
    let title = desired_title.map(str::to_string).or(prev_title);
    let folder = desired_folder
        .map(str::to_string)
        .or_else(|| prev_desired_folder.clone());
    // 既有 edit 行带未发送移动目标 → 远端仍在链起点；用链起点作为移除基准。
    let chain_origin_is_remote_state = prev_action == ACTION_EDIT && prev_desired_folder.is_some();
    let old_label = if chain_origin_is_remote_state {
        prev_prev_folder.clone()
    } else {
        prev_folder.map(str::to_string).or(prev_prev_folder)
    };
    conn.execute(
        "DELETE FROM subscription_outbox WHERE feed_url = ?1 AND account_key = ?2",
        params![feed_url, account_key],
    )?;
    conn.execute(
        "DELETE FROM sync_queue WHERE action = 'add_feed' AND feed_url = ?1",
        params![feed_url],
    )?;
    conn.execute(
        "INSERT INTO subscription_outbox
            (account_generation, account_key, action, feed_url, remote_id,
             desired_title, desired_folder, prev_folder)
         VALUES (?1, ?2, 'edit', ?3, ?4, ?5, ?6, ?7)",
        params![
            account_generation,
            account_key,
            feed_url,
            remote_id,
            title,
            folder,
            old_label
        ],
    )?;
    Ok(())
}

/// remote_created 窗口的编辑合并原语：本地订阅尚无 remote_id、但存在同账号
/// 同 URL 的 pending **subscribe** 行时，把本地编辑的目标字段并入该行——
/// 绑定义务不因「编辑时还没有 remote_id」而丢失（执行器绑定后按该目标补推）。
/// 合并语义同 [`enqueue_edit`]：本次给出的字段覆盖、未给出的沿用。
///
/// `prev_folder` 是本次移动的旧 label（远端挂载若已按旧 label 发过，需要 `r=`
/// 移除；B2-3 的分类改名会经此表达，member 订阅行因此第一次带上 prev）。
/// 只取**首值**（`COALESCE(prev_folder, ?)`）：链起点才是远端实际持有的 label，
/// 后续再改目标不得把移除基准改到已生效的中间值。只在本次同时给出目标分类时
/// 生效（无移动的调用不得凭空写入移除基准）。
/// 返回 true = 命中了 pending subscribe 行并已更新。
pub fn update_pending_subscribe_desired(
    conn: &Connection,
    account_key: &str,
    feed_url: &str,
    title: Option<&str>,
    folder: Option<&str>,
    prev_folder: Option<&str>,
) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET desired_title = COALESCE(?1, desired_title),
                desired_folder = COALESCE(?2, desired_folder),
                prev_folder = CASE WHEN ?2 IS NULL THEN prev_folder
                                   ELSE COALESCE(prev_folder, ?3) END,
                updated_at = datetime('now')
          WHERE feed_url = ?4 AND account_key = ?5 AND action = 'subscribe' AND status = 'pending'",
        params![title, folder, prev_folder, feed_url, account_key],
    )?;
    Ok(n > 0)
}

/// 响应在途期间订阅行被替换（合并/退订/删除）时，把 quick_add 已创建的
/// remote_id 补给**同账号同代际**的替代 pending 行，避免远端孤儿：
/// - 替代行是 unsubscribe（本地已删）→ 填入远端 id，下一轮退订收敛；
/// - 替代行是 subscribe（合并/重新添加）→ 填入远端 id 并推进 remote_created，
///   下一轮直接绑定（quick_add 幂等，本可不填；填上避免重复请求）。
///
/// 只在行还处于「无 remote_id」时补——已有目标的行使旧响应彻底落空（不覆盖）。
pub fn adopt_remote_id_for_pending(
    conn: &Connection,
    account_key: &str,
    account_generation: i64,
    feed_url: &str,
    remote_id: i64,
) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET remote_id = ?1,
                stage = CASE WHEN action = 'subscribe' THEN 'remote_created' ELSE stage END,
                updated_at = datetime('now')
          WHERE feed_url = ?2 AND account_key = ?3 AND account_generation = ?4
            AND status = 'pending' AND remote_id IS NULL
            AND action IN ('subscribe', 'unsubscribe')
            AND id = (SELECT MAX(id) FROM subscription_outbox
                       WHERE feed_url = ?2 AND account_key = ?3 AND account_generation = ?4
                         AND status = 'pending' AND remote_id IS NULL
                         AND action IN ('subscribe', 'unsubscribe'))",
        params![remote_id, feed_url, account_key, account_generation],
    )?;
    Ok(n > 0)
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubscriptionOutboxItem> {
    Ok(SubscriptionOutboxItem {
        id: r.get(0)?,
        account_generation: r.get(1)?,
        account_key: r.get(2)?,
        action: r.get(3)?,
        feed_url: r.get(4)?,
        remote_id: r.get(5)?,
        resolved_remote_id: r.get(6)?,
        desired_title: r.get(7)?,
        desired_folder: r.get(8)?,
        prev_folder: r.get(9)?,
        stage: r.get(10)?,
        status: r.get(11)?,
        attempts: r.get(12)?,
        last_error: r.get(13)?,
    })
}

const SELECT_COLS: &str = "id, account_generation, account_key, action, feed_url, remote_id,
     resolved_remote_id, desired_title, desired_folder, prev_folder, stage, status, attempts,
     last_error";

/// 全部待推送队项（status='pending'，按入队顺序）。
pub fn pending_outbox(conn: &Connection) -> AppResult<Vec<SubscriptionOutboxItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLS} FROM subscription_outbox WHERE status = 'pending' ORDER BY id"
    ))?;
    let rows = stmt.query_map([], row_from)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// 同账号（key 相同）但代际落后的待推队项重绑到当前代际：
/// 同一账号改密码/重新保存会推进代际，旧意图仍属于该账号，重绑后继续可发。
/// key 不匹配的行一律不touch（换号后暂停，回原账号才恢复）。
pub fn rebind_outbox_generation(
    conn: &Connection,
    account_key: &str,
    account_generation: i64,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE subscription_outbox SET account_generation = ?1, updated_at = datetime('now')
          WHERE account_key = ?2 AND account_generation <> ?1 AND status = 'pending'",
        params![account_generation, account_key],
    )?;
    Ok(n)
}

/// subscribe 已越过 quick_add：记录 remote_id 并进入 remote_created 阶段
/// （后续只做本地绑定与分类挂载，重启后不会重新 quick_add）。
/// 守卫 (id, generation, action='subscribe', stage='pending', status='pending')：
/// 过期响应不得推进新行，也不得重复推进已进入 remote_created 的行；
/// 阶段只为 subscribe 定义，edit/unsubscribe 行不得被写进 remote_created
/// 或改写 remote_id（R2/P3-3）。
pub fn advance_subscribe_stage(
    conn: &Connection,
    id: i64,
    account_generation: i64,
    remote_id: i64,
) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET stage = 'remote_created', remote_id = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND account_generation = ?3 AND action = 'subscribe'
            AND stage = 'pending' AND status = 'pending'",
        params![remote_id, id, account_generation],
    )?;
    Ok(n > 0)
}

/// 退订恢复路径的**解析推进**（B2-R2 复审 P2）：`remote_id IS NULL` 的退订行经
/// 幂等 `quick_add`-by-URL 解析出已验证 id 后，在列表发现/发送之前先把结果落库——
/// 订阅列表长期失败或进程重启后，下一轮直接采用该 id，**不再重复 quick_add**
/// （非幂等 quick_add 的服务端上，每重复一次都会新建一个重复订阅）。
///
/// 守卫 (id, generation, action='unsubscribe', status='pending', remote_id IS NULL)：
/// 过期响应/换代/被合并替换/已确认的行不得被旧解析结果推进 —— 未推进时调用方
/// 本轮不发请求、不确认（行保持未知 id，由新意图或回原账号的轮次接管）。
pub fn advance_unsubscribe_resolution(
    conn: &Connection,
    id: i64,
    account_generation: i64,
    resolved_remote_id: i64,
) -> AppResult<bool> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET resolved_remote_id = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND account_generation = ?3 AND action = 'unsubscribe'
            AND status = 'pending' AND remote_id IS NULL",
        params![resolved_remote_id, id, account_generation],
    )?;
    Ok(n > 0)
}

/// 精确确认出队：只删「本轮计划的那一行、代际未变、仍是 pending」。
/// 中途被合并改写的队项是新 id（AUTOINCREMENT 不复用），旧 id 的确认天然落空。
pub fn prune_outbox(conn: &Connection, ids: &[i64], account_generation: i64) -> AppResult<usize> {
    let mut n = 0usize;
    for id in ids {
        n += conn.execute(
            "DELETE FROM subscription_outbox
              WHERE id = ?1 AND account_generation = ?2 AND status = 'pending'",
            params![id, account_generation],
        )?;
    }
    Ok(n)
}

/// subscribe 行的「目标未变」精确确认出队（OPT-009 阶段 B1-R2 复审 P2）：
/// 只有行仍是同一 id/代际/pending，且 desired_title/desired_folder/prev_folder
/// 与执行器**实际发送**的目标逐字段相等（NULL 敏感）才删除。
///
/// 为什么 id+generation 不够：remote_created 窗口的本地编辑经
/// [`update_pending_subscribe_desired`] **原地**更新同一行（id/代际都不变）。
/// 若挂载/补 edit 的 HTTP 在途期间目标 A→B，旧成功按 id 出队会把**从未发送**的
/// B 意图静默删除。比对失败 = 保留队项：下一轮发送最新目标（最后写入者获胜）。
///
/// `sent_prev`（B2-3）：挂载的 `r=` 也参与比对——发送时无 r 而在途期间补上旧
/// label（分类改名/移动）时，确认必须落空，否则远端会残留该旧 label。
pub fn confirm_subscribe_if_desired_unchanged(
    conn: &Connection,
    id: i64,
    account_generation: i64,
    sent_title: Option<&str>,
    sent_folder: Option<&str>,
    sent_prev: Option<&str>,
) -> AppResult<bool> {
    let n = conn.execute(
        "DELETE FROM subscription_outbox
          WHERE id = ?1 AND account_generation = ?2 AND action = 'subscribe' AND status = 'pending'
            AND desired_title IS ?3 AND desired_folder IS ?4 AND prev_folder IS ?5",
        params![id, account_generation, sent_title, sent_folder, sent_prev],
    )?;
    Ok(n > 0)
}

/// 编辑发送成功、确认落空（队项已被合并替换）时，把替代行的**移除基准**
/// 重定为「远端刚刚接受的目标 label」。
///
/// 为什么需要：合并纪律要求「未发送链保留链起点」（见 [`enqueue_edit`]），
/// 而链起点只在**没有任何发送成功**时等于远端当前状态。若在途的旧编辑已经
/// 成功（远端此刻就是已发送目标），替代行仍按链起点发 `r=` 就移除不了远端
/// 现在持有的 label（A→B 已推达、在途再 B→C 时远端会残留 B）。
/// 发送成功 = 远端当前 label 即该次发送的目标——只有执行器在确认落空那一刻
/// 知道这个事实，故由它回写替代行。
///
/// **作用域是账号 key，不是 (key, generation)**（B2-1）：同账号改密码/重连也会
/// 推进代际，而「远端已接受该目标」是**账号作用域**的事实——若按代际限定，
/// 换代后（executor 的代际复核已失配、确认阶段被跳过）后继行会保留过期链起点，
/// 下一轮发 `r=链起点`，远端长期残留已发送的中间 label。跨账号安全由 key 守卫
/// 保证：本函数只改**同一账号 key** 的行，别的账号队列一个字节都不动。
///
/// 其余守卫：同 URL / 仍 pending；幂等（已是该值不动）。
///
/// **三类替代行**（B2-R1 复审 F-1；B2-R2 复审 P3-A/D 起统一为「改写」）：
/// - `action='edit'`：移动链上的后继（A→B 已推达、在途再 B→C）——重定到已接受
///   目标（已是该值不动）；
/// - `action='subscribe'`：**删除/退订后又重新添加**同一 URL 的后续行——把移除
///   基准重定为已接受目标；`prev` 为空时补填、非空时改写（R2 复审前只补空：
///   P3-A 起删除的基准可能是「链起点快照」，而发送成功的 2xx 才是远端当下的
///   label，链起点已过期——不改写则挂载发 `r=链起点`，远端残留已接受目标）；
/// - `action='unsubscribe'`：删除撞上在途移动留下的退订行（尚未被重加替换）——
///   同样改写：它将来被重加继承时，基准必须是远端当下的 label。
///
/// `desired_folder IS NOT NULL` 仍限定 edit/subscribe 两类（没有移动目标的行
/// 不参与）；`unsubscribe` 行没有目标字段，只按上面的条件改写。
pub fn rebase_successor_edit_prev(
    conn: &Connection,
    account_key: &str,
    feed_url: &str,
    remote_folder_after_send: &str,
) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE subscription_outbox
            SET prev_folder = ?1, updated_at = datetime('now')
          WHERE feed_url = ?2 AND account_key = ?3 AND status = 'pending'
            AND (
                 (action = 'edit' AND desired_folder IS NOT NULL AND prev_folder IS NOT ?1)
              OR (action = 'subscribe' AND desired_folder IS NOT NULL AND prev_folder IS NOT ?1)
              OR (action = 'unsubscribe' AND prev_folder IS NOT ?1)
            )",
        params![remote_folder_after_send, feed_url, account_key],
    )?;
    Ok(n)
}

/// 该 id 的行是否仍在队（status='pending'）。执行器据此判断「本轮发送成功的
/// edit 行是否已被后继意图替换」：仍在 = 没有后继，不必重定移除基准；
/// 已不在 = 被合并替换（新 id）或已被删除——前者需要按已接受目标重定
/// （见 [`rebase_successor_edit_prev`]）。id 为 AUTOINCREMENT 不复用，
/// 故单看 id 即可唯一识别队项。
pub fn outbox_pending_by_id(conn: &Connection, id: i64) -> AppResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subscription_outbox WHERE id = ?1 AND status = 'pending'",
        params![id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 远端协议不支持：置 local_only 终态（保留展示，不重试、不冒充完成）。
/// 入参 (id, account_generation)：代际守卫防「过期计划的标记」落到同 id 但代际
/// 已推进/重绑的行上（与 mark_outbox_failed 同源，R2/P3-2）。
pub fn mark_outbox_local_only(conn: &Connection, items: &[(i64, i64)]) -> AppResult<usize> {
    let mut n = 0usize;
    for (id, generation) in items {
        n += conn.execute(
            "UPDATE subscription_outbox
                SET status = 'local_only', updated_at = datetime('now')
              WHERE id = ?1 AND account_generation = ?2 AND status = 'pending'",
            params![id, generation],
        )?;
    }
    Ok(n)
}

/// 失败标记：attempts +1、last_error 摘要（截断口径与 sync_queue 一致）。
/// 入参 (id, account_generation, 摘要)：代际守卫防「过期计划的失败」记到同 id
/// 但代际已推进的行上（id 不复用盖不住重绑这一维，R2/P3-2）。
pub fn mark_outbox_failed(conn: &Connection, failed: &[(i64, i64, String)]) -> AppResult<()> {
    if failed.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE subscription_outbox
            SET attempts = attempts + 1, last_error = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND account_generation = ?3 AND status = 'pending'",
    )?;
    for (id, generation, summary) in failed {
        stmt.execute(params![
            super::sync_queue::truncate_last_error(summary),
            id,
            generation
        ])?;
    }
    Ok(())
}

/// 订阅写回统计（UI 持久展示）：pending = 待推/待重试；failed = pending 中
/// attempts>0；local_only = 远端不支持、仅本地生效的终态项。**按当前账号
/// 限定**（B3）：异账号（换号暂停）的行不参与任何计数与最近错误——UI 只展示
/// 当前账号的真值；读取失败原样 Err（调用方不得当「无待办」）。
#[derive(Debug, Default, Serialize)]
pub struct SubscriptionOutboxStats {
    pub pending: usize,
    pub failed: usize,
    pub local_only: usize,
    pub last_error: Option<String>,
}

pub fn subscription_outbox_stats(
    conn: &Connection,
    account_key: &str,
) -> AppResult<SubscriptionOutboxStats> {
    let (pending, failed, local_only, last_error): (i64, i64, i64, Option<String>) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM subscription_outbox
                  WHERE status = 'pending' AND account_key = ?1),
                (SELECT COUNT(*) FROM subscription_outbox
                  WHERE status = 'pending' AND attempts > 0 AND account_key = ?1),
                (SELECT COUNT(*) FROM subscription_outbox
                  WHERE status = 'local_only' AND account_key = ?1),
                (SELECT last_error FROM subscription_outbox
                  WHERE status = 'pending' AND attempts > 0 AND last_error IS NOT NULL
                    AND account_key = ?1
                  ORDER BY id DESC LIMIT 1)",
            params![account_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    Ok(SubscriptionOutboxStats {
        pending: pending as usize,
        failed: failed as usize,
        local_only: local_only as usize,
        last_error,
    })
}

/// `subscription-outbox-changed` 事件 payload（纯函数，wire 形态单点 + 可测）：
/// `{pending, failed, local_only, last_error}` 与 [`SubscriptionOutboxStats`] /
/// 前端 `SubscriptionOutboxStats`（src/lib/api.ts）三方同形。configured=false →
/// None：未配置同步静默不发事件（与 queue_changed_payload 同一「未配置不刷
/// 状态」纪律）。payload 只承载计数与错误摘要——不带 URL、账号或任何凭据。
pub fn subscription_outbox_changed_payload(
    stats: &SubscriptionOutboxStats,
    configured: bool,
) -> Option<serde_json::Value> {
    if !configured {
        return None;
    }
    Some(serde_json::json!({
        "pending": stats.pending,
        "failed": stats.failed,
        "local_only": stats.local_only,
        "last_error": stats.last_error,
    }))
}

/// 某 URL 是否仍有 pending 队项。
///
/// B1-R3（复审 P2-3）：删除对账的保护集合必须**按当前账号**限定——异账号的
/// 暂停意图不保护本账号快照。故本查询强制带 `account_key`（pull_feeds 的批量
/// 保护查询用同一谓词）。
pub fn outbox_pending_for_url(
    conn: &Connection,
    account_key: &str,
    feed_url: &str,
) -> AppResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subscription_outbox
          WHERE feed_url = ?1 AND account_key = ?2 AND status = 'pending'",
        params![feed_url, account_key],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/* ============================================================
单元测试：合并/精确确认/代际重绑/local-only/stats
（cargo test 由 CI 执行，DEC-local-cargo-gate-20261005）
============================================================ */
#[cfg(test)]
mod t_opt009_tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut c).unwrap();
        c
    }

    fn ids(c: &Connection) -> Vec<i64> {
        pending_outbox(c)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    /// 合并纪律：同 URL 的新意图替换旧意图且**换新 id**；旧 id 的确认落空，
    /// 不会误删合并后的新目标（旧 HTTP 成功不得确认新意图）。
    /// 判别力：若实现改为「UPDATE 复用同一行」，旧 id 确认会删掉新 desired 目标，
    /// 下面 `prune_outbox(old_id)` 后队列清空的断言即红。
    /// B1-R3：移动链的移除基准是链起点（B1 场景里第一行由「无分类 → A」建起，
    /// 链起点为空——用新操作的 prev 反而会发出多余的 r=A）。
    #[test]
    fn merge_replaces_intent_without_id_reuse_old_confirm_misses_new() {
        let c = conn();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("旧标题"),
            Some("A"),
            Some("原分类"),
        )
        .unwrap();
        let old = ids(&c)[0];
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("新标题"),
            Some("B"),
            Some("A"),
        )
        .unwrap();
        let new = ids(&c)[0];
        assert_ne!(new, old, "合并必须换新 id（不复用）");
        prune_outbox(&c, &[old], 0).unwrap();
        let left = pending_outbox(&c).unwrap();
        assert_eq!(left.len(), 1, "旧 id 的确认不得删掉新意图");
        assert_eq!(left[0].desired_title.as_deref(), Some("新标题"));
        assert_eq!(left[0].desired_folder.as_deref(), Some("B"));
        assert_eq!(
            left[0].prev_folder.as_deref(),
            Some("原分类"),
            "移动需带链起点旧 label（远端实际持有的那个）"
        );
        // 新 id 确认才清队
        prune_outbox(&c, &[new], 0).unwrap();
        assert!(pending_outbox(&c).unwrap().is_empty());
    }

    /// 代际屏障：key 不匹配的行不重绑（换号暂停）；key 相同但代际落后 → 重绑
    /// 到当前代际继续可发；已 local_only 的终态不参与重绑。
    #[test]
    fn generation_rebind_only_for_same_account_key() {
        let c = conn();
        enqueue_subscribe(&c, 1, "acct-A", "https://a.example/rss", None, None).unwrap();
        enqueue_subscribe(&c, 1, "acct-B", "https://b.example/rss", None, None).unwrap();
        rebind_outbox_generation(&c, "acct-A", 5).unwrap();
        let items = pending_outbox(&c).unwrap();
        let a = items
            .iter()
            .find(|i| i.feed_url.contains("a.example"))
            .unwrap();
        let b = items
            .iter()
            .find(|i| i.feed_url.contains("b.example"))
            .unwrap();
        assert_eq!(a.account_generation, 5, "同账号（key 相同）重绑到当前代际");
        assert_eq!(b.account_generation, 1, "异账号队项暂停，代际不动");
    }

    /// 退订结转：quick_add 已建但本地未绑定的目标 id，删源入队退订时必须结转，
    /// 且 feed 级联不会清 outbox（本表无外键）。
    /// 判别力：不结转 remote_id 时入队返回 false、退订请求发不出去。
    #[test]
    fn unsubscribe_inherits_remote_id_from_pending_subscribe() {
        let c = conn();
        enqueue_subscribe(&c, 0, "k", "https://f.example/rss", None, Some("分类")).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 0, 42).unwrap();
        let queued = enqueue_unsubscribe(&c, 0, "k", "https://f.example/rss", None, None).unwrap();
        assert!(queued, "有结转目标必须入队");
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1, "同 URL 只留最新意图");
        assert_eq!(items[0].action, "unsubscribe");
        assert_eq!(items[0].remote_id, Some(42), "remote_id 必须结转");
    }

    /// R2(P2-1) 跨账号结转屏障：A 的 pending subscribe（remote_id=42）不得被
    /// B 的退订借用——结转只认同 account_key（key 不同即换号，换号不得把旧
    /// 账号的远端目标发给新账号）。
    /// 阶段 B 政策：B 的入队也不得**删除** A 的队项（异账号同 URL 保留/暂停，
    /// 否则 A 的远端已建意图成孤儿、回 A 后无从收敛）。
    /// 判别力：修前查询只按 feed_url，B 的调用会返回 true 并插入
    /// account_key='acct-B', remote_id=42 的 unsubscribe；且 DELETE 不分账号会
    /// 删掉 A 行。修后返回 false、B 无队项、A 行原样。对照面钉住「同账号仍结转」。
    #[test]
    fn unsubscribe_inherit_is_account_key_scoped() {
        let c = conn();
        enqueue_subscribe(&c, 1, "acct-A", "https://f.example/rss", None, None).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 1, 42).unwrap();

        let queued =
            enqueue_unsubscribe(&c, 1, "acct-B", "https://f.example/rss", None, None).unwrap();
        assert!(!queued, "异账号不得结转 A 的 remote_id");
        let left = pending_outbox(&c).unwrap();
        assert_eq!(left.len(), 1, "不得形成可向 B 发送的 unsubscribe 队项");
        assert_eq!(left[0].account_key, "acct-A", "A 的队项必须保留（暂停）");
        assert_eq!(left[0].remote_id, Some(42), "A 的远端目标不得被 B 触碰");

        // 对照面：同账号结转仍生效
        enqueue_subscribe(&c, 1, "acct-A", "https://f.example/rss", None, None).unwrap();
        advance_subscribe_stage(&c, ids(&c)[0], 1, 42).unwrap();
        let queued =
            enqueue_unsubscribe(&c, 1, "acct-A", "https://f.example/rss", None, None).unwrap();
        assert!(queued, "同账号必须继续结转");
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].account_key, "acct-A");
        assert_eq!(items[0].remote_id, Some(42));
    }

    /// R2(P3-3) 阶段推进只对 subscribe 行生效：edit 行即使 (id, generation,
    /// status) 全配也不得被旧 subscribe 响应写成 remote_created / 改写 remote_id。
    /// 判别力：无 action 守卫时本调用返回 true 且 remote_id 被改写为 99。
    #[test]
    fn advance_stage_only_touches_subscribe_action() {
        let c = conn();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("t"),
            None,
            None,
        )
        .unwrap();
        let id = ids(&c)[0];
        assert!(
            !advance_subscribe_stage(&c, id, 0, 99).unwrap(),
            "edit 行不得被 subscribe 阶段推进"
        );
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(
            item.remote_id,
            Some(10),
            "remote_id 不得被 subscribe 响应改写"
        );
        assert_eq!(item.stage, "pending");
    }

    /* ---------- B2-R2：退订解析推进与移除基准结转 ---------- */

    /// B2-R2（R2-2）：解析推进只对「仍未知远端目标的 pending 退订行」生效——
    /// 错 id / 换代 / 错 action / 已确认出队 / remote_id 已知的行都不得被旧解析
    /// 结果推进（响应在途/换号/重启不得串号）；成功时只写 `resolved_remote_id`，
    /// 不冒充「入队时已知的远端绑定」（恢复路径仍必须做列表发现）。
    /// 判别力：无守卫时错 id/换代也能写入解析结果。
    #[test]
    fn advance_unsubscribe_resolution_guards_identity_and_action() {
        let c = conn();
        enqueue_subscribe(&c, 0, "k", "https://f.example/rss", None, None).unwrap();
        let sub_id = ids(&c)[0];
        assert!(enqueue_unsubscribe(&c, 0, "k", "https://f.example/rss", None, None).unwrap());
        let unsub_id = ids(&c)[0];
        assert_ne!(sub_id, unsub_id, "替换行必须换新 id（旧确认天然落空）");

        // 错 id / 换代：落空
        assert!(!advance_unsubscribe_resolution(&c, unsub_id + 999, 0, 101).unwrap());
        assert!(!advance_unsubscribe_resolution(&c, unsub_id, 7, 101).unwrap());
        // 正确：只写解析结果
        assert!(advance_unsubscribe_resolution(&c, unsub_id, 0, 101).unwrap());
        let item = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.id == unsub_id)
            .unwrap();
        assert_eq!(item.resolved_remote_id, Some(101));
        assert_eq!(
            item.remote_id, None,
            "解析结果不得写 remote_id（发现阶段仍需进行）"
        );
        // 已确认出队（行不存在）→ 落空
        prune_outbox(&c, &[unsub_id], 0).unwrap();
        assert!(!advance_unsubscribe_resolution(&c, unsub_id, 0, 303).unwrap());
        // 错 action：subscribe 行不得被退订解析推进
        assert!(!advance_unsubscribe_resolution(&c, sub_id, 0, 303).unwrap());
        // 已知 remote_id 的退订行（走直接退订、不做解析）不得被解析推进
        enqueue_subscribe(&c, 0, "k", "https://g.example/rss", None, None).unwrap();
        let g_sub = ids(&c)[0];
        advance_subscribe_stage(&c, g_sub, 0, 42).unwrap();
        assert!(enqueue_unsubscribe(&c, 0, "k", "https://g.example/rss", None, None).unwrap());
        let g_unsub = ids(&c)[0];
        let g_row = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.id == g_unsub)
            .unwrap();
        assert_eq!(g_row.remote_id, Some(42), "前置：入队时已知远端目标");
        assert!(!advance_unsubscribe_resolution(&c, g_unsub, 0, 777).unwrap());
    }

    /// B2-R2（R2-3）：退订行携带「删除时快照的远端当前 label」移除基准；同 URL
    /// 快速重加替换该行时，后继 subscribe **继承**基准（挂载才能发 `r=` 移除远端
    /// 旧 label）；无基准的退订被替换时不凭空产生基准（对照面）。
    /// 判别力：不结转时重加行 prev 为 None——远端残留旧 label。
    #[test]
    fn subscribe_carries_removal_baseline_from_replaced_unsubscribe() {
        let c = conn();
        enqueue_subscribe(&c, 0, "k", "https://f.example/rss", None, None).unwrap();
        assert!(
            enqueue_unsubscribe(&c, 0, "k", "https://f.example/rss", None, Some("旧分类")).unwrap()
        );
        assert_eq!(
            pending_outbox(&c).unwrap()[0].prev_folder.as_deref(),
            Some("旧分类"),
            "退订行必须落下移除基准（快照）"
        );

        // 同 URL 快速重加：替换退订行 → 继承基准
        enqueue_subscribe(
            &c,
            0,
            "k",
            "https://f.example/rss",
            Some("标题"),
            Some("新分类"),
        )
        .unwrap();
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.action, "subscribe");
        assert_eq!(
            item.prev_folder.as_deref(),
            Some("旧分类"),
            "重加行必须继承被替换退订行的移除基准"
        );
        assert_eq!(item.remote_id, None, "不得继承远端 id（退订可能已生效）");

        // 对照面：无基准的退订被替换时不凭空产生基准
        enqueue_subscribe(&c, 0, "k", "https://u.example/rss", None, None).unwrap();
        assert!(enqueue_unsubscribe(&c, 0, "k", "https://u.example/rss", None, None).unwrap());
        enqueue_subscribe(&c, 0, "k", "https://u.example/rss", None, None).unwrap();
        let u = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("u.example"))
            .unwrap();
        assert_eq!(u.prev_folder, None, "无基准不得凭空产生");
    }

    /// B2-R2 复审 P3-A：删除侧移除基准必须取被替换 pending 行的**链起点**
    /// （`prev_folder`）——移动未发送时远端仍持链起点，本地 label 只是新目标；
    /// 没有链起点才退回调用方快照。判别力：按调用方快照 B 时远端 A 永不移除。
    #[test]
    fn enqueue_unsubscribe_prefers_replaced_row_chain_origin() {
        let c = conn();
        // 未发送的移动 A→B：链起点 A 才是远端当前 label
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            None,
            Some("B分类"),
            Some("A分类"),
        )
        .unwrap();
        assert!(
            enqueue_unsubscribe(&c, 0, "k", "https://f.example/rss", None, Some("B分类")).unwrap()
        );
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.action, "unsubscribe");
        assert_eq!(item.remote_id, Some(10), "remote_id 结转不变");
        assert_eq!(
            item.prev_folder.as_deref(),
            Some("A分类"),
            "链起点优先于调用方快照（远端尚未收到任何移动）"
        );

        // 对照面：被替换行没有链起点（如 pending subscribe）→ 用调用方快照
        enqueue_subscribe(&c, 0, "k", "https://u.example/rss", None, None).unwrap();
        assert!(
            enqueue_unsubscribe(&c, 0, "k", "https://u.example/rss", None, Some("本地分类"))
                .unwrap()
        );
        let u = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("u.example"))
            .unwrap();
        assert_eq!(
            u.prev_folder.as_deref(),
            Some("本地分类"),
            "无链起点时退回调用方快照"
        );
    }

    /// local-only 终态：标记后不再 pending、不再重试；stats 分开计数；
    /// prune 只清 pending（local_only 行保留展示）。
    /// 代际守卫：过期代际的标记不得落到已重绑到当前代际的行上（B1：执行器
    /// 能力门用 (id, generation) 精确标记，不能无误伤也不能漏）。
    #[test]
    fn local_only_is_terminal_and_counted_separately() {
        let c = conn();
        enqueue_subscribe(&c, 0, "fever", "https://f.example/rss", None, None).unwrap();
        let id = ids(&c)[0];
        // 过期代际标记落空（行仍是 pending）
        mark_outbox_local_only(&c, &[(id, 99)]).unwrap();
        assert_eq!(pending_outbox(&c).unwrap().len(), 1, "过期代际不得标记");
        // 当前代际标记生效
        mark_outbox_local_only(&c, &[(id, 0)]).unwrap();
        assert!(pending_outbox(&c).unwrap().is_empty(), "终态退出待推");
        assert_eq!(prune_outbox(&c, &[id], 0).unwrap(), 0, "prune 不得清终态行");
        let s = subscription_outbox_stats(&c, "fever").unwrap();
        assert_eq!((s.pending, s.failed, s.local_only), (0, 0, 1));
    }

    /// B1 合并纪律补充：同账号同 URL 的编辑按**字段**合并（改名后移动不丢改名），
    /// 且入队编辑会清掉同 URL 的 legacy add_feed 队项（避免 quick_add+edit 双推送）。
    /// 判别力：无字段合并时 desired_title 丢成 None；不清理 legacy 时 sync_queue
    /// 仍留 add_feed 行。
    #[test]
    fn edit_merges_previous_desired_fields_and_clears_legacy_add_feed() {
        let c = conn();
        enqueue_sync(&c, None, Some("https://f.example/rss"), "add_feed", None).unwrap();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            Some("新标题"),
            None,
            None,
        )
        .unwrap();
        let legacy: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE action = 'add_feed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "编辑必须清掉 legacy add_feed（防双推送）");

        // 第二次只移动分类：未给出的标题字段沿用第一次的（尚未推送）
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            None,
            Some("目标分类"),
            Some("原分类"),
        )
        .unwrap();
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1, "同账号同 URL 只留最新一行");
        assert_eq!(
            items[0].desired_title.as_deref(),
            Some("新标题"),
            "未给出的字段必须沿用旧行（合并最后目标）"
        );
        assert_eq!(items[0].desired_folder.as_deref(), Some("目标分类"));
        assert_eq!(items[0].prev_folder.as_deref(), Some("原分类"));
    }

    /// B1：remote_created 窗口的编辑合并原语——pending subscribe 行命中才更新、
    /// 字段按 COALESCE 合并（未给的不清空）；异账号/异动作不误伤。
    #[test]
    fn update_pending_subscribe_desired_merges_fields() {
        let c = conn();
        enqueue_subscribe(&c, 0, "k", "https://f.example/rss", None, Some("原分类")).unwrap();
        assert!(
            update_pending_subscribe_desired(
                &c,
                "k",
                "https://f.example/rss",
                Some("标题"),
                None,
                None
            )
            .unwrap(),
            "命中 pending subscribe 必须返回 true"
        );
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items[0].desired_title.as_deref(), Some("标题"));
        assert_eq!(
            items[0].desired_folder.as_deref(),
            Some("原分类"),
            "未给出的字段不得被清空"
        );
        assert!(
            !update_pending_subscribe_desired(
                &c,
                "other",
                "https://f.example/rss",
                Some("x"),
                None,
                None
            )
            .unwrap(),
            "异账号不得命中"
        );
    }

    /// B1：替代行 adopt——行被替换后，quick_add 返回的 remote_id 补给同账号同代际
    /// 的替代 pending 行（subscribe 照填并推进 remote_created；退订行只填 id），
    /// 已有 remote_id 的行不被覆盖。
    #[test]
    fn adopt_remote_id_fills_successor_and_never_overwrites() {
        let c = conn();
        // 场景一：替代行是新的 subscribe（删除后重新添加）→ 填 id + 推进阶段
        enqueue_subscribe(&c, 5, "k", "https://f.example/rss", None, None).unwrap();
        let first = ids(&c)[0];
        enqueue_subscribe(&c, 5, "k", "https://f.example/rss", None, Some("分类")).unwrap();
        let second = ids(&c)[0];
        assert_ne!(second, first, "替换必须换新 id");
        assert!(adopt_remote_id_for_pending(&c, "k", 5, "https://f.example/rss", 77).unwrap());
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items[0].remote_id, Some(77), "替代行必须拿到远端已建 id");
        assert_eq!(
            items[0].stage, "remote_created",
            "subscribe 替代行进入待绑定阶段"
        );
        // 已带 id 的行不被后续 adopt 覆盖
        assert!(!adopt_remote_id_for_pending(&c, "k", 5, "https://f.example/rss", 88).unwrap());
        assert_eq!(pending_outbox(&c).unwrap()[0].remote_id, Some(77));

        // 场景二：替代行是退订意图（本地删除；enqueue_unsubscribe 在无结转目标时
        // 不建行，这里直接构造「已存在的退订意图」）→ 只填 id，不写 remote_created
        c.execute(
            "INSERT INTO subscription_outbox (account_generation, account_key, action, feed_url)
             VALUES (5, 'k', 'unsubscribe', 'https://g.example/rss')",
            [],
        )
        .unwrap();
        assert!(adopt_remote_id_for_pending(&c, "k", 5, "https://g.example/rss", 88).unwrap());
        let u = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("g.example"))
            .unwrap();
        assert_eq!(u.remote_id, Some(88));
        assert_eq!(u.stage, "pending", "退订行不写 remote_created 阶段");
        // 异代际/异账号不命中
        assert!(!adopt_remote_id_for_pending(&c, "other", 5, "https://g.example/rss", 99).unwrap());
        assert!(!adopt_remote_id_for_pending(&c, "k", 9, "https://g.example/rss", 99).unwrap());
    }

    /// B1-R2（复审 P2）：subscribe 确认必须与「实际发送的目标」逐字段相等——
    /// 发送后原地更新（同 id/代际）时确认落空、队项保留；NULL 敏感比较；
    /// 代际守卫不放松。
    /// 判别力：只按 (id, generation) 确认的实现在「发送 A → 更新 B → 期望行保留」
    /// 断言处必红（会把从未发送的 B 意图静默删除）。
    #[test]
    fn subscribe_confirm_requires_sent_desired_to_be_current() {
        let c = conn();
        // 目标仍是已发送的 A：确认成功
        enqueue_subscribe(&c, 2, "k", "https://f.example/rss", None, Some("A")).unwrap();
        let id = ids(&c)[0];
        assert!(confirm_subscribe_if_desired_unchanged(&c, id, 2, None, Some("A"), None).unwrap());
        assert!(pending_outbox(&c).unwrap().is_empty());

        // 发送 A 后原地更新为 B：确认落空、行保留（最新目标待下轮发送）
        enqueue_subscribe(&c, 2, "k", "https://f.example/rss", None, Some("A")).unwrap();
        let id = ids(&c)[0];
        update_pending_subscribe_desired(
            &c,
            "k",
            "https://f.example/rss",
            Some("B标题"),
            Some("B"),
            None,
        )
        .unwrap();
        assert!(
            !confirm_subscribe_if_desired_unchanged(&c, id, 2, None, Some("A"), None).unwrap(),
            "发送后目标已变：旧确认不得出队（否则新意图静默丢失）"
        );
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].desired_folder.as_deref(), Some("B"));
        // 最新目标（与实际发送一致时）确认成功
        assert!(
            confirm_subscribe_if_desired_unchanged(&c, id, 2, Some("B标题"), Some("B"), None)
                .unwrap()
        );
        assert!(pending_outbox(&c).unwrap().is_empty());

        // NULL 敏感：无目标行只有期望 (None, None) 才确认；代际守卫不放松
        enqueue_subscribe(&c, 2, "k", "https://g.example/rss", None, None).unwrap();
        let id2 = ids(&c)[0];
        assert!(
            !confirm_subscribe_if_desired_unchanged(&c, id2, 2, None, Some("x"), None).unwrap()
        );
        assert!(!confirm_subscribe_if_desired_unchanged(&c, id2, 9, None, None, None).unwrap());
        assert!(confirm_subscribe_if_desired_unchanged(&c, id2, 2, None, None, None).unwrap());
    }

    /// B1-R3（复审 P2-1）：未发送的移动链折叠必须保留**链起点**（远端实际持有的
    /// 旧 label）；而既有行只是改名 / 既有行是 subscribe（remote_created 窗口）
    /// 时，远端持有的就是新操作读到的旧 label——用新传入的 prev。
    /// 判别力：无脑用「新操作的 prev」（修前形态）时 A→B→C 的 prev 变成 B，
    /// wire 只发 `a=C,r=B`，远端的 A 永不移除。
    #[test]
    fn move_chain_fold_keeps_chain_origin_of_unsent_moves() {
        let c = conn();
        // 场景一：两次移动都未发送 → 保留链起点 A
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            None,
            Some("B"),
            Some("A"),
        )
        .unwrap();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://f.example/rss",
            10,
            None,
            Some("C"),
            Some("B"),
        )
        .unwrap();
        let items = pending_outbox(&c).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].desired_folder.as_deref(), Some("C"));
        assert_eq!(
            items[0].prev_folder.as_deref(),
            Some("A"),
            "未发送链的移除基准必须是链起点（远端实际持有的 label）: {items:?}"
        );

        // 场景二：既有行只是改名（desired_folder 为空）→ 首次移动用新 prev
        enqueue_edit(
            &c,
            0,
            "k",
            "https://g.example/rss",
            10,
            Some("新标题"),
            None,
            None,
        )
        .unwrap();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://g.example/rss",
            10,
            None,
            Some("D"),
            Some("C"),
        )
        .unwrap();
        let g = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("g.example"))
            .unwrap();
        assert_eq!(g.desired_folder.as_deref(), Some("D"));
        assert_eq!(
            g.prev_folder.as_deref(),
            Some("C"),
            "既有行没有未发送移动目标时，远端持有的就是新 prev"
        );

        // 场景三：既有行是 subscribe（remote_created 窗口，prev 恒为 NULL）→ 用新 prev
        c.execute(
            "INSERT INTO subscription_outbox
                (account_generation, account_key, action, feed_url, desired_folder, stage)
             VALUES (0, 'k', 'subscribe', 'https://h.example/rss', 'X', 'remote_created')",
            [],
        )
        .unwrap();
        enqueue_edit(
            &c,
            0,
            "k",
            "https://h.example/rss",
            10,
            None,
            Some("Y"),
            Some("X"),
        )
        .unwrap();
        let h = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("h.example"))
            .unwrap();
        assert_eq!(h.action, "edit");
        assert_eq!(h.desired_folder.as_deref(), Some("Y"));
        assert_eq!(
            h.prev_folder.as_deref(),
            Some("X"),
            "subscribe 行不携带远端已发送的移动目标：用新 prev"
        );
    }

    /// B1-R3（复审 P2-1 配套）+ B2-1（代际切换结转）：发送成功后确认落空 →
    /// 替代行的移除基准重定为已发送目标。守卫：**同账号 key**/同 URL、
    /// action='edit'、仍 pending、仍带移动目标。
    /// 判别力：不限定账号/动作的实现在「异账号不命中」断言处必红；
    /// 把作用域收成 (key, generation) 的实现在「同 key 换代后仍重定」处必红
    /// （B2-1：换代后确认阶段被跳过，只有 key 作用域的重定能修好后继意图）。
    #[test]
    fn rebase_successor_rewrites_only_scoped_pending_move_rows() {
        let c = conn();
        enqueue_edit(
            &c,
            3,
            "k",
            "https://f.example/rss",
            10,
            None,
            Some("C"),
            Some("A"),
        )
        .unwrap();
        // 同 key、代际已推进（3 → 9）：仍必须重定（远端已接受该目标是账号作用域事实）
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://f.example/rss", "B").unwrap(),
            1
        );
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.desired_folder.as_deref(), Some("C"));
        assert_eq!(
            item.prev_folder.as_deref(),
            Some("B"),
            "发送成功 = 远端当前 label：替代行必须改用它移除"
        );
        // 幂等：已是该值不再改写
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://f.example/rss", "B").unwrap(),
            0
        );
        // 守卫：异账号不命中（跨账号队列一个字节都不动）
        assert_eq!(
            rebase_successor_edit_prev(&c, "other", "https://f.example/rss", "D").unwrap(),
            0
        );
        assert_eq!(
            pending_outbox(&c).unwrap()[0].prev_folder.as_deref(),
            Some("B")
        );

        // 异账号同 URL 的行同样不命中
        enqueue_edit(
            &c,
            3,
            "other",
            "https://f.example/rss",
            10,
            None,
            Some("C2"),
            Some("A2"),
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://f.example/rss", "D").unwrap(),
            1,
            "只重定本账号行"
        );
        let other = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.account_key == "other")
            .unwrap();
        assert_eq!(
            other.prev_folder.as_deref(),
            Some("A2"),
            "异账号行不得被本账号的发送结果改写"
        );

        // 不触碰：无移动目标的 rename-only 行
        enqueue_edit(
            &c,
            3,
            "k",
            "https://g.example/rss",
            10,
            Some("标题"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://g.example/rss", "D").unwrap(),
            0
        );
        // B2-R1(F-1)：subscribe 后继（删除后重加）在 prev 为空时**补填**已接受目标
        c.execute(
            "INSERT INTO subscription_outbox
                (account_generation, account_key, action, feed_url, desired_folder)
             VALUES (3, 'k', 'subscribe', 'https://h.example/rss', 'X')",
            [],
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://h.example/rss", "D").unwrap(),
            1
        );
        let h = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("h.example"))
            .unwrap();
        assert_eq!(
            h.prev_folder.as_deref(),
            Some("D"),
            "重加行必须补上已接受目标的移除基准"
        );
        // B2-R2 复审（P3-A/D）：已有 prev 的 subscribe 行同样改写到已接受目标——
        // 「2xx = 远端当前持有 D」是账号作用域事实，改写前的值是发送前/链起点快照
        // （删除时从被替换移动行继承的链起点），按它发 r= 会把远端 D 永久留下。
        c.execute(
            "INSERT INTO subscription_outbox
                (account_generation, account_key, action, feed_url, desired_folder, prev_folder)
             VALUES (3, 'k', 'subscribe', 'https://i.example/rss', 'X', '已有起点')",
            [],
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://i.example/rss", "D").unwrap(),
            1
        );
        let i_row = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("i.example"))
            .unwrap();
        assert_eq!(i_row.prev_folder.as_deref(), Some("D"));
        // 幂等：已是该值不再改写
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://i.example/rss", "D").unwrap(),
            0
        );
        // 删除中的退订行（pending unsubscribe）的移除基准同样改写：删除撞上在途
        // 移动、随后才重加时，后继继承的必须是已接受目标而不是链起点。
        c.execute(
            "INSERT INTO subscription_outbox
                (account_generation, account_key, action, feed_url, prev_folder)
             VALUES (3, 'k', 'unsubscribe', 'https://m.example/rss', 'A')",
            [],
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://m.example/rss", "D").unwrap(),
            1
        );
        let m_row = pending_outbox(&c)
            .unwrap()
            .into_iter()
            .find(|i| i.feed_url.contains("m.example"))
            .unwrap();
        assert_eq!(m_row.prev_folder.as_deref(), Some("D"));
        // 无移动目标的 subscribe 行同样不触碰
        c.execute(
            "INSERT INTO subscription_outbox
                (account_generation, account_key, action, feed_url)
             VALUES (3, 'k', 'subscribe', 'https://j.example/rss')",
            [],
        )
        .unwrap();
        assert_eq!(
            rebase_successor_edit_prev(&c, "k", "https://j.example/rss", "D").unwrap(),
            0
        );
        // 已不在队的 id 判定（执行器据此决定是否重定）
        let id = pending_outbox(&c).unwrap()[0].id;
        assert!(outbox_pending_by_id(&c, id).unwrap());
        prune_outbox(&c, &[id], 3).unwrap();
        assert!(!outbox_pending_by_id(&c, id).unwrap());
    }

    /// B2-3/B2-5：remote_created 窗口的合并原语带上（a）标题目标与（b）移动的旧
    /// label——分类改名/移动对尚未绑定的成员也表达 `t=`/`a=`/`r=`；
    /// `prev_folder` 取**首值**（链起点才是远端实际持有的 label，后续再改目标不得
    /// 改写移除基准）；无目标分类的调用不得凭空写入 prev；确认必须比对三字段。
    #[test]
    fn subscribe_desired_merge_carries_title_and_first_prev() {
        let c = conn();
        enqueue_subscribe(
            &c,
            0,
            "k",
            "https://f.example/rss",
            Some("标题A"),
            Some("A"),
        )
        .unwrap();
        let id = ids(&c)[0];
        // 移动 A→B：带上旧 label
        assert!(update_pending_subscribe_desired(
            &c,
            "k",
            "https://f.example/rss",
            None,
            Some("B"),
            Some("A")
        )
        .unwrap());
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.desired_title.as_deref(), Some("标题A"));
        assert_eq!(item.desired_folder.as_deref(), Some("B"));
        assert_eq!(
            item.prev_folder.as_deref(),
            Some("A"),
            "首次移动写入旧 label"
        );
        // 再移动 B→C：prev 保持链起点 A（不是 B）
        assert!(update_pending_subscribe_desired(
            &c,
            "k",
            "https://f.example/rss",
            Some("标题C"),
            Some("C"),
            Some("B")
        )
        .unwrap());
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.desired_title.as_deref(), Some("标题C"));
        assert_eq!(
            item.prev_folder.as_deref(),
            Some("A"),
            "prev 取首值（链起点）"
        );
        // 无目标分类的调用不得凭空写入 prev
        assert!(update_pending_subscribe_desired(
            &c,
            "k",
            "https://f.example/rss",
            None,
            None,
            Some("X")
        )
        .unwrap());
        assert_eq!(
            pending_outbox(&c).unwrap()[0].prev_folder.as_deref(),
            Some("A"),
            "无移动的调用不得写入移除基准"
        );
        // 确认逐字段（标题/分类/旧 label NULL 敏感）
        assert!(
            !confirm_subscribe_if_desired_unchanged(&c, id, 0, Some("标题C"), Some("C"), None)
                .unwrap()
        );
        assert!(
            !confirm_subscribe_if_desired_unchanged(&c, id, 0, None, Some("C"), Some("A")).unwrap()
        );
        assert!(confirm_subscribe_if_desired_unchanged(
            &c,
            id,
            0,
            Some("标题C"),
            Some("C"),
            Some("A")
        )
        .unwrap());
        assert!(pending_outbox(&c).unwrap().is_empty());
    }

    /// stats 口径 + 失败标记：attempts 累加、last_error 截断口径与 sync_queue 一致。
    /// R2(P3-2)：入参带代际，过期代际的失败标记不得落到当前行（判别力见下）。
    #[test]
    fn stats_and_failed_marker() {
        let c = conn();
        enqueue_subscribe(&c, 3, "k", "https://f.example/rss", None, None).unwrap();
        let id = ids(&c)[0];
        mark_outbox_failed(&c, &[(id, 3, "订阅推送失败: timeout".into())]).unwrap();
        mark_outbox_failed(&c, &[(id, 3, "订阅推送失败: HTTP 500".into())]).unwrap();
        // 代际守卫：过期计划的失败不得记到当前代际行（误记会让 attempts/last_error 撒谎）
        mark_outbox_failed(&c, &[(id, 99, "过期计划失败".into())]).unwrap();
        let s = subscription_outbox_stats(&c, "k").unwrap();
        assert_eq!((s.pending, s.failed, s.local_only), (1, 1, 0));
        assert_eq!(s.last_error.as_deref(), Some("订阅推送失败: HTTP 500"));
        let item = &pending_outbox(&c).unwrap()[0];
        assert_eq!(item.attempts, 2);
        // 精确确认带代际：代际不符不得删行
        assert_eq!(prune_outbox(&c, &[id], 99).unwrap(), 0);
        assert_eq!(prune_outbox(&c, &[id], 3).unwrap(), 1);
    }

    /// B3：统计必须**按当前账号限定**——异账号（换号暂停）的队项不得混入当前
    /// 账号的 pending/failed/local_only 与最近错误。
    /// 判别力：旧实现（全库口径）下 A 的暂停项会出现在 B 的 `local_only`
    /// 与 pending 计数里，本用例即红。
    #[test]
    fn stats_are_account_scoped() {
        let c = conn();
        // A 的暂停项：local_only 终态（换号后不得混入任何别的账号）
        enqueue_subscribe(&c, 1, "acct-A", "https://a.example/rss", None, None).unwrap();
        let id_a = ids(&c)[0];
        mark_outbox_local_only(&c, &[(id_a, 1)]).unwrap();
        // B 的队项：一条失败待推
        enqueue_subscribe(&c, 7, "acct-B", "https://b.example/rss", None, None).unwrap();
        let id_b = ids(&c)[0];
        mark_outbox_failed(&c, &[(id_b, 7, "B 的失败摘要".into())]).unwrap();

        let b = subscription_outbox_stats(&c, "acct-B").unwrap();
        assert_eq!(
            (b.pending, b.failed, b.local_only),
            (1, 1, 0),
            "B 的统计只含 B 的行（A 的 local_only 不得混入）"
        );
        assert_eq!(b.last_error.as_deref(), Some("B 的失败摘要"));

        let a = subscription_outbox_stats(&c, "acct-A").unwrap();
        assert_eq!(
            (a.pending, a.failed, a.local_only),
            (0, 0, 1),
            "A 只看到自己的 local_only 终态（B 的待推不得混入）"
        );
        assert_eq!(a.last_error, None, "A 无失败：不得借用 B 的最近错误");

        let none = subscription_outbox_stats(&c, "acct-none").unwrap();
        assert_eq!((none.pending, none.failed, none.local_only), (0, 0, 0));
        assert_eq!(none.last_error, None);
    }

    /// B3：统计读取失败必须 Err——不得静默返回 0 冒充「无待办」。
    #[test]
    fn stats_read_failure_is_error_not_zero() {
        let c = conn();
        c.execute("DROP TABLE subscription_outbox", []).unwrap();
        assert!(
            subscription_outbox_stats(&c, "acct-A").is_err(),
            "读取失败必须返回 Err（返回 0 会冒充「无待办」）"
        );
    }

    /// B3：事件 payload 单点——configured=false（未配置同步）不发事件；
    /// configured=true 时四字段与 stats / 前端 SubscriptionOutboxStats 三方同形。
    #[test]
    fn changed_payload_shape_and_silent_when_unconfigured() {
        let stats = SubscriptionOutboxStats {
            pending: 2,
            failed: 1,
            local_only: 3,
            last_error: Some("订阅推送失败: HTTP 500".into()),
        };
        assert!(
            subscription_outbox_changed_payload(&stats, false).is_none(),
            "未配置同步不发事件（不制造噪音）"
        );
        let p = subscription_outbox_changed_payload(&stats, true).unwrap();
        assert_eq!(p["pending"], 2);
        assert_eq!(p["failed"], 1);
        assert_eq!(p["local_only"], 3);
        assert_eq!(p["last_error"], "订阅推送失败: HTTP 500");
        let empty = SubscriptionOutboxStats::default();
        let p0 = subscription_outbox_changed_payload(&empty, true).unwrap();
        assert_eq!(
            (p0["pending"].as_i64(), p0["failed"].as_i64()),
            (Some(0), Some(0))
        );
        assert!(
            p0["last_error"].is_null(),
            "无错误摘要为 null（不伪造空串）"
        );
    }
}

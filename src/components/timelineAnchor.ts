// Note: 返回位置全场景规则表（无法可靠恢复时明确归零，不猜） — 见 .agents/notes/implemented/feature/2026-10-07-返回位置与列表保位.md
/* ============================================================
   返回位置统一规则（TASK-111② 后台刷新保位 + TASK-115 切换返回恢复 /
   阅读器焦点归还）——本模块是全部「回位/归还」机制的单一记账点与规则文档点。

   TASK-123（审计 P2-5①③）：保位窗口扩展——本模块的三处扩展（均**扩展**
   TASK-111/115 机制，不推翻其分流与记账规则）：
   ① 锚载荷加 intra-item 像素偏移（offsetPx = 滚动容器 scrollTop − 顶条卡片
   虚拟起点）：审计「锚只存 id 无卡片内像素偏移；长社交正文也只能恢复卡片
   顶部」。记录/提交/重锚全链路携带；两个恢复消费点（TASK-111 刷新回位 /
   TASK-115 切换返回）在 scrollToIndex(align:'start') 之后补加偏移 = 精确还原
   视口（动态测量下卡片起点可能微移，属既有估算行高误差量级，如实接受）。
   ② 节流尾沿补记（commitTopAnchor）：审计「250ms 节流无尾沿补记，停滚后立即
   切换还可能记到较早的位置」。节流记录收敛高频事件不变；Timeline 在滚动静默
   ANCHOR_RECORD_THROTTLE_MS 后调用 commitTopAnchor 一次，把**最终停留位置**
   （含偏移）无条件落锚——离开上下文时的存档（stashTopAnchorForReturn）因此
   是停滚时的真实位置，而非最后一次节流窗口放行的较早位置。commit 与 record
   共用节流基准（commit 后窗口内 record 照常被吸收）。
   ③（bootstrap/nav 侧，见彼处注释）后台刷新改「保位窗口重取」：keepReadingPosition
   不再只拉首屏——按当前游标 loaded 分页重取整个已加载窗口，锚 id 在刷新后仍
   在列表中（除非真被删除）。这让「导航缓存命中恢复 → 后台重拉」的组合不再
   二次丢锚（审计 P2-5②）：重拉保窗口，恢复定位过的锚自然幸存；重拉落地时
   positionRestoreNonce 照常 bump，回位 effect 按活锚（恢复时已 rearm 到恢复
   落点）校正头部插入造成的索引漂移。**分流规则不变**：切换返回的恢复动作
   本身仍只走 switchRestoreNonce（一次性定位），positionRestoreNonce 只由
   reload 落地发出——发出方从「内容刷新三入口」扩展为「内容刷新三入口 +
   导航后台刷新」（导航**切换动作**依旧不 bump），「持续跟踪 vs 一次性定位」
   的通道分离原样保留。

   ┌─ 全场景规则表（契约 UI-CONTRACT-TASK-115-RETURN-POSITION.md 的代码载体；
   │  契约文档本身禁碰，此处为 X3 的单点注释） ────────────────────────────
   │ 场景                              │ 行为               │ 机制
   │ 后台刷新（feeds-updated/手动同步/ │ 原位保持           │ TASK-111 顶条锚 +
   │ 单源刷新；TASK-123 起导航后台刷新│                    │ positionRestoreNonce
   │ 亦然——窗口重取+落地回位）        │                    │ + 窗口重取
   │ 切布局/视图/范围后切回（缓存命中）│ 恢复到离开时顶条   │ TASK-115 per-filterKey
   │                                  │ （含卡片内偏移）   │ 锚存档 + switchRestoreNonce
   │ 切排序（重拉）                   │ 不恢复（新语境）   │ 裁定：filterKey 变化即
   │                                  │                    │ 新语境，存档随键自然失效
   │ 视图 cache-miss 导航（新语境）   │ 首屏起步不保窗口   │ TASK-123 裁定：无档可恢复，
   │                                  │                    │ 窗口深度属于旧语境
   │ 首次进入某 filterKey             │ 顶部（现状）       │ 无存档 → 归零回落
   │ 存档锚 id 不在恢复列表中         │ 归零回落（不猜）   │ anchorRestoreIndex → null
   │ 阅读器关闭（Esc）                │ 滚动不动 + 焦点归  │ TASK-115 关闭信号
   │                                  │ 还原选中卡         │ readerCloseNonce
   │ 阅读器关闭但列表已切换           │ 焦点归还回落       │ 原卡不在 items → 不聚焦
   │ 画廊布局（image，非虚拟化）      │ 全部回落现状       │ 与 TASK-111 同口径
   └──────────────────────────────────────────────────────────────────────

   与 TASK-111 刷新保位的协同（不叠加保证）：
   - 刷新保位是「持续跟踪」：锚随滚动持续节流更新，后台刷新落地（nonce 信号）
     时按**当前**锚回位；切换返回是「一次性定位」：只在导航缓存命中恢复 entries
     时按**存档**锚定位一次。两者的触发源（positionRestoreNonce vs
     switchRestoreNonce）、消费 effect、锚来源（活锚 vs 存档）完全分离；
   - 导航**切换动作**不 bump positionRestoreNonce（恢复走 switchRestoreNonce）；
     导航随后发起的**后台刷新**自 TASK-123 起携带 keepReadingPosition（窗口
     重取 + 落地回位）——它是内容刷新（与 feeds-updated 同类），不是切换动作
     本身；「切换动作绝不借用刷新通道」的隔离语义原样保留（回归网更新为
     同步段隔离 + 刷新落地段按刷新通道回位两段断言）。

   TASK-111②：后台刷新保位——顶条锚（top anchor）。

   审计：「后台刷新保留当前阅读位置，避免靠整体替换列表刷新所有内容」。
   现况：feeds-updated → reloadFromBackend 整体替换 entries；newest_first 下
   新文章插头部使既有条目索引后移，虚拟列表 scrollTop 不变但内容整体错位，
   视觉跳动。修法：Timeline 滚动时以节流方式记录「可见首条目 id」（顶条锚），
   后台刷新落地后若锚 id 仍在新快照中，程序性滚动到其新索引（复用
   Timeline 既有 suppressNextScrollEvents 机制）；锚丢失回落现状（不强制顶部）。

   设计取舍（与任务卡决议一致）：
   - 锚是模块级状态而非 store 状态：滚动是高频路径，节流后的记录也不该触发
     任何组件重渲染；消费侧（Timeline）只订阅保位信号 nonce（store 字段
     positionRestoreNonce），锚本身零订阅。
   - 分流显式：锚随记录时的 filterKey（布局×视图×范围×筛选×排序）一起记账，
     消费时 filterKey 不一致一律回落——用户主动切范围/布局/视图/排序后，锚
     属于上一个阅读上下文，绝不回位；保位信号（reload 的 keepReadingPosition
     选项）由内容刷新路径发出（TASK-123 起导航后台刷新同属内容刷新，见上），
     双重隔离。
   - 程序性滚动（J/K 定位 / 搜索锚定 / 归零）同样会经过 onScroll：记录它们
     的落点是正确语义（那也是用户此刻的阅读位置），无需区分。

   TASK-115：切换返回恢复——per-filterKey 锚存档（X1）。

   审计：「切布局/视图/范围均 filterKey 归零丢位置；缓存恢复只恢复 entries
   不恢复滚动」。修法：filterKey 变化（= 离开当前上下文）时把活锚按其自身
   filterKey 存档（stashTopAnchorForReturn）；导航三路径（selectLayout /
   selectView / selectFeed）缓存命中恢复 entries 时 bump switchRestoreNonce，
   Timeline 消费存档：存档锚 id 在恢复列表中 → scrollToIndex(align:'start')
   程序性定位（一次性，独立于刷新保位）；无存档 / 锚丢失 → 归零回落。
   - 存档按 filterKey 键覆盖（同一上下文反复离开只保留最后一次离开位置），
     容量 LRU 上限 RETURN_ANCHOR_ARCHIVE_MAX（复用 TASK-111 预算纪律：
     8 键与视图缓存 VIEW_ENTRIES_CACHE_MAX 同值，覆盖真实来回切换深度，
     淘汰只影响该上下文退回「归零回落」，无正确性影响）；
   - 切排序（重拉）：filterKey 已变 = 新语境，进入时查的是新键的存档（空）
     → 自然归零，裁定「不恢复」无需特判（存档随 filterKey 键自然失效）；
     toggleTimelineSort 同时清空视图缓存，切回是 cache-miss 重拉路径，
     恢复本就不触发（双保险）；
   - 恢复成功后 rearmTopAnchor 把活锚重锚到恢复落点：否则节流窗口会让
     紧随 scrollToIndex 到达的 scroll 事件把活锚记回恢复前位置，用户随即
     再离开时存档到的是过期位置（第二次切回退回旧处）。

   TASK-115：阅读器焦点归还（X2）——Timeline 消费关闭信号。

   审计：「阅读器关闭 clearReaderSelection 只清字段，无焦点归还」。修法：
   clearReaderSelection bump readerCloseNonce（不透明计数器，store 字段），
   Timeline 消费关闭信号：原卡仍在当前列表 → 按 moveCardFocus 同一语义归位
   （align:'auto'——原卡可见则不滚、不可见则 scrollToIndex 定位后聚焦）；列表
   已切换（原卡不在 items）→ 归零回落不聚焦。关闭前的原选中卡 id 由 Timeline
   在 activeArticleId 跟随 effect 里捕获（组件内 ref）——关闭时刻该 ref 必然
   持有阅读器正在显示的文章 id。选型理由（Timeline 消费关闭信号 vs store
   action 关闭前记录原 id）：store 不反向依赖 components 层（本模块在
   components/ 下，store→components 是倒挂方向，会开循环依赖的口子）；且
   「只有原 id 字段无法区分连续两次开关同一篇」（值不变不触发 effect），
   计数器才是可靠的重触发信号。焦点归还决策收口在纯函数
   readerFocusReturnIndex（回归网直接断言）。
   ============================================================ */

/** 顶条锚：可见首条目 id + 记录时的筛选上下文签名 + 记录时刻。
    TASK-123：offsetPx = 卡片内像素偏移（scrollTop − 顶条卡片虚拟起点），
    恢复时 scrollToIndex(align:'start') 后补加，精确还原视口（含长卡片
    中部的停留位置——审计 P2-5③：锚只存 id 只能恢复卡片顶）。 */
export interface TopAnchor {
  id: string;
  /** 记录时的 filterKey（布局|视图|范围|显示筛选|排序）——消费时必须逐字一致 */
  filterKey: string;
  /** TASK-123：卡片内像素偏移（≥0；0 = 卡片顶对齐视口顶） */
  offsetPx: number;
  /** 记录时刻（performance.now()），诊断用；节流判定不依赖它 */
  at: number;
}

/** 节流窗口：连续 scroll 事件高频到达，记录收敛到约 4 次/秒足够
    （锚只需落在「刷新落地前用户最后停留的位置」量级）。
    TASK-123：尾沿补记（commitTopAnchor）让「最后停留位置」不再依赖节流
    相位——见头注 ②。 */
export const ANCHOR_RECORD_THROTTLE_MS = 250;

let anchor: TopAnchor | null = null;
let lastRecordAt = 0;

/** 记录顶条锚（节流）。窗口内重复调用被忽略并返回 false——调用方（回归网）
    可直接断言节流生效。offsetPx 缺省 0（无偏移 = 卡片顶对齐，TASK-123 前的
    语义；测试与无 DOM 上下文可省略）。 */
export function recordTopAnchor(id: string, filterKey: string, now: number, offsetPx = 0): boolean {
  if (now - lastRecordAt < ANCHOR_RECORD_THROTTLE_MS) return false;
  lastRecordAt = now;
  anchor = { id, filterKey, offsetPx, at: now };
  return true;
}

/** TASK-123：尾沿补记（无条件写入，绕过节流）。滚动静默后由 Timeline 的
    尾沿定时器调用一次，把最终停留位置（含偏移）落锚；推进节流基准——
    commit 后窗口内到达的 record 照常被吸收（与 rearm 同语义）。
    与 record 的分工：record = 高频节流采样（lead 锚），commit = 停滚定稿
    （尾沿锚）；两者写同一份活锚，消费侧无感知差异。 */
export function commitTopAnchor(id: string, filterKey: string, now: number, offsetPx: number): void {
  lastRecordAt = now;
  anchor = { id, filterKey, offsetPx, at: now };
}

/** 丢弃锚：筛选上下文切换（filterKey 变化）时调用——上一个上下文的锚对新
    上下文既不可用（消费侧也会被 filterKey 比对拦下）也不该滞留。 */
export function clearTopAnchor(): void {
  anchor = null;
}

/** 读取当前锚（消费侧与回归网用；不改变锚状态） */
export function peekTopAnchor(): TopAnchor | null {
  return anchor;
}

/** 保位回位决策（纯函数，回归网直接断言）：
    返回锚 id 在新快照中的索引；以下情形返回 null = 回落现状（不滚动、不强制顶部）：
    - 无锚（用户从未滚动——保持「顶部看新文章」的既有行为）；
    - 锚的 filterKey 与当前上下文不一致（用户已主动切换——不回位）；
    - 锚 id 不在新快照（文章被删/被筛掉/被分页截出——无处可回）。 */
export function anchorRestoreIndex(
  a: TopAnchor | null,
  filterKey: string,
  items: ReadonlyArray<{ id: string }>,
): number | null {
  if (!a) return null;
  if (a.filterKey !== filterKey) return null;
  const idx = items.findIndex((it) => it.id === a.id);
  return idx >= 0 ? idx : null;
}

/* ============================================================
   TASK-115：切换返回恢复——per-filterKey 锚存档（X1）
   ============================================================ */

/** 存档容量上限（TASK-111 预算纪律：与视图缓存 VIEW_ENTRIES_CACHE_MAX 同值）。
    LRU 淘汰只影响被淘汰上下文退回「归零回落」（首次进入同口径），无正确性影响。 */
export const RETURN_ANCHOR_ARCHIVE_MAX = 8;

/** 存档表：filterKey → 离开该上下文时的顶条锚。模块级（非 store 状态）：
    存档/消费都不该触发重渲染，消费侧只订阅恢复信号 nonce（store 字段
    switchRestoreNonce）——与活锚「载荷留模块、信号入 store」同一分工。 */
const returnAnchorArchive = new Map<string, TopAnchor>();

/** LRU 语义写入：先摘除再插入保证新鲜度，超限删最旧（Map 迭代序 = 插入序）。 */
function archiveSet(key: string, value: TopAnchor): void {
  returnAnchorArchive.delete(key);
  returnAnchorArchive.set(key, value);
  while (returnAnchorArchive.size > RETURN_ANCHOR_ARCHIVE_MAX) {
    const oldest = returnAnchorArchive.keys().next().value;
    if (oldest === undefined) break;
    returnAnchorArchive.delete(oldest);
  }
}

/** 离开上下文时存档活锚（filterKey 变化的 layout effect 调用，紧随其后清活锚）。
    以锚**自身携带的 filterKey**（= 被离开的上下文）为键——用户随后切回该上下文
    时按当前 filterKey 查档命中。无活锚（用户从未滚动）不存档：切回即顶部（现状）。
    注意：只存档不清活锚——清活锚仍由调用点的既有 clearTopAnchor() 负责（时序：
    先存档再清，活锚不丢失）。 */
export function stashTopAnchorForReturn(): void {
  if (anchor) archiveSet(anchor.filterKey, anchor);
}

/** 查档（切换返回恢复的 Timeline 消费侧）：按**当前** filterKey 取离开时的锚。
    命中即刷新 LRU 新鲜度；无档返回 null（消费侧据此归零回落）。 */
export function peekReturnAnchor(filterKey: string): TopAnchor | null {
  const v = returnAnchorArchive.get(filterKey);
  if (v === undefined) return null;
  returnAnchorArchive.delete(filterKey);
  returnAnchorArchive.set(filterKey, v);
  return v;
}

/** 活锚重锚（绕过节流）：程序性恢复定位后调用。scrollToIndex 到达的 scroll
    事件本会经 recordTopAnchor 记录落点，但节流窗口内会被忽略——若恢复定位前
    恰好刚记录过（250ms 内），活锚将滞留在恢复前的位置，用户随即再离开时存档
    到过期锚。恢复是「一次性定位」且落点确定（锚 id 对齐视口顶 + TASK-123
    偏移补加），直接重锚并推进节流基准，保证后续存档/刷新保位从正确落点起算。
    TASK-123：offsetPx 缺省 0——恢复消费点在补加偏移后以同一偏移重锚。 */
export function rearmTopAnchor(id: string, filterKey: string, now: number, offsetPx = 0): void {
  anchor = { id, filterKey, offsetPx, at: now };
  lastRecordAt = now;
}

/* ============================================================
   TASK-115：阅读器焦点归还——纯决策（X2）
   ============================================================ */

/** 焦点归还决策（纯函数，回归网直接断言）：原卡在当前列表中的索引；
    以下情形返回 null = 归零回落（不聚焦、不滚动）：
    - 无档案（非阅读器关闭路径 / 已被消费）；
    - 原卡不在当前 items（列表已切换——焦点归还无对象，绝不猜）。
    原选中卡 id 由 Timeline 捕获（关闭信号消费侧，见头注 X2 选型理由），
    本函数只做「该不该归还 / 归还到哪」的判定。 */
export function readerFocusReturnIndex(
  returnId: string | null,
  items: ReadonlyArray<{ id: string }>,
): number | null {
  if (!returnId) return null;
  const idx = items.findIndex((it) => it.id === returnId);
  return idx >= 0 ? idx : null;
}

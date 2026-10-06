/* ============================================================
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
     选项）也只由内容刷新路径（feeds-updated / 手动同步 / 单源刷新）发出，
     双重隔离。
   - 程序性滚动（J/K 定位 / 搜索锚定 / 归零）同样会经过 onScroll：记录它们
     的落点是正确语义（那也是用户此刻的阅读位置），无需区分。
   ============================================================ */

/** 顶条锚：可见首条目 id + 记录时的筛选上下文签名 + 记录时刻 */
export interface TopAnchor {
  id: string;
  /** 记录时的 filterKey（布局|视图|范围|显示筛选|排序）——消费时必须逐字一致 */
  filterKey: string;
  /** 记录时刻（performance.now()），诊断用；节流判定不依赖它 */
  at: number;
}

/** 节流窗口：连续 scroll 事件高频到达，记录收敛到约 4 次/秒足够
    （锚只需落在「刷新落地前用户最后停留的位置」量级）。 */
export const ANCHOR_RECORD_THROTTLE_MS = 250;

let anchor: TopAnchor | null = null;
let lastRecordAt = 0;

/** 记录顶条锚（节流）。窗口内重复调用被忽略并返回 false——调用方（回归网）
    可直接断言节流生效。 */
export function recordTopAnchor(id: string, filterKey: string, now: number): boolean {
  if (now - lastRecordAt < ANCHOR_RECORD_THROTTLE_MS) return false;
  lastRecordAt = now;
  anchor = { id, filterKey, at: now };
  return true;
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

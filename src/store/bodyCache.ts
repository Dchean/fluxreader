// Note: 正文/AI 产物与列表快照解耦；清理缓存后旧值不得复活 — 见 .agents/notes/implemented/architecture/2026-10-06-正文与-AI-产物的实体缓存.md
/* ============================================================
   TASK-122（审计 P2-3）：bodyById —— 正文/AI 实体缓存（模块级，独立于视图行）

   本模块是「文章正文 / AI 产物 / 水合状态」的**唯一真值源**，取代原先分散在
   ArticleEntry.content / rawContent / hydrated、store.hydratedIds、
   store.hydrationErrors 五处的正文侧状态（审计「显式水合状态机：状态仍分布在
   content、hydrated、hydratedIds、hydrationErrors、模块级 inFlight，且……不是
   单一判别联合状态机」——本模块即该问题的收口）。

   ── 状态机（BodyState，单点文档；无字符串态共用）──────────────────
   每篇文章 id 至多一条记录；无记录 = 「未请求」（含 LRU 淘汰后的回退态）。

     (无记录) ──入队水合──▶ loading ──响应命中行──▶ ready
                                │ 缺行                │ 批量失败
                                ▼                    ▼
                             missing ◀──────────  failed
                                │ retryHydration       │ retryHydration
                                └──────▶ (删记录→未请求，可重新入队) ◀──────┘

     ready ──reconcile 观察到「DB 已无 AI 产物」──▶ cleared
     ready ──(contentId, revision) 变化（含共享兄弟）──▶ cleared（正文清空）
     cleared ──重新生成摘要/译文 或 重新水合新修订──▶ ready（按形态的解除路径）

   语义表（呈现与水合判定消费方见 selectors.entryNeedsHydration /
   selectArticleBody / Timeline 卡片 / Reader）：
   - loading  : 请求在途（等价旧 hydrationInFlight 的可见化），不重复入队；
   - ready    : 已按 DB 真值水合（content 为空串 = 后端无正文，同样是终态）；
   - cleared  : 显式失效态，两种形态：①AI 产物（摘要/译文）被清理——**正文保留**
                （清理 AI 缓存不动正文），不再自动重取，AI 区块呈现「已清空」，
                重新生成后回 ready；②正文修订失效（invalidateContentRevision：
                条目引用的 contents 已被替换/共享兄弟更新）——**正文清空**，
                需按新修订重取（reader.ts 与 entryNeedsHydration 都按
                「cleared 且 content 为空」判定重入队），在途旧响应被 bump 的
                显式失效计数丢弃；
   - missing  : 响应缺行 = 文章不存在/已被删除，呈现「文章不存在」（重试幂等）；
   - failed   : 请求失败，呈现错误 + 内联重试（不自动重试，与旧 hydrationErrors 同）。

   ── 显式失效规则（单点）────────────────────────────────────────
   1. reconcileBodyEntities（唯一失效入口）：后端快照落地（mergeSnapshotEntries
      的 fromBackend=true 路径，即 reloadFromBackend / reloadFilteredEntries /
      anchorToArticle 三处后端拉取）时逐行对齐 bodyById：
      - 行 AI 产物为空而记录非空 → DB 已删除该产物（清理 AI 缓存把
        ai_summary/translated_content 置 NULL，列表行如实带回 NULL）→ 置
        cleared + bump contentRevision。受影响 id 由**行真值**推导（前端无法
        预知清理范围；CacheCleanupSection 的清空动作完成后必触发 reload，其
        落地即执行本对齐——这就是清理链路的显式失效点）；
      - 行 AI 产物非空且与记录不同（ready 记录）→ 对齐为行真值（含他端再生）；
      - 行 snippet 与记录所对应的视图行不同 → 该文章已被源站重新同步，正文可能
        陈旧 → 记录失效（删记录回未请求态，懒水合自动重取）——探针 P4
        「snippet 更新后正文陈旧、entryNeedsHydration=false 不重取」的修复本体。
        判据 = 「新行 snippet ≠ 被替换视图行的 snippet」：记录存活意味着其水合后
        未曾失效，视图行 snippet 即水合时基线；不按 aiWrittenAt 豁免（正文陈旧
        与 AI 写入时序无关）；loading 记录不参与（在途响应本就会带回新正文）；
      - cleared 记录不被 reload 复活/改写（显式态只能被再次生成解除）；
      - 新鲜度守卫：记录的 aiWrittenAt 晚于行抓取时刻 → 行快照早于本次写入
        （生成刚落库、行是旧查询）→ 跳过，不回退（等价旧 merge 继承对
        「生成完成 → 旧 reload 落地」的保护，但以时间判据表达，不再继承）；
      - loading 记录不参与对齐（AI 产物尚在途）：若其响应携清理前的旧行值落地，
        aiWrittenAt（落地时刻）晚于后续 reload 的行抓取时刻 → 下一轮对齐即清除。
        即：极端并发窗口最多存活到下一次 reload 自愈，且任何时刻 cleared 态
        都不会被写入（本条即「清理后 cleared 不被在途 reload 复活」的实现）。
   2. contentRevision：显式写入/失效计数（清理失效、再次生成各 bump）。在途
      水合响应携带发起时的 revision 快（stamp），落地时与现值比对，不等即丢弃
      ——失效后迟到的旧响应不得覆盖新状态（含对 cleared 的复活企图）。
      R3 边界守卫：记录同时携带条目身份锚（feedId|title，水合时写入）；
      后端快照里同 id 条目“消失后重新出现”且 (contentId, contentRev) 巧合相等
      时，锚不符（已是另一条目）同样失效——正文删除重建复用 id 的缓存边界
      （invalidateContentRevision 第 2 判据）。
   3. 快照替换（六个 merge 调用点）不触碰本缓存的正文——正文随实体而非视图行
      存活，刷新/切视图不丢已加载正文（旧 mergeSnapshotEntries 继承机制对
      「刷新不丢正文 / 水合死区不复发」的保护意图由此承接）。

   ── 水合 AI 列的回退规则（与旧实现逐字同语义）────────────────────
   applyBodyRow 的 AI 字段取 `行值 ?? aiFallback`：行列非 NULL = DB 真值（胜）；
   NULL = DB 无产物 → 保留调用方传入的现值（AI 流式半截/刚生成未落库的产物，
   旧 reader.ts 的 `row.ai_summary ?? a.aiSummary` 同型）。aiFallback 由调用方
   从「当前生效值」（记录 ?? 视图行）取。

   ── 内存预算（TASK-111 纪律）────────────────────────────────────
   BODY_CACHE_MAX = 2000 条 LRU：正文 HTML 是重字段（每条 2-3KB 起），无上限
   会随长会话滚动无界增长（与 viewEntriesCache 的单键预算同型问题）。淘汰即回
   「未请求」态：卡片懒水合判定（无记录 → 需要水合）自动重新入队重取，淘汰无
   正确性影响、只付一次 IPC。2000 ≫ 单视口卡片数（约 30）+ 翻页深度，正常浏览
   不会触顶；触顶淘汰的是最久未使用的条目。

   依赖纪律：本模块是纯实体缓存，**不得** import store 内任何模块（internals /
   slices / selectors 都可能反过来 import 本模块，保持无环）；需要 store 侧
   信息（如生成中标记）一律由调用方以参数传入。
   ============================================================ */

/** 正文/AI 记录状态（判别态；语义表见模块头注） */
export type BodyState = 'loading' | 'ready' | 'cleared' | 'missing' | 'failed';

/** 单篇文章的正文/AI 实体记录（bodyById 的值） */
export interface BodyEntry {
  state: BodyState;
  /** 渲染态正文 HTML（Readability 全文提取后 = 全文；空串 = 后端无正文） */
  content: string;
  /** RSS 原始正文（「全文 ↔ RSS 正文」切换回跳用） */
  rawContent: string;
  /** 译文（cleared 后为空串） */
  translatedContent: string;
  /** AI 摘要（cleared 后为空串） */
  aiSummary: string;
  /** OPT-013 B1：两类产物的缓存签名（null = 未标记版本/未生成）。
      来源与产物同源：水合行真值（applyBodyRow）或生成回读行（applyAiProduct）。
      产物被清空/重试清半截时一并回 null——签名不得脱离对应产物独立存活
      （「新产物 + 旧签名」在记录层也不出现）。 */
  aiSummarySig: string | null;
  translatedSig: string | null;
  /** 正文已被 Readability 全文覆盖 */
  fulltextExtracted: boolean;
  /** 显式失效计数：清理失效 / 再次生成各 bump；在途响应以 stamp 比对丢弃 */
  contentRevision: number;
  /** OPT-008B 第二阶段：记录对应的 DB 内容身份与修订（JOIN contents 带出）。
      DB 修订变化 = 正文真值已更新（含共享兄弟变更）——列表快照合并 / 水合响应
      以 (contentId, contentRev) 判定失效，snippet 未变也不留旧正文。 */
  contentId: number | null;
  contentRev: number;
  /** OPT-008B R3：条目身份锚（`feedId|title`，水合响应落地时记录）。
      快照里同 id 条目“消失后重新出现”且 (contentId, contentRev) 巧合相等时，
      锚不符 = 后端行已是另一条目（文章删除重建复用了 rowid）→ 失效丢弃旧正文。
      publishedAt 不进锚：列表行是原文字符串、条目是解析后的值，表示形态不一致。 */
  entryAnchor: string | null;
  /** AI 产物最近一次真实写入时刻（ms）。对齐失效的新鲜度判据：晚于行抓取
      时刻的写入不回退（详见模块头注「显式失效规则 1」） */
  aiWrittenAt: number;
  /** missing/failed 的呈现文案（missing = 固定文案；failed = 错误消息） */
  message: string;
}

/** 正文缓存条数上限（LRU）。取值依据见模块头注「内存预算」。 */
export const BODY_CACHE_MAX = 2000;

/** 极简 LRU：Map 迭代序 = 插入序，命中即摘除重插，超限删最旧。
    与 internals 的 LRUMap 同形——此处私有副本，避免 bodyCache ↔ internals 互引。 */
class BodyLru extends Map<string, BodyEntry> {
  private readonly max: number;

  constructor(max: number) {
    super();
    this.max = max;
  }

  get(key: string): BodyEntry | undefined {
    const v = super.get(key);
    if (v !== undefined) {
      super.delete(key);
      super.set(key, v);
    }
    return v;
  }

  set(key: string, value: BodyEntry): this {
    super.delete(key);
    super.set(key, value);
    while (super.size > this.max) {
      const oldest = super.keys().next().value;
      if (oldest === undefined) break;
      super.delete(oldest);
    }
    return this;
  }
}

const bodyById: Map<string, BodyEntry> = new BodyLru(BODY_CACHE_MAX);

/* ── 写入通知（响应性接线）────────────────────────────────────
   本缓存是模块级状态（不在 store 快照内），而组件经 useAppStore 的 selector
   订阅读取（entryNeedsHydration / selectArticleBody）。Zustand 只在 store
   状态变化时重算 selector——记录写入若不通知，卡片会停在旧呈现（如响应落地
   后仍显示「加载正文…」）。因此每次真实写入都触发注入的 notify 回调；store.ts
   在 bindAppStore 后注入「bump AppState.bodyCacheNonce」的实现，选择器引用该
   nonce 建立订阅依赖（见 selectors.ts）。notify 由宿主注入而非直接 import
   store：保持本模块零 store 依赖（internals ← bodyCache 单向，无环）。 */
let notifyWrite: (() => void) | null = null;

/** 宿主（store.ts）注入写入通知；多次注入以后一次为准（测试重装载同）。 */
export function bindBodyCacheNotify(fn: () => void): void {
  notifyWrite = fn;
}

/** 每个真实写入的公共尾：触发通知（未注入时静默——纯函数级测试场景）。 */
function touch(): void {
  notifyWrite?.();
}

/** 读记录（命中即刷新 LRU 新鲜度）。无记录 = 未请求/已淘汰。 */
export function getBodyEntry(id: string): BodyEntry | undefined {
  return bodyById.get(id);
}

/** 「文章不存在」的固定呈现文案（missing 终态） */
export const BODY_MISSING_MESSAGE = '文章不存在或已被删除';

function emptyRecord(state: BodyState): BodyEntry {
  return {
    state,
    content: '',
    rawContent: '',
    translatedContent: '',
    aiSummary: '',
    aiSummarySig: null,
    translatedSig: null,
    fulltextExtracted: false,
    contentRevision: 0,
    contentId: null,
    contentRev: 0,
    entryAnchor: null,
    aiWrittenAt: 0,
    message: '',
  };
}

/** 水合入队：置 loading 态并返回失效戳（发起时的 contentRevision）。
    仅应作用于无记录或 **cleared**（正文修订失效待重取）的 id（调用方守卫：
    loading/ready/missing/failed = 终态或在途，不入队；失败重试走 dropBodyEntry）。
    R3 修复：cleared → loading 时**保留显式失效计数**（不回零）——失效后的
    重入队若把计数归零，早于失效发出的在途旧响应（stamp=0）会被重新匹配而
    落地，正是「修订失效被旧正文复活」的窗口；保留计数使旧戳仍被丢弃。
    loading 记录 revision 恒等于为其签发的 stamp（对齐与失效都不触碰 loading），
    故不存在「响应被丢后卡死 loading」的死区。 */
export function markBodyLoading(id: string): number {
  const existing = bodyById.get(id);
  if (existing && existing.state !== 'cleared') return existing.contentRevision;
  const revision = existing?.contentRevision ?? 0;
  bodyById.set(id, { ...emptyRecord('loading'), contentRevision: revision });
  touch();
  return revision;
}

/** 显式 AI 写入（生成落账 / 失效 / 重试清半截）的公共段：bump revision +
    刷新 AI 写入时刻。state 由调用方给定（生成 → ready 解除 cleared；
    失效 → cleared）。 */
function withExplicitAiWrite(rec: BodyEntry, state: BodyState): BodyEntry {
  return { ...rec, state, contentRevision: rec.contentRevision + 1, aiWrittenAt: Date.now() };
}

/** OPT-008B R3：由后端行（详情/批量水合）计算条目身份锚。行缺 feed_id/title
    （旧 mock）→ null（不参与锚比较，保持既有行为）。 */
function anchorOfRow(row: { feed_id?: number | null; title?: string | null }): string | null {
  if (row.feed_id == null || row.title == null) return null;
  return `${row.feed_id}|${row.title}`;
}

/** 水合响应落地（单行命中）：写 ready 记录。
    - stamp 比对：现记录 revision ≠ 发起时快 → 期间被显式失效/他路接管，
      本响应过期，整行丢弃（cleared/最新状态不被旧响应复活）；
    - aiFallback：行 AI 列为 NULL 时保留的现值（流式半截/未落库产物，
      规则见模块头注「水合 AI 列的回退规则」）；
    - 无记录（LRU 淘汰后迟到落地等）→ 以空记录为底新建，stamp=0 恒匹配。 */
export function applyBodyRow(
  id: string,
  row: { content_html?: string | null; translated_content?: string | null; ai_summary?: string | null; fulltext_extracted?: boolean | null; content_id?: number | null; content_revision?: number | null; feed_id?: number | null; title?: string | null; ai_summary_sig?: string | null; translated_sig?: string | null },
  stamp: number,
  aiFallback: { translatedContent: string; aiSummary: string },
): void {
  const current = bodyById.get(id);
  if (current && current.contentRevision !== stamp) return; // 显式失效在途 → 响应过期
  const base = current ?? emptyRecord('ready');
  /* OPT-013 B1：签名随产物来源走——行携带产物（非 NULL）时签名取行值
     （行缺省签名字段 = 未标记版本 → null）；行产物为 NULL（回退到 aiFallback
     的流式半截）时保留记录现值（签名不脱离产物独立改写）。 */
  const summarySig = row.ai_summary != null ? row.ai_summary_sig ?? null : base.aiSummarySig;
  const translatedSig = row.translated_content != null ? row.translated_sig ?? null : base.translatedSig;
  bodyById.set(id, {
    ...base,
    state: 'ready',
    content: row.content_html ?? '',
    rawContent: row.content_html ?? '',
    translatedContent: row.translated_content ?? aiFallback.translatedContent,
    aiSummary: row.ai_summary ?? aiFallback.aiSummary,
    aiSummarySig: summarySig,
    translatedSig,
    fulltextExtracted: row.fulltext_extracted ?? base.fulltextExtracted,
    contentRevision: stamp,
    /* OPT-008B 第二阶段：记录 DB 内容身份/修订——行携带即刷新（本响应是新鲜
       详情）；行缺省（旧 mock）保持现值。 */
    contentId: row.content_id ?? base.contentId,
    contentRev: row.content_revision ?? base.contentRev,
    entryAnchor: anchorOfRow(row) ?? base.entryAnchor,
    aiWrittenAt: Date.now(),
    message: '',
  });
  touch();
}

/** OPT-008B 第二阶段：正文真值修订变化（列表快照合并发现条目引用的 contents
    已更新——snippet 未变也失效；共享兄弟内容变更经同一路径传播）。
    失效判据（任一成立即失效）：
    1. 记录里 DB 身份/修订与给定不符；
    2. anchor 非空且记录已有锚时锚不符——同 id 条目“消失后重新出现”已是另一
       条目（文章删除重建复用 rowid）而 (contentId, contentRev) 巧合相等的
       边界（R3；正常重入窗口的同一锚比较相等，不误失效）。
    失效 = 清掉正文/提取态并回 cleared（保留 AI 字段：产物是条目级，正文重取后
    由用户/自动流程再生成），bump 显式失效计数使在途旧详情响应丢弃。
    无记录（未水合）→ 无需处理（下次水合即新内容）。 */
export function invalidateContentRevision(
  id: string,
  contentId: number | null,
  revision: number,
  anchor: string | null = null,
): boolean {
  const current = bodyById.get(id);
  if (!current) return false;
  const samePair = current.contentId === contentId && current.contentRev === revision;
  const anchorCompatible =
    anchor === null || current.entryAnchor === null || current.entryAnchor === anchor;
  if (samePair && anchorCompatible) return false;
  bodyById.set(id, {
    ...current,
    state: 'cleared',
    content: '',
    rawContent: '',
    fulltextExtracted: false,
    contentId,
    contentRev: revision,
    entryAnchor: anchor ?? current.entryAnchor,
    contentRevision: current.contentRevision + 1,
  });
  touch();
  return true;
}

/** 水合响应缺行（含空 rows）：文章不存在终态。stamp 比对同 applyBodyRow。 */
export function markBodyMissing(id: string, stamp: number): void {
  const current = bodyById.get(id);
  if (current && current.contentRevision !== stamp) return;
  bodyById.set(id, { ...(current ?? emptyRecord('missing')), state: 'missing', message: BODY_MISSING_MESSAGE });
  touch();
}

/** 水合失败：failed 终态 + 错误文案（内联重试依据）。stamp 比对同 applyBodyRow。 */
export function markBodyFailed(id: string, stamp: number, message: string): void {
  const current = bodyById.get(id);
  if (current && current.contentRevision !== stamp) return;
  bodyById.set(id, { ...(current ?? emptyRecord('failed')), state: 'failed', message });
  touch();
}

/** 删除记录 → 回「未请求」态：retryHydration 的重试入口、以及未来任何需要
    强制重取的路径。LRU 淘汰与删除语义相同。 */
export function dropBodyEntry(id: string): void {
  if (bodyById.delete(id)) touch();
}

/** 全文提取成功：正文被 Readability 结果覆盖（rawContent 保留 RSS 原文）。
    AI 字段不动；不 bump revision（非显式失效，同一实体的正文内容更新）。
    无记录 = 未水合，提取前提不成立（调用方守卫），此处防御性忽略。
    OPT-008B R3：identity = 提取响应携带的提交后 (contentId, contentRev)
    ——落记录使下一次 reload 的修订比较**相等**（DB 行已是该修订），刚成功
    提取的正文不会被按“修订变化”清空重取；缺省（旧后端/降级）时保持现值。 */
export function applyExtractedFulltext(
  id: string,
  html: string,
  identity?: { contentId: number | null; contentRev: number },
): void {
  const current = bodyById.get(id);
  if (!current) return;
  bodyById.set(id, {
    ...current,
    content: html,
    fulltextExtracted: true,
    state: 'ready',
    contentId: identity ? identity.contentId : current.contentId,
    contentRev: identity ? identity.contentRev : current.contentRev,
  });
  touch();
}

/** AI 生成完成（摘要/译文回读 DB 消毒版后落账）：解除 cleared、bump revision。
    这是 cleared → ready 的唯一解除路径（模块头注状态机）。
    OPT-013 B1：`signature` 与产物**成对**写入（undefined = 不改写签名——流式
    半截场景；null = 产物清空/无签名）。回读路径应传回读行的签名（可能为
    null：存量无签名产物重新生成后行带新签名，回读即取到）。
    仅更新**已存在**的记录；无记录返回 false（文章从未水合时调用方回退写
    视图行过渡字段——此时若凭空建记录，会让懒水合判定误判「正文已水合」，
    卡片正文永远取不到）。 */
export function applyAiProduct(
  id: string,
  field: 'aiSummary' | 'translatedContent',
  value: string,
  signature?: string | null,
): boolean {
  const current = bodyById.get(id);
  if (!current) return false;
  const sigPatch: Partial<BodyEntry> = signature === undefined
    ? {}
    : (field === 'aiSummary' ? { aiSummarySig: signature } : { translatedSig: signature });
  bodyById.set(id, withExplicitAiWrite({ ...current, [field]: value, ...sigPatch }, 'ready'));
  touch();
  return true;
}

/** AI 流式增量追加（打字机）。记录存在 → 追加进记录并刷新 aiWrittenAt
    （保护流式产物不被旧行对齐清除），cleared 期间生成视为再次生成的开始
    （回 ready，完成时 applyAiProduct 再 bump）；无记录 → 返回 false，
    调用方回退写视图行。 */
export function appendAiDelta(id: string, field: 'aiSummary' | 'translatedContent', delta: string): boolean {
  const current = bodyById.get(id);
  if (!current) return false;
  bodyById.set(id, {
    ...current,
    [field]: (current[field] || '') + delta,
    state: current.state === 'cleared' ? 'ready' : current.state,
    aiWrittenAt: Date.now(),
  });
  touch();
  return true;
}

/** 后端行事实 → reconcile 的输入形态（id 已字符串化、NULL 已转 ''） */
export interface BodyRowFact {
  aiSummary: string;
  translatedContent: string;
  /** OPT-013 B1：行真值携带的缓存签名（null = 未标记版本）——对齐时与产物同时写 */
  aiSummarySig: string | null;
  translatedSig: string | null;
  /** 行 snippet（正文可能变化的伴生信号，见模块头注 P4 规则） */
  snippet: string;
}

/** TASK-122 显式失效单点（规则见模块头注）：后端快照落地时逐 id 对齐 bodyById。
    rowsFetchedAt = 该批行的**发起抓取时刻**（调用方在 await 前取 Date.now()），
    用于新鲜度守卫；isGenerating = 该 id 摘要/译文是否生成中（流式产物不受
    行对齐影响——行是抓取前的旧值，生成完成时经 applyAiProduct 落账）。 */
export function reconcileBodyEntities(
  facts: ReadonlyMap<string, BodyRowFact>,
  rowsFetchedAt: number,
  isGenerating: (id: string) => boolean,
  prevSnippets: ReadonlyMap<string, string>,
): void {
  for (const [id, rec] of [...bodyById.entries()]) {
    if (rec.state !== 'ready' && rec.state !== 'cleared') continue; // loading/missing/failed 不参与
    if (isGenerating(id)) continue;
    const fact = facts.get(id);
    if (!fact) continue;
    /* P4（探针 P4 修复本体）：行 snippet 变化 = 文章被重新同步 → 正文可能陈旧
       → 删记录回未请求态（懒水合自动重取）。见模块头注「显式失效规则 1」。 */
    const prevSnippet = prevSnippets.get(id);
    if (prevSnippet !== undefined && prevSnippet !== fact.snippet) {
      bodyById.delete(id);
      touch();
      continue;
    }
    const hadAi = rec.aiSummary !== '' || rec.translatedContent !== '';
    const rowHasAi = fact.aiSummary !== '' || fact.translatedContent !== '';
    if (!hadAi && !rowHasAi) continue;
    if (rec.aiWrittenAt > rowsFetchedAt) continue; // 记录比行新（生成刚落库、行是旧查询）
    if (hadAi && !rowHasAi) {
      // DB 已删除 AI 产物（清理 AI 缓存）→ 显式失效：cleared + bump（不再被 reload 复活）
      bodyById.set(id, withExplicitAiWrite({ ...rec, aiSummary: '', translatedContent: '', aiSummarySig: null, translatedSig: null }, 'cleared'));
      touch();
    } else if (rec.state === 'ready' && (rec.aiSummary !== fact.aiSummary || rec.translatedContent !== fact.translatedContent)) {
      // 行带来不同的 DB 真值（他端再生/本端未见过）→ 对齐为行真值（不解除 cleared）；
      // OPT-013 B1：签名随行真值一起对齐（产物与签名成对，不出现新产物配旧签名）
      bodyById.set(id, { ...rec, aiSummary: fact.aiSummary, translatedContent: fact.translatedContent, aiSummarySig: fact.aiSummarySig, translatedSig: fact.translatedSig, aiWrittenAt: rowsFetchedAt });
      touch();
    }
  }
}

/** 测试专用：清空全部记录（frontend-regression 的夹具复位；生产代码不得调用） */
export function resetBodyCacheForTests(): void {
  bodyById.clear();
  touch();
}

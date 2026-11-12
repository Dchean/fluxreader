import type {
  ArticleEntry,
  CategoryGroup,
  FeedItem,
  FeedAiMode,
  PublishedAtSource,
} from '../types';
import { normalizeFeedAiMode } from '../types';

/* ============================================================
   Tauri invoke 封装 —— 浏览器开发环境回退 mock
   ============================================================ */

/** 检测是否在 Tauri 窗口内（有无 IPC） */
export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let invokeFn: Invoke | null = null;

/** Rust AppError 序列化后注入的字段（Tauri invoke 拒绝时抛 {code,message}）。
    统一提取成可读文案——此前各处 `String(e)` 会显示出 [object Object]。 */
export function extractError(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'object' && e !== null) {
    const rec = e as Record<string, unknown>;
    if (typeof rec.message === 'string') return rec.message;
    if (typeof rec.code === 'string') return `[${rec.code}]`;
  }
  return String(e);
}

/** 从 IPC 错误对象提取稳定错误码（extractError 只给消息；分支处置如
 * 「stalePreview 要求重新预览」需要码）。非对象/无码返回 null。 */
export function extractErrorCode(e: unknown): string | null {
  if (typeof e === 'object' && e !== null) {
    const rec = e as Record<string, unknown>;
    if (typeof rec.code === 'string') return rec.code;
  }
  return null;
}

/** Validate the actual IPC representation before allocating the Blob backing buffer. */
export function imageBytes(value: unknown): Uint8Array<ArrayBuffer> {
  const limit = 25 * 1024 * 1024;
  const invalid = () => new Error('Invalid image IPC byte response');
  if (value instanceof ArrayBuffer) {
    if (value.byteLength > limit) throw { code: 'responseTooLarge', message: '图片超过大小限制' };
    return new Uint8Array(value);
  }
  if (value instanceof Uint8Array) {
    if (value.byteLength > limit) throw { code: 'responseTooLarge', message: '图片超过大小限制' };
    return new Uint8Array(value);
  }
  if (!Array.isArray(value)) throw invalid();
  if (value.length > limit) throw { code: 'responseTooLarge', message: '图片超过大小限制' };
  for (const byte of value) if (!Number.isInteger(byte) || byte < 0 || byte > 255) throw invalid();
  return new Uint8Array(value);
}
async function getInvoke(): Promise<Invoke | null> {
  if (!isTauri()) return null;
  if (invokeFn) return invokeFn;
  const { invoke } = await import('@tauri-apps/api/core');
  invokeFn = invoke as Invoke;
  return invokeFn;
}

// 本机读/藏写入在进入 IPC 前按调用顺序登记；DB Mutex 本身不保证 IPC FIFO。
// read 共用一条前沿以包含 bulk、范围标读与同文读闭包；收藏独立，不互相阻塞。
// Note: 顺序与失败边界 — 见 .agents/notes/implemented/architecture/2026-10-05-乐观写入的版本守卫.md
const localFlagWriteFrontiers: Record<'read' | 'starred', Promise<void> | null> = {
  read: null,
  starred: null,
};
function runLocalFlagWrite<T>(field: 'read' | 'starred', write: () => Promise<T>): Promise<T> {
  const task = (localFlagWriteFrontiers[field] ?? Promise.resolve()).then(write);
  const tail = task.then(() => {}, () => {});
  localFlagWriteFrontiers[field] = tail;
  void tail.then(() => {
    if (localFlagWriteFrontiers[field] === tail) localFlagWriteFrontiers[field] = null;
  });
  return task;
}

/** 只读等待：测试harness在复位/卸载前等待调用时已登记的读/藏写；不能改写或清空队列。
    store 的 UI action 多为 void，故测试不能通过 await action 判断这些 IPC 已结束。 */
export async function awaitLocalFlagWriteFrontier(): Promise<void> {
  await Promise.all([localFlagWriteFrontiers.read, localFlagWriteFrontiers.starred]);
}

// PlayerBar 的时间事件/关闭 effect 直接写播放进度。资料库恢复先停播放，再等待这些
// 已登记写结束；恢复期间拒绝迟到事件，不能在新库重新写入旧剧集 URL。
let playbackWritesPaused = false;
const playbackWrites = new Set<Promise<null>>();
export async function runPlaybackRestoreBoundary<T>(op: () => Promise<T>): Promise<T> {
  playbackWritesPaused = true;
  try {
    await Promise.allSettled([...playbackWrites]);
    return await op();
  } finally { playbackWritesPaused = false; }
}

/** 由实际 store 装配注入：成功保存/应用配置、批量清理后使显示验签过时。
    API 层不导入 store，保存失败不会发失效信号。 */
let aiCacheInvalidated: (() => void) | null = null;
export function bindAiCacheInvalidationHandler(handler: () => void): void {
  aiCacheInvalidated = handler;
}

/* ============================================================
   后端行类型（Rust 侧 Serialize 的镜像）
   ============================================================ */

export interface FolderRow {
  id: number;
  name: string;
  layout: string;
  auto_summary: boolean;
  auto_translate: boolean;
  summary_mode: FeedAiMode;
  translate_mode: FeedAiMode;
  collapsed: boolean;
}

export interface FeedRow {
  id: number;
  sync_id: string;
  subscription_state: 'active' | 'retained';
  effective_layout: FeedItem['layout'];
  folder_id: number;
  feed_url: string;
  site_url: string | null;
  title: string;
  favicon_url: string | null;
  layout: string;
  /** 旧兼容列（不再是有效真值；仅作为缺 summary_mode 时的映射输入） */
  auto_summary: boolean;
  auto_translate: boolean;
  /** OPT-011：AI 开关三态真值（缺省inherit；旧bool不参与解析） */
  summary_mode?: string | null;
  translate_mode?: string | null;
  fetch_failed: boolean;
  fetch_error: string | null;
  last_fetched_at: string | null;
}

export interface ArticleListItemRow {
  id: number;
  feed_id: number;
  title: string;
  author: string | null;
  snippet: string;
  image_url: string | null;
  gallery_first_image_url: string | null;
  image_count: number;
  media_revision: number;
  retained: boolean;
  body_missing: boolean;
  enclosure_url: string | null;
  enclosure_mime: string | null;
  duration_sec: number | null;
  ai_summary: string | null;
  source: string;
  published_at: string | null;
  is_read: boolean;
  is_starred: boolean;
  /** with_content 时填充 */
  url?: string | null;
  content_html?: string | null;
  translated_content?: string | null;
  fulltext_extracted?: boolean;
  /** OPT-008B 第二阶段：内容身份 + 修订（前端失效依据；两种模式都返回） */
  content_id?: number | null;
  content_revision?: number;
  /** OPT-013 B1：AI 产物缓存签名（生成时输入身份的 sha256；存量/未生成行 null）。
   *  产物非空且签名为空 = 旧无签名产物——可显示、可重新生成，但不冒充缓存
   *  （B3 起 UI 五态判定呈现为「已过期」，见 store/slices/ai.ts 的
   *  selectAiProductState）。 */
  ai_summary_sig?: string | null;
  translated_sig?: string | null;
  /** v33：永久首次见到时间（RFC3339）；发布时间的兜底/推断标记据此判定。 */
  first_seen_at?: string | null;
  /** v33：published_at 来源（feed / feed_clamped / inferred_fetch / legacy_unknown）。 */
  published_at_source?: PublishedAtSource;
}

export interface ArticleDetailRow extends ArticleListItemRow {
  images: string[];
  url: string | null;
  content_html: string | null;
  translated_content: string | null;
  fulltext_extracted: boolean;
}

export interface RefreshSummary {
  new_articles: number;
  failed_feeds: number;
}

export type LocalSyncMode = 'realtime' | 'scheduled';
export interface LocalSyncConnection {
  endpoint: string;
  username: string;
  password: string;
  mode: LocalSyncMode;
  intervalMinutes: number;
}
/** 后端权威快照；connected 仅表示已存连接，三个时间字段均为 Unix 秒。 */
export interface LocalSyncStatus {
  connected: boolean;
  endpoint: string | null;
  username: string | null;
  mode: LocalSyncMode;
  interval_minutes: number;
  generation: number;
  library_id: string;
  pending_operations: number;
  pending_bodies: number;
  pending_publications: number;
  last_state_sync: number;
  last_body_sync: number;
  next_retry_at: number;
  last_error: string | null;
}
export interface PortableBackupInfo {
  path: string;
  size_bytes: number;
  digest: string;
  sources: number;
  articles: number;
  starred: number;
  missing_bodies: number;
}
export interface PortableBackupPreview {
  digest: string;
  format: 4;
  created_at: string;
  folders: number;
  sources: number;
  articles: number;
  starred: number;
  protected_bodies: number;
  missing_bodies: number;
  replaces_current_library: true;
  disconnects_services: true;
}

/** 完整数据库快照（手动备份、迁移前/镜像删除前自动快照共用）。 */
export interface DatabaseBackupInfo {
  path: string;
  fileName: string;
  sizeBytes: number;
  schemaVersion: number;
  fromVersion: number;
  toVersion: number;
  createdAt: string;
  kind: 'manual' | 'migration' | 'mirror' | string;
}

/** 待确认连接建议（OPT-006 R1）：仅非敏感字段 + 激活 CAS 版本号；
 * 激活需重新输入密码，并把 `version` 原样回传给 syncSave（后端独立校验，不信任 UI）。 */
export interface PendingConnection {
  version: number;
  sync_protocol?: string | null;
  greader_endpoint?: string | null;
  greader_username?: string | null;
}

/** extract_fulltext 的结果（TASK-076 / P2-10 后半）：降级不再静默。
 * degraded=true 时 html 是保留的原文，reason 说明为何没采用提取结果。
 * OPT-008B R3（wire 增字段）：提交成功时携带实际内容身份/修订（Rust 侧
 * camelCase 序列化为 contentId/contentRevision）——前端落 bodyCache 记录，
 * 下一次 reload 不把刚成功提取的正文按「修订变化」清空重取。降级可没有提交
 * 身份；未降级却缺少有效提交身份的响应不能作为前端恢复成功的依据。 */
export interface ExtractFulltextResult {
  html: string;
  degraded: boolean;
  reason: string | null;
  contentId?: number | null;
  contentRevision?: number;
}

/** OPT-013 B2：翻译分段计划（ai_cache_check 响应的一部分）。
 *  - total：当前输入下的总段数（短文 = 1；空正文 = 0）；
 *  - completed：当前签名下已持久化的完成段数（重启后可恢复的依据）；
 *  - discarded：因输入变化（模型/提示词/正文修订/分段策略）作废的旧片段数——
 *    > 0 表示存在作废旧片段、界面必须如实告知（B2 最小提示）；但**不**一律
 *    等于整篇重新生成：`0 < completed < total` 时复用已完成段、从第
 *    completed+1 段继续（R2 复审口径收紧）。 */
export interface TranslationSegmentPlan {
  total: number;
  completed: number;
  discarded: number;
}

/** OPT-013 B1/B2：ai_cache_check 响应（命中判定 + 翻译分段计划）。 */
export interface AiCacheCheckResult {
  summaryVerified: boolean;
  translationVerified: boolean;
  translationSegments: TranslationSegmentPlan;
}

export interface SyncReport {
  pushed_states: number;
  pushed_feeds: number;
  pulled_feeds: number;
  pulled_entries: number;
  merged_states: number;
  /** TASK-100 P3-9：对账删除计数——feeds 阶段对账发现远端已删除的订阅源数
      （后端 sync/mod.rs SyncReport.removed_feeds，此前前端未消费）。纯信息展示。 */
  removed_feeds?: number;
  errors: string[];
}

export interface SyncRunResult {
  activity_id: number;
  feeds: SyncReport | null;
  states: SyncReport | null;
  error: { stage: 'feeds' | 'states'; code: string; message: string } | null;
}

export interface ExpiredRetryResult {
  requeued: number;
  bound_pending: number;
  bindable_retry: number;
  local_only: number;
  remaining_expired: number;
}

export interface SyncStatusInfo {
  connected: boolean;
  endpoint: string | null;
  /** 服务端账户名（连接时记录，设置页动态显示） */
  account: string | null;
  last_sync: number;
  /** 同步协议："greader" | "fever" */
  protocol?: string | null;
}

/** sync_queue_stats 返回（TASK-116 四态展示 + 队列生命周期分类）。
 * waiting = 真正可推送且未确认的 bound_pending + retry_wait；
 * failed = 终态 failed；retry_wait = 可重试失败（前端可合并展示「部分失败」）；
 * local_only / bindable_retry / expired_unconfirmed 单独给出，不再混入 waiting；
 * last_error = retry_wait/failed 中最近一次推送失败摘要（Rust 侧截断到 200 字符）。
 *
 * 新增字段在旧事件载荷/旧测试夹具中可能缺失，故保持可选；Rust IPC 与事件
 * 始终返回完整分类，组件由 store 的同步字段补齐。 */
export interface SyncQueueStats {
  account_generation?: number;
  waiting: number;
  failed: number;
  local_only?: number;
  bindable_retry?: number;
  bound_pending?: number;
  retry_wait?: number;
  expired_unconfirmed?: number;
  last_error: string | null;
}

/** sync_local_feeds 的结构化结果。**部分失败仍是可消费结果**：
 * `ok=false` 且 `failed>0`，调用方不得把整次添加/同步宣布成功。 */
export interface SyncLocalFeedsResult {
  ok: boolean;
  protocol: 'greader' | 'fever' | null;
  writeBack: boolean;
  queued: number;
  pushed: number;
  failed: number;
  errors: string[];
  message: string;
  /** 前端兼容层字段：旧后端纯文本回执解析失败时为 false（未知，不得当成功）。 */
  structured?: boolean;
}

/* ---------- OPT-009 B3：订阅写回能力矩阵与持久回执 ---------- */

/** 单动作的远端写回支持度。**逐字对应 Rust `SubscriptionWriteSupport` 的
 *  serde 输出**（src-tauri/src/commands/sync.rs）——改任一侧必须同步改另一侧
 *  （Rust 单测锁定 serde 名称）。 */
export type SubscriptionWriteSupport = 'Supported' | 'LocalOnly' | 'Unsupported';

/** 单动作的能力声明：结构化枚举 + 面向用户的如实说明（与 support 同源；
 *  确认弹窗/能力卡直接采用，不在 UI 另写一套说法）。 */
export interface SubscriptionActionCapability {
  support: SubscriptionWriteSupport;
  detail: string;
}

/** B2 已登记的能力边界（code 为稳定标识，detail 为用户可读说明）。 */
export interface SubscriptionCapabilityLimit {
  code: string;
  detail: string;
}

/** subscription_capabilities 返回（OPT-009 B3；后端单一事实源，只读）。 */
export interface SubscriptionCapabilities {
  connected: boolean;
  /** "greader" | "fever"；未连接为 null */
  protocol: 'greader' | 'fever' | null;
  /** 能力边界的一句话概述（设置页能力卡/提示条直接采用） */
  overview: string;
  subscribe: SubscriptionActionCapability;
  edit_title: SubscriptionActionCapability;
  move_folder: SubscriptionActionCapability;
  unsubscribe: SubscriptionActionCapability;
  rename_folder: SubscriptionActionCapability;
  delete_folder: SubscriptionActionCapability;
  empty_folder_remote: SubscriptionActionCapability;
  limitations: SubscriptionCapabilityLimit[];
}

/** subscription_outbox_stats 返回（OPT-009 B3）：**当前账号限定**的订阅写回
 *  回执（异账号暂停的队项不混入）。pending = 待推/待重试；failed = pending 中
 *  attempts>0 的子集；local_only = 远端不支持、仅本地生效的终态项；
 *  last_error = 最近失败摘要（Rust 侧已截断）。 */
export interface SubscriptionOutboxStats {
  pending: number;
  failed: number;
  local_only: number;
  last_error: string | null;
}

/** sync_save 返回：首连且本地有未绑定源时 firstConnect=true
 *（前端据此弹「同步本地订阅到账户」确认框） */
export interface SyncSaveResult {
  message: string;
  firstConnect: boolean;
  unboundLocalFeeds: number;
}

/** check_for_updates 返回（OPT-014 / F16）：Rust 侧固定请求官方仓库
 * releases/latest（禁止跟随重定向）；url 只可能是本仓库 releases 域——
 * html_url 缺失时回落后端官方发布页，已提供但非受信/非法时整体报错
 * （不会拿官方页顶替）。版本比较仍在前端 compareVersions。 */
export interface UpdateCheckResult {
  version: string;
  url: string;
}

export interface ArticleListArgs {
  feed_id?: number | null;
  folder_id?: number | null;
  only_unread?: boolean;
  only_starred?: boolean;
  only_today?: boolean;
  /** 当前阅读会话捕获的本地日界：[since_ms, until_ms)。 */
  since_ms?: number;
  until_ms?: number;
  newest_first?: boolean;
  limit?: number;
  offset?: number;
  /** 附带正文 HTML（社交/通知布局直接渲染，免逐篇水合） */
  with_content?: boolean;
  /** 布局过滤（TASK-094 / REQ-107，可选）：后端按 feed 级覆盖 → 分类兜底过滤，
      列表分页因此带布局维度。省略 = 不过滤。 */
  layout?: string;
  /** TASK-117：keyset 续拉游标锚——上一页最后一行的 (published_at 原文, id)。
      两者成对给出才生效（后端缺一回落 OFFSET 既有语义）；last_published 必须
      原样透传后端返回的 published_at 字符串（RFC3339），不得本地重格式化——
      后端谓词与排序共用同一字符串比较口径，重格式化会让游标错位。
      省略 = 走既有 OFFSET 语义（首屏/锚定路径不带本键）。 */
  last_published?: string | null;
  last_id?: number | null;
}

/** OPT-008B R2（N1）：读写入回执行——读闭包真实翻转的条目（含同 URL 副本）。 */
export interface ReadWriteReceipt {
  id: number;
  feed_id: number;
}

/** 后端聚合的精确条目计数（按 feed 分组）——侧边栏数字用，不受列表分页 limit 影响 */
export interface FeedCountsRow {
  feed_id: number;
  total: number;
  unread: number;
  starred: number;
  today: number;
}

/** 与源内等价条目列表同口径的范围精确计数。 */
export interface ScopeCountsRow {
  scope_id: number;
  total: number;
  unread: number;
  starred: number;
  today: number;
}

/** OPT-008B：`scope_counts` 返回——全部 / 每分类 / 每源（当前布局过滤后），
    与列表/搜索/markAll 同一显示集合谓词族。 */
export interface ScopeCountsDetailRow {
  all: { total: number; unread: number; starred: number; today: number };
  folders: ScopeCountsRow[];
  feeds: ScopeCountsRow[];
}

/* ============================================================
   API 门面：Tauri 调用 + 浏览器 mock 回退
   ============================================================ */

export const api = {
  async listFolders(): Promise<FolderRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_folders') as FolderRow[]) : null;
  },
  async createFolder(name: string, layout: string): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('create_folder', { name, layout }) as number) : null;
  },
  async deleteFolder(id: number): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('delete_folder', { id }) as null) : null;
  },
  async updateFolderLayout(id: number, layout: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_folder_layout', { id, layout }) as null) : null;
  },
  async listFeeds(): Promise<FeedRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_feeds') as FeedRow[]) : null;
  },
  async listArticleSources(): Promise<FeedRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_article_sources') as FeedRow[]) : null;
  },
  async listArticleMemberIds(args: ArticleListArgs): Promise<number[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_article_member_ids', { args }) as number[]) : null;
  },
  async getArticleUpdates(ids: number[]): Promise<{ items: ArticleListItemRow[]; missing_ids: number[] } | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_article_updates', { ids }) as { items: ArticleListItemRow[]; missing_ids: number[] }) : null;
  },
  /* OPT-017F：feedCounts / scopeCounts 两个分项读取包装已删（全仓零调用方；
     OPT-008B R2 起计数统一走下方 countsSnapshot 组合快照）。对应 IPC 命令
     （feed_counts / scope_counts）保留在 Rust 注册表，命令签名不随包装器删除
     ——见 .agents/notes/implemented/simplification/2026-10-16-零消费入口清理与id解析单点.md。 */
  /** OPT-008B R2（N2/P3-4）：**计数组合快照**——同一开关值、同一 DB 锁内返回
      源计数（布局无关）+ scope 精确计数（按布局）。对账/启动装载只发这一条
      IPC，源键与聚合键不会来自两个时刻。 */
  async countsSnapshot(layout?: string): Promise<{ feeds: FeedCountsRow[]; scope: ScopeCountsDetailRow } | null> {
    const inv = await getInvoke();
    return inv
      ? (await inv('counts_snapshot', { layout: layout ?? null }) as { feeds: FeedCountsRow[]; scope: ScopeCountsDetailRow })
      : null;
  },
  /** 添加订阅源（OPT-011：新 UI 传三态 mode；旧 caller 可省略 mode 走旧 bool 显式语义） */
  async addFeed(
    feedUrl: string,
    title: string | null,
    folderId: number | null,
    layout: string,
    autoSummary: boolean | null,
    autoTranslate: boolean | null,
    syncToBackend: boolean,
    summaryMode?: FeedAiMode,
    translateMode?: FeedAiMode,
  ): Promise<FeedRow | null> {
    const inv = await getInvoke();
    return inv
      ? (await inv('add_feed', {
          feedUrl,
          title,
          folderId,
          layout,
          autoSummary,
          autoTranslate,
          summaryMode: summaryMode ?? null,
          translateMode: translateMode ?? null,
          syncToBackend,
        }) as FeedRow)
      : null;
  },
  async deleteFeed(id: number): Promise<null> {
    const mount = await getInvoke();
    return mount ? (await mount('delete_feed', { id }) as null) : null;
  },
  /** 编辑源：标题/分类/布局/AI 开关一次性更新（undefined 字段保持不变）。
   * OPT-011：新 UI 传三态 summaryMode/translateMode；旧 caller 仍可只传 bool
   * （后端映射显式 on/off，见 db::update_feed_full）。 */
  async updateFeed(args: {
    id: number;
    title?: string;
    folderId?: number;
    layout?: string;
    autoSummary?: boolean;
    autoTranslate?: boolean;
    summaryMode?: FeedAiMode;
    translateMode?: FeedAiMode;
  }): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_feed', args) as null) : null;
  },
  async renameFolder(id: number, name: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('rename_folder', { id, name }) as null) : null;
  },
  /** 刷新单个订阅源（直连），返回新增条数 */
  async refreshFeed(feedId: number): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('refresh_feed', { feedId }) as number) : null;
  },
  async updateFeedLayout(id: number, layout: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_feed_layout', { id, layout }) as null) : null;
  },
  async listArticles(args: ArticleListArgs): Promise<ArticleListItemRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_articles', { args }) as ArticleListItemRow[]) : null;
  },
  /** 计算某篇文章在当前筛选排序下的绝对位置（0 起）——搜索/深层打开文章的双向分页锚定 */
  async articleIndex(args: ArticleListArgs, articleId: number): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('article_index', { args, articleId }) as number | null) : null;
  },
  /** 文章搜索：LIKE 子串匹配（标题/正文/摘要/AI 摘要/翻译，词间 AND）。
   *  TASK-100 D29 更正注释：实际入口从来是 LIKE 子串（src-tauri db/articles.rs
   *  search_articles），FTS5 只是历史遗留的提法，后端并无 FTS5 表。浏览器环境返回 null。 */
  async searchArticles(query: string, limit?: number): Promise<ArticleListItemRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('search_articles', { query, limit }) as ArticleListItemRow[]) : null;
  },
  async getArticle(id: number): Promise<ArticleDetailRow | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_article', { id }) as ArticleDetailRow) : null;
  },
  /** 批量拉取文章详情（正文水合专用）：一次 IPC 返回多篇，替代逐篇 getArticle */
  async getArticles(ids: number[]): Promise<ArticleDetailRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_articles', { ids }) as ArticleDetailRow[]) : null;
  },
  /** 单条标读/取消标读。R2（N1）：读闭包可能一并标读同 URL 未读副本——
   * 返回**真实翻转行回执**（`[{id, feed_id}]`），供前端让受影响条目/计数本地
   * 收敛；mock/旧后端返回 null 时按无回执处理（仅乐观行）。 */
  async setRead(id: number, read: boolean): Promise<ReadWriteReceipt[] | null> {
    return runLocalFlagWrite('read', async () => {
      const inv = await getInvoke();
      return inv ? (await inv('set_read', { id, read }) as ReadWriteReceipt[]) : null;
    });
  },
  /** 批量标读：一次 IPC 处理整批 id（AUDIT P3[F4]）。语义与逐条 setRead 等价，见
   *  Rust 侧 set_read_bulk 的契约注释（每个 id 都走 record_read_state + 入队）。
   *  R2（N1）：返回整批真实翻转行回执（去重升序；含闭包副本）。 */
  async setReadBulk(ids: number[], read: boolean): Promise<ReadWriteReceipt[] | null> {
    return runLocalFlagWrite('read', async () => {
      const inv = await getInvoke();
      return inv ? (await inv('set_read_bulk', { ids, read }) as ReadWriteReceipt[]) : null;
    });
  },
  async setStarred(id: number, starred: boolean): Promise<null> {
    return runLocalFlagWrite('starred', async () => {
      const inv = await getInvoke();
      return inv ? (await inv('set_starred', { id, starred }) as null) : null;
    });
  },
  async markAllRead(
    feedId: number | null,
    folderId: number | null,
    opts?: { starredOnly?: boolean; sinceMs?: number; layout?: string },
  ): Promise<number | null> {
    return runLocalFlagWrite('read', async () => {
      const inv = await getInvoke();
      return inv
        ? (await inv('mark_all_read', {
            feedId,
            folderId,
            starredOnly: opts?.starredOnly ?? false,
            sinceMs: opts?.sinceMs ?? null,
            layout: opts?.layout ?? null,
          }) as number)
        : null;
    });
  },
  async refreshAllFeeds(): Promise<RefreshSummary | null> {
    const inv = await getInvoke();
    return inv ? (await inv('refresh_all_feeds') as RefreshSummary) : null;
  },
  async getSetting(key: string): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_setting', { key }) as string | null) : null;
  },
  /** 后端事务内合并显式键；返回提交后的完整设置，不以本机缓存整包覆盖远端别键。 */
  async patchAppSettings(patch: Record<string, unknown>): Promise<string> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('patch_app_settings', { patch }) as string;
  },
  setSetting(key: string, value: string): Promise<null> {
    if (key === 'last_playback' && playbackWritesPaused) return Promise.reject(new Error('恢复期间暂停保存播放进度'));
    const task = (async () => {
      const inv = await getInvoke();
      return inv ? await inv('set_setting', { key, value }) as null : null;
    })();
    if (key === 'last_playback') {
      playbackWrites.add(task);
      void task.then(() => playbackWrites.delete(task), () => playbackWrites.delete(task));
    }
    return task;
  },
  /** 创建当前数据库的完整手动快照（不参与自动保留清理）。 */
  async createDatabaseBackup(): Promise<DatabaseBackupInfo> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Tauri 客户端可用');
    return (await inv('create_database_backup')) as DatabaseBackupInfo;
  },
  /** 读取并消费迁移前自动备份失败的一次性警告。 */
  async takeStartupBackupWarning(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('take_startup_backup_warning') as string | null) : null;
  },

  /* ---- 更新检查（OPT-014：出网收口在 Rust，固定目的地，无 URL 入参） ---- */
  /** 检查官方仓库最新发布。失败（含 404/429/5xx/无 tag）抛 AppError 拒绝；
   * 浏览器环境无 IPC → 返回 null（调用方按失败处理）。 */
  async checkForUpdates(): Promise<UpdateCheckResult | null> {
    const inv = await getInvoke();
    return inv ? (await inv('check_for_updates') as UpdateCheckResult) : null;
  },

  /* ---- 分类/订阅源级设置落库（接线后端已有命令） ---- */
  async setFolderCollapsed(id: number, collapsed: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_folder_collapsed', { id, collapsed }) as null) : null;
  },
  /** OPT-011：分组三态单字段 patch——undefined不发送，只写被改字段；两字段独立落库。 */
  async setFolderAiModes(id: number, summary?: FeedAiMode, translate?: FeedAiMode): Promise<null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const args: Record<string, unknown> = { id };
    if (summary !== undefined) args.summary = summary;
    if (translate !== undefined) args.translate = translate;
    return (await inv('set_folder_ai_modes', args) as null);
  },
  /* OPT-017F：旧 bool 双字段包装 setFeedAiFlags 已删（全仓零调用方；OPT-011
     起的唯一真值入口是下方三态 setFeedAiModes）。对应 IPC 命令
     set_feed_ai_flags 保留（旧 bool→显式 on/off 的映射语义在 Rust 侧有单测
     锁定），命令签名不随包装器删除——见
     .agents/notes/implemented/simplification/2026-10-16-零消费入口清理与id解析单点.md。 */
  /** OPT-011：源级 AI 开关三态（新真值入口；只动给出的字段，undefined 不改）。
   * 与后端旧 bool 入口（set_feed_ai_flags，命令保留）的边界一致：只写本地，
   * 不推远端。 */
  async setFeedAiModes(id: number, summaryMode?: FeedAiMode, translateMode?: FeedAiMode): Promise<null> {
    const inv = await getInvoke();
    return inv
      ? (await inv('set_feed_ai_modes', {
          id,
          summaryMode: summaryMode ?? null,
          translateMode: translateMode ?? null,
        }) as null)
      : null;
  },

  /* ---- AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi） ---- */
  async getAiConfig(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_ai_config') as string | null) : null;
  },
  async saveAiConfig(value: string): Promise<null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const result = await inv('save_ai_config', { value }) as null;
    aiCacheInvalidated?.();
    return result;
  },
  /** 连通性测试 + 拉模型列表；失败抛错（UI 层 toast） */
  async aiListModels(baseUrl: string, apiKey: string): Promise<string[] | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const raw = (await inv('ai_list_models', { baseUrl, apiKey })) as unknown;
    if (Array.isArray(raw)) return raw as string[];
    throw new Error(String(raw));
  },
  /** 流式 AI 事件（channel 推送）
   *
   * TASK-070（REQ-104）：后端 `AiEvent::Error` 变体已删除（生产从不构造，
   * 失败经 invoke rejection 与调用方 .catch 传达）。此处 `'error'` 分支保留为
   * **防御性兜底**：channel 收到未知/畸形 type 时仍能报错而不是静默丢弃；
   * 不再有任何 Rust 侧路径会发出它。 */
  async aiSummarize(
    articleId: number,
    onDelta: (text: string) => void,
    onDone: () => void,
    onError: (msg: string) => void,
    canStart?: () => boolean,
  ): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const { Channel } = await import('@tauri-apps/api/core');
    const channel = new Channel<Record<string, unknown>>();
    channel.onmessage = (msg) => {
      const m = msg as { type?: string; data?: unknown };
      if (m.type === 'delta') onDelta(String(m.data ?? ''));
      else if (m.type === 'done') onDone();
      else if (m.type === 'error') onError(String(m.data ?? 'AI 错误')); // 防御性兜底，见上方说明
    };
    if (canStart && !canStart()) return null;
    return (await inv('ai_summarize', { articleId, onChannel: channel })) as string;
  },
  async aiTranslate(
    articleId: number,
    onDelta: (text: string) => void,
    onDone: () => void,
    onError: (msg: string) => void,
    canStart?: () => boolean,
  ): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const { Channel } = await import('@tauri-apps/api/core');
    const channel = new Channel<Record<string, unknown>>();
    channel.onmessage = (msg) => {
      const m = msg as { type?: string; data?: unknown };
      if (m.type === 'delta') onDelta(String(m.data ?? ''));
      else if (m.type === 'done') onDone();
      else if (m.type === 'error') onError(String(m.data ?? 'AI 错误'));
    };
    if (canStart && !canStart()) return null;
    return (await inv('ai_translate', { articleId, onChannel: channel })) as string;
  },

  /** OPT-013 B1：AI 缓存签名校验（后端权威结论）。
   *
   *  签名输入（模型/endpoint/提示词/正文哈希/修订）只有后端在同一份配置快照下
   *  能算——前端不再自行以「产物非空」短路，命中判定在动作时刻经本命令取回：
   *  对应项 true = 产物非空且签名匹配当前输入，可复用展示；false = 未生成、
   *  无签名（存量）或已过期（换模型/提示词/正文修订）→ 走重新生成。
   *  浏览器环境无 IPC → null（调用方按「无后端可校验」处理）。
   *
   *  Note: 命中判定/代次/在途契约 — 见
   *  .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md（OPT-013 B1 追加）。 */
  async aiCacheCheck(
    articleId: number,
  ): Promise<AiCacheCheckResult | null> {
    const inv = await getInvoke();
    return inv
      ? (await inv('ai_cache_check', { articleId }) as AiCacheCheckResult)
      : null;
  },

  /** OPT-013 B3：按「条目 × 产物」清除 AI 缓存（真实消费者「清除」入口的后端接线）。
   *
   *  product = 'summary' | 'translation'（与后端 AiKind::id 的签名字符串域逐字一致）。
   *  后端在单事务内：清产物列与签名列、`ai_epoch + 1`（在途迟到提交被拒）、删除该
   *  条目该产物在**所有签名**下的持久化片段——清除后「继续」零恢复，且不可能再
   *  显示 partial。条目不存在/取值非法 → AppError（前端如实 toast，不清前端状态）。
   *  浏览器环境无 IPC → null（调用方按不可用处理）。 */
  async aiClearCache(
    articleId: number,
    product: 'summary' | 'translation',
  ): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('ai_clear_cache', { articleId, product }) as null) : null;
  },

  /** 全文提取（Readability）：拉原文网页抽正文并覆盖缓存。
   * TASK-076：返回结构化结果而非裸字符串——degraded=true 表示本次**没有采用**提取
   * 结果（html 是保留的原文），reason 给出原因；调用方不必再靠比对字符串去猜。 */
  async extractFulltext(articleId: number): Promise<ExtractFulltextResult | null> {
    const inv = await getInvoke();
    return inv ? (await inv('extract_fulltext', { articleId }) as ExtractFulltextResult) : null;
  },

  /** Raw IPC normally returns ArrayBuffer; Tauri's blocked-protocol fallback can return number[].
   * Registration is acknowledged before HTTP begins, so cancellation before registration is not lost. */
  async fetchImage(url: string, pageUrl: string | undefined, requestId: string,
    registered: () => void, canStart: () => boolean): Promise<Uint8Array<ArrayBuffer> | null> {
    const inv = await getInvoke();
    if (!inv || !canStart()) return null;
    const { Channel } = await import('@tauri-apps/api/core');
    if (!canStart()) return null;
    const started = new Channel<{ registered: boolean }>();
    started.onmessage = (event) => { if (event.registered === true) registered(); };
    return imageBytes(await inv('fetch_image', { url, pageUrl, requestId, started }));
  },
  async cancelImage(requestId: string): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('cancel_image', { requestId });
  },

  /** Clear only the matching local cover URL, for any source. The caller requires permanent
   * HTTP evidence and a current cover owner; this never pushes a cover deletion remotely. */
  async reportBrokenCover(articleId: number, url: string): Promise<boolean | null> {
    const inv = await getInvoke();
    return inv ? (await inv('report_broken_cover', { articleId, url }) as boolean) : null;
  },
  /* ---- SMTC 系统媒体控制 ---- */
  /** 播放状态/元数据同步到系统媒体控制（PlayerBar 节流调用）。 */
  async mediaUpdateFull(title: string, show: string, durationSec: number, positionSec: number, playing: boolean): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('media_update_full', { title, show, durationSec, positionSec, playing });
  },

  /** 播放结束/关闭播放条：SMTC 置 Stopped。 */
  async mediaStop(): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('media_stop');
  },

  /* ---- 本地资料库持续同步 / 便携备份 v4 ---- */
  async localSyncStatus(): Promise<LocalSyncStatus | null> {
    const inv = await getInvoke();
    return inv ? await inv('local_sync_status') as LocalSyncStatus : null;
  },
  async localSyncConnect(config: LocalSyncConnection): Promise<LocalSyncStatus> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('local_sync_connect', { ...config }) as LocalSyncStatus;
  },
  async localSyncDisconnect(): Promise<LocalSyncStatus> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('local_sync_disconnect') as LocalSyncStatus;
  },
  async localSyncRun(): Promise<LocalSyncStatus> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('local_sync_run') as LocalSyncStatus;
  },
  async choosePortableBackupFile(save: boolean): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('choose_portable_backup_file', { save }) as string | null;
  },
  async exportPortableBackup(path: string): Promise<PortableBackupInfo> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('export_portable_backup', { path }) as PortableBackupInfo;
  },
  async previewPortableBackup(path: string): Promise<PortableBackupPreview> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    return await inv('preview_portable_backup', { path }) as PortableBackupPreview;
  },
  async restorePortableBackup(path: string, expectedDigest: string): Promise<PortableBackupInfo> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Windows 客户端可用');
    const result = await inv('restore_portable_backup', { path, expectedDigest }) as PortableBackupInfo;
    aiCacheInvalidated?.();
    return result;
  },

  /* ---- OPML 导入导出 ---- */
  /** 导入 OPML 内容（字符串）。返回 (新增, 跳过)。 */
  async opmlImport(content: string): Promise<{ imported: number; skipped: number } | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    return (await inv('opml_import', { content })) as { imported: number; skipped: number };
  },
  /** 导出全部订阅为 OPML 字符串。 */
  async opmlExport(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('opml_export') as string) : null;
  },

  /* ---- 后端同步（Google Reader / Fever） ---- */
  /** 返回 null = 浏览器环境（mock 模式） */
  /** 轻量连通测试（按协议分派 ClientLogin / Fever api_key），不落库不做同步 */
  async syncTest(protocol: string, endpoint: string, username: string, password: string): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_test', { protocol, endpoint, username, password }) as string) : null;
  },
  /** 保存凭据（先测试，失败不保存）。重活（拉订阅/同步状态）由前端随后台阶段执行。
   * 返回 JSON：{ message, firstConnect, unboundLocalFeeds }——首连且本地有
   * 未绑定源时前端弹「同步本地订阅」确认框。
   * OPT-006 R1：`activatePendingVersion` 仅在**专用激活**配置导入建议时传入
   * （值取自 syncPendingConnection().version）；普通保存不传 = 不消费任何建议。
   * 后端在 HTTP 前与提交事务内两次做版本/身份校验（CAS），不匹配整体拒绝。 */
  async syncSave(protocol: string, endpoint: string, username: string, password: string, activatePendingVersion?: number): Promise<SyncSaveResult | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const args: Record<string, unknown> = { protocol, endpoint, username, password };
    if (activatePendingVersion !== undefined) {
      args.activatePendingVersion = activatePendingVersion;
    }
    const raw = (await inv('sync_save', args)) as string;
    try {
      return JSON.parse(raw) as SyncSaveResult;
    } catch {
      // 兼容旧纯文本消息格式
      return { message: raw, firstConnect: false, unboundLocalFeeds: 0 };
    }
  },
  /** 把本地直连订阅（未绑定的）推送到后端。幂等：已绑定的跳过、
   * 服务端已存在回查绑定不报错。返回结构化结果，部分失败保留错误列表。
   * `feedId`/`feedUrl` 用于 AddFeed 的定向同步：只处理本次新增订阅；
   * 两者都省略时才走手工「同步本地订阅」的全量语义。 */
  async syncLocalFeeds(filter: { feedId?: number; feedUrl?: string } = {}): Promise<SyncLocalFeedsResult | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const args: Record<string, unknown> = {};
    if (filter.feedId != null) args.feedId = filter.feedId;
    if (Object.prototype.hasOwnProperty.call(filter, 'feedUrl')) {
      const url = filter.feedUrl?.trim();
      if (!url) {
        throw new Error('同步目标 feedUrl 不能为空（空 selector 不得回退为全量同步）');
      }
      args.feedUrl = url;
    }
    const raw = await inv('sync_local_feeds', args) as unknown;
    const structuredResult = (value: unknown): SyncLocalFeedsResult | null => (
      value != null
      && typeof value === 'object'
      && typeof (value as { ok?: unknown }).ok === 'boolean'
        ? value as SyncLocalFeedsResult
        : null
    );
    const unknownResult = (detail: string): SyncLocalFeedsResult => ({
      ok: false,
      protocol: null,
      writeBack: false,
      queued: 0,
      pushed: 0,
      failed: 0,
      errors: [detail],
      message: `旧版同步服务返回了非结构化回执，无法确认同步结果：${detail}`,
      structured: false,
    });
    if (typeof raw !== 'string') {
      const direct = structuredResult(raw);
      if (direct) return direct;
      let detail: string;
      try {
        detail = JSON.stringify(raw) ?? String(raw);
      } catch {
        detail = String(raw);
      }
      return unknownResult(detail);
    }
    /* 兼容旧后端字符串回执：JSON 可解析则按结构化结果；纯文本无法证明成功，
       按 unknown 返回并提示，绝不折叠成 ok:true。 */
    try {
      const parsed = JSON.parse(raw) as unknown;
      const structured = structuredResult(parsed);
      if (structured) return structured;
      throw new Error('non-structured');
    } catch {
      return unknownResult(raw);
    }
  },
  /** 分步同步：which='feeds'（订阅层）| 'states'（状态+条目层）。full=true 含全量对账 */
  async syncPhase(which: 'feeds' | 'states', full = false): Promise<SyncReport | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_phase', { which, full }) as SyncReport) : null;
  },
  async syncRun(): Promise<SyncRunResult | null> {
    const inv = await getInvoke();
    return inv ? await inv('sync_run') as SyncRunResult : null;
  },
  async syncRetryExpired(expectedAccountGeneration: number): Promise<ExpiredRetryResult | null> {
    const inv = await getInvoke();
    return inv ? await inv('sync_retry_expired', { expectedAccountGeneration }) as ExpiredRetryResult : null;
  },
  async syncDisconnect(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_disconnect') as string) : null;
  },
  /** 缓存清理：days 天前「已读且未收藏、无待同步」的文章，或 AI 缓存。
   * 未读文章永不清理；远端仍存在的已读文章可能在下次同步后重新导入。 */
  async cacheCleanup(days: number, scope: 'articles' | 'ai'): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const result = await inv('cache_cleanup', { days, scope }) as string;
    if (scope === 'ai') aiCacheInvalidated?.();
    return result;
  },
  /** 首次关闭询问的应答：action='tray'|'exit'；remember=true 持久化选择 */
  async resolveClose(action: 'tray' | 'exit', remember: boolean): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('resolve_close', { action, remember });
  },
  async syncStatus(): Promise<SyncStatusInfo | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_status') as SyncStatusInfo) : null;
  },
  /** 配置导入的待确认连接建议（OPT-006）：只读展示；激活走 syncSave
   * （需重新输入密码），放弃走 syncDismissPendingConnection。浏览器环境返回 null。 */
  async syncPendingConnection(): Promise<PendingConnection | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_pending_connection') as PendingConnection | null) : null;
  },
  /** 放弃待确认连接建议（不触碰活动凭据/绑定）。 */
  async syncDismissPendingConnection(): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('sync_dismiss_pending_connection');
  },
  /** 同步队列统计（TASK-116 四态展示：等待/部分失败 + 最近错误摘要）。
   * 浏览器环境返回 null */
  async syncQueueStats(): Promise<SyncQueueStats | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_queue_stats') as SyncQueueStats) : null;
  },
  /** 订阅写回能力矩阵（OPT-009 B3 只读 IPC；设置页能力卡与确认弹窗远端语义
   *  句的单一事实源）。浏览器环境返回 null */
  async subscriptionCapabilities(): Promise<SubscriptionCapabilities | null> {
    const inv = await getInvoke();
    return inv ? (await inv('subscription_capabilities') as SubscriptionCapabilities) : null;
  },
  /** 订阅写回回执（OPT-009 B3 只读 IPC；当前账号限定的 pending/failed/
   *  local_only/最近错误——异账号暂停项不混入）。浏览器环境返回 null */
  async subscriptionOutboxStats(): Promise<SubscriptionOutboxStats | null> {
    const inv = await getInvoke();
    return inv ? (await inv('subscription_outbox_stats') as SubscriptionOutboxStats) : null;
  },
};

/* ============================================================
   行类型 → 前端模型适配（snake_case → 前端字段语义）
   ============================================================ */

/** 时间字符串 → unix ms（无时间时取 0，排序仍稳定）。
 *  TASK-100 D30（R-P3-7 前端半边）：SQLite 存量数据中的 `YYYY-MM-DD HH:MM:SS`
 *  （空格分隔、无时区标记）此前交给 Date.parse 按本地时区解读，UTC 存储的时间
 *  会偏移一个时区差（排序/「今天」边界随之漂移）。旧格式一律按 UTC 解读——
 *  Rust 代理正在加 v15 迁移归一存量数据，前端兼容旧格式是双保险；
 *  标准 ISO 形态（T 分隔 / 带 Z）仍走 Date.parse。 */
export function parseTs(iso: string | null): number {
  if (!iso) return 0;
  const m = /^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2})/.exec(iso);
  if (m) {
    return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]), Number(m[6]));
  }
  const t = Date.parse(iso);
  return Number.isNaN(t) ? 0 : t;
}

export function feedRowToItem(row: FeedRow): FeedItem {
  return {
    id: String(row.id),
    syncId: row.sync_id,
    subscriptionState: row.subscription_state,
    effectiveLayout: row.effective_layout === 'inherit' ? undefined : row.effective_layout,
    name: row.title,
    url: row.feed_url,
    favicon: row.favicon_url ?? '',
    // 后端列受 schema 约束（'inherit' | 五布局），此处窄化到前端联合类型
    layout: row.layout as FeedItem['layout'],
    autoSummary: row.auto_summary,
    autoTranslate: row.auto_translate,
    /* mode是唯一真值；未指定时继承，布局默认关闭，不消费旧bool。 */
    summaryMode: normalizeFeedAiMode(row.summary_mode),
    translateMode: normalizeFeedAiMode(row.translate_mode),
    fetchFailed: row.fetch_failed,
  };
}

export function folderRowsToCategories(folders: FolderRow[], feeds: FeedRow[]): CategoryGroup[] {
  return folders.map((f) => {
    const own = feeds.filter((x) => x.folder_id === f.id);
    return {
      id: `cat-${f.id}`,
      name: f.name,
      collapsed: f.collapsed,
      settingsCollapsed: false,
      layout: f.layout as CategoryGroup['layout'],
      autoSummary: f.auto_summary,
      autoTranslate: f.auto_translate,
      summaryMode: normalizeFeedAiMode(f.summary_mode),
      translateMode: normalizeFeedAiMode(f.translate_mode),
      feeds: own.map(feedRowToItem),
    };
  });
}

export function articleRowToEntry(row: ArticleListItemRow): ArticleEntry {
  return {
    id: String(row.id),
    feedId: String(row.feed_id),
    title: row.title,
    publishedAt: parseTs(row.published_at),
    firstSeenAt: row.first_seen_at ? parseTs(row.first_seen_at) : undefined,
    publishedAtSource: row.published_at_source ?? 'legacy_unknown',
    isRead: row.is_read,
    isStarred: row.is_starred,
    retained: row.retained,
    bodyMissing: row.body_missing,
    imageCount: row.image_count,
    mediaRevision: row.media_revision,
    tags: [],
    source: row.source as ArticleEntry['source'],
    snippet: row.snippet,
    author: row.author ?? '',
    cover: row.image_url ?? undefined,
    imageUrl: row.gallery_first_image_url ?? undefined,
    audioUrl: row.enclosure_url ?? undefined,
    enclosureUrl: row.enclosure_url ?? undefined,
    durationSec: row.duration_sec ?? undefined,
    url: row.url ?? undefined,
    urlKnown: row.url !== undefined,
    aiSummary: row.ai_summary ?? '',
    /* TASK-122：content/rawContent 不再映射进视图行（正文真值源 bodyById；
       with_content 恒 false，列表行的 content_html 恒 NULL，映射本就是空转）。
       translatedContent/aiSummary/fulltextExtracted 保留：列表行携带的 DB 列
       真值，未水合卡片的即时显示源（过渡兼容位，见 ArticleEntry 注释）。 */
    translatedContent: row.translated_content ?? '',
    fulltextExtracted: row.fulltext_extracted ?? false,
    /* OPT-008B 第二阶段：内容身份/修订进视图行——前端失效判定依据
       （列表快照合并与时序水合守卫消费，见 internals/bodyCache）。 */
    contentId: row.content_id ?? null,
    contentRevision: row.content_revision ?? 0,
    /* OPT-013 B1/B3：AI 产物缓存签名进视图行（null/undefined = 旧无签名产物——
       五态判定（slices/ai.ts）据此呈「已过期」；命中判定同样在 store/slices/ai.ts
       经后端 ai_cache_check 完成，前端不自算签名）。 */
    aiSummarySig: row.ai_summary_sig ?? null,
    translatedSig: row.translated_sig ?? null,
  };
}


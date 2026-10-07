import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { applyBodyRow, applyExtractedFulltext, dropBodyEntry, getBodyEntry, markBodyFailed, markBodyLoading, markBodyMissing } from '../bodyCache';
import type { EntryRollbackClaim } from '../internals';
import { appStore, getEntryVersion, markEntriesRead, optimisticEntryFlagToggle, rollbackEntryClaims, syncCurrentViewCache } from '../internals';
import type { AppState } from '../types';

/** 阅读器 slice：选中文章、正文水合（懒加载 + 批量合批）与卡片就地标读/收藏。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type ReaderSlice = Pick<
  AppState,
  | 'activeArticleId'
  | 'isShowingTranslatedProse'
  | 'isRawRenderMode'
  | 'showFulltext'
  | 'bodyCacheNonce'
  | 'openedReadIds'
  | 'selectArticle'
  | 'ensureArticleContent'
  | 'retryHydration'
  | 'hydrateArticleContent'
  | 'clearReaderSelection'
  | 'readerCloseNonce'
  | 'extractCurrentArticle'
  | 'toggleReaderFulltext'
  | 'toggleCurrentReadStatus'
  | 'toggleCurrentStar'
  | 'toggleReaderRenderMode'
  | 'markEntriesReadBulk'
  | 'toggleEntryFlag'
>;

/* ============================================================
   正文水合批量队列 —— 首屏/布局切换时几十张可见卡片同帧触发 ensureArticleContent，
   逐篇 getArticle（各一次 IPC + 各一次 set + 全量 selector 重算）是「加载正文
   几秒 + 切换卡顿」的根因。这里把同一帧内的请求合批：微任务 flush 成一次
   get_articles IPC + 一次 set。 */
let hydrationQueue: Set<string> | null = null;
let hydrationFlushScheduled = false;

function enqueueHydration(id: string) {
  if (!hydrationQueue) hydrationQueue = new Set();
  hydrationQueue.add(id);
  if (hydrationFlushScheduled) return;
  hydrationFlushScheduled = true;
  /* 微任务：等当前同步帧内所有卡片都入队后一次性 flush */
  Promise.resolve().then(() => {
    hydrationFlushScheduled = false;
    const q = hydrationQueue;
    hydrationQueue = null;
    if (!q || q.size === 0) return;
    appStore().getState().hydrateArticleContent(Array.from(q));
  });
}

/* TASK-103：在途水合去重 → TASK-122：由 bodyById 的 loading 记录承载——
   同 id 在途不重复 IPC 的判定从模块级 Set 收敛为记录状态（无记录才入队；
   loading 记录存在即在途）。useLazyHydrate 的条件重触发（快照替换让水合前提
   重新成立）与虚拟列表的重挂载都会在请求未落地时再次入队；loading 态让
   entryNeedsHydration 为假，重复入队被守卫拦下（原 hydrationInFlight 的
   跨帧窗口与 enqueueHydration 的同帧 Set 去重语义都由此承接）。 */

/** 「智能全文」判定：正文是否已是全文（无需 Readability 提取）。
    启发式：只认明确的"正文被截断"信号——摘要型源（少数派等）的结尾标记
    "查看全文/阅读全文/继续阅读/阅读原文" 等。返回 true = 需要提取全文（是摘要）；
    false = 已是全文（跳过，省请求）。
    注意：不按"文本短"判断——论坛（Linux DO）的短帖本身就是完整全文，短 ≠ 摘要；
    也不认"阅读更多"——那是论坛/列表"去原帖看回复"链接，非正文截断。 */
function shouldExtractFulltext(html: string): boolean {
  if (!html) return false;
  const text = html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  // 明确的正文截断标记（"全文/原文"字样；不含"阅读更多"= 论坛去原帖链接）
  const truncationMarks = /查看全文|阅读全文|继续阅读|展开全文|阅读原文|查看原文|Read more|read more|Continue reading|continue reading/;
  return truncationMarks.test(text);
}

export const createReaderSlice: StateCreator<AppState, [], [], ReaderSlice> = (set, get) => ({
  activeArticleId: null,
  isShowingTranslatedProse: false,
  isRawRenderMode: false,
  showFulltext: false,
  /* TASK-122：bodyById 写入通知序号（真实写入由 store.ts 注入的 notify 回调
     bump；本 slice 只持初值。语义见 types.ts bodyCacheNonce 注释） */
  bodyCacheNonce: 0,
  openedReadIds: {},
  /* TASK-115②：阅读器关闭信号（nonce，不透明计数器；字段说明见 types.ts）。
     仅在 clearReaderSelection bump——本 action 只发信号，不携带载荷：原选中卡
     id 由 Timeline 在 activeArticleId 跟随 effect 用 ref 记账（关闭 commit 时
     该 ref 保留关闭前值），Timeline 的 readerClose effect 消费本信号归还焦点。
     选型理由见下方 clearReaderSelection 长注释与 timelineAnchor.ts 头注 X2 节。 */
  readerCloseNonce: 0,

  /* ================= 阅读器 ================= */

  selectArticle: (id) => {
    const { entries, settings, dataMode } = get();
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    const shouldMarkRead = dataMode === 'tauri' && settings.markReadOnOpen && !art.isRead;
    if (shouldMarkRead) {
      /* 后端模式：已读落库（不重载快照，本地同步置位即可） */
      /* TASK-067 N10：标读失败对用户可见（此前静默，重启后回退未读）。
         TASK-118（审计相邻缺口）：乐观置位（markEntriesRead）后快照 isRead
         字段版本，失败走统一回滚助手 rollbackEntryClaims——与单条 toggle /
         批量标读同一份实现（恢复读态 + 逐 feed 回补 unread，版本守卫：期间
         已被接管则跳过）。
         边界裁定：失败时文章刚被选中阅读——只回滚读态与计数，不清
         activeArticleId / openedReadIds（用户还在读；「打开过」的保留标记
         使卡片在未读筛选下原地变灰而非消失）。 */
      markEntriesRead(new Set([id]));
      const claim: EntryRollbackClaim = { id, field: 'isRead', prev: false, version: getEntryVersion(id, 'isRead') };
      void api.setRead(Number(id), true).catch((e) => {
        rollbackEntryClaims([claim]);
        get().showToast(`标读失败：${extractError(e)}`);
      });
    }
    set((s) => ({
      /* 记录"本次会话中被打开过"：即使标已读，在未读筛选下也保留显示（原地变灰） */
      openedReadIds: { ...s.openedReadIds, [id]: true },
      activeArticleId: id,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
    }));
    /* 打开文章：触发「智能全文」判定（摘要型源才提取原文；列表卡片水合不触发） */
    get().ensureArticleContent(id, { extractFulltext: true });
  },

  /** 正文懒加载（幂等）：列表快照不含 HTML，选中/社交卡片挂载时拉详情水合。
      守卫用「条目仍在」（不依赖 activeArticleId：社交卡片挂载时也走这里）。
      实现：并入批量水合队列（微任务合批）——首屏几十张可见卡片同帧触发时，
      合并成一次 get_articles IPC + 一次 set，避免逐篇 IPC 洪峰与逐篇 O(n) 重渲染。
      注意：extractFulltext（打开文章的智能全文）即使正文已水合也须执行——
      列表卡片批量水合只填 content，不触发智能全文判定；打开文章时若 bodyById
      已 ready（列表水合过），仍要走 extractFulltext 分支。
      TASK-122：水合真值落 bodyById（记录 state：loading→ready/missing/failed），
      视图行只同步 snippet/url 轻字段；幂等守卫从「art.content || hydratedIds」
      改为「bodyById 已有记录」（loading=在途、ready/cleared/missing/failed=终态，
      失败只能经 retryHydration 删记录后重入队）。 */
  ensureArticleContent: (id, opts) => {
    const { dataMode, entries } = get();
    if (dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    if (opts?.extractFulltext) {
      /* 打开文章：需要完整详情（含 url/fulltext_extracted）+ 智能全文判定。
         若已水合（列表卡片批量水合过），content 已有，但仍需取详情以判断
         是否要提取全文——故不因记录存在而短路。
         TASK-122：发起前签失效戳（记录现 revision）；期间发生显式失效
         （清理 AI 缓存 / 再次生成）则迟到的详情响应整体丢弃。 */
      const stamp = markBodyLoading(id);
      void api.getArticle(Number(id)).then((row) => {
        if (!row) {
          /* 详情缺行 = 文章不存在（旧实现静默 return，记录卡 loading；TASK-122
             终态化：missing 呈现「文章不存在」，可经 retryHydration 幂等重试） */
          markBodyMissing(id, stamp);
          return;
        }
        const cur = get().entries.find((a) => a.id === id);
        /* TASK-122：详情落 bodyById（正文/AI/译文真值）；AI 列 NULL 时保留现值
           （流式半截/未落库产物——与旧 `row.x ?? a.x` 回退逐字同语义）。
           生成中传视图行值做回退（记录在途时其 AI 为空，不能当回退源）。
           条目已被快照替换移除也照常落记录（实体缓存语义，见批量路径同款裁定）；
           仅视图行轻字段同步跳过。 */
        const generating = !!(get().summarizingIds[id] || get().translatingIds[id]);
        const rec = getBodyEntry(id);
        applyBodyRow(id, row, stamp, {
          aiSummary: generating ? (cur?.aiSummary || '') : (rec?.aiSummary ?? cur?.aiSummary ?? ''),
          translatedContent: generating ? (cur?.translatedContent || '') : (rec?.translatedContent ?? cur?.translatedContent ?? ''),
        });
        if (!cur) return;
        /* 视图行轻字段照旧同步：snippet/url（列表行可能缺 url）；无变化不 set
           （保持 entries 引用稳定，避免无谓重渲染） */
        const nextUrl = cur.url ?? row.url ?? undefined;
        const nextSnippet = row.snippet || cur.snippet;
        if (nextUrl !== cur.url || nextSnippet !== cur.snippet) {
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id ? { ...a, url: nextUrl, snippet: nextSnippet } : a,
            ),
          }));
        }
        const mode = get().settings.defaultOpenMode;
        const alreadyExtracted = row.fulltext_extracted ?? false;
        if (mode === 'fulltext' && row.url && !alreadyExtracted && shouldExtractFulltext(row.content_html ?? '')) {
          void api
            .extractFulltext(Number(id))
            .then((res) => {
              if (!res) return;
              /* TASK-076（P2-10 后半，DEC-req104-p2-10b-fulltext-degraded-20260920）：
                 后端现在直接给结构化 degraded 标志，取代此前「返回内容 == 当前正文」
                 的字符串比对——那种猜法在正文恰好相同时会误判，而且静默、用户看不到
                 原因。降级时如实提示并保持原状（不置标志、不切全文视图）。 */
              if (res.degraded) {
                get().showToast(
                  `未采用全文提取：${res.reason ?? '提取结果不可用'}`,
                  { label: '重试', run: () => get().extractCurrentArticle() },
                );
                return;
              }
              applyExtractedFulltext(id, res.html);
              set({ showFulltext: true });
            })
            .catch((e: unknown) => {
              const msg = extractError(e);
              get().showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
            });
        }
      }).catch((e: unknown) => {
        /* 打开文章的详情拉取失败：Reader 不能静默空白（REQ-001 排查 P1-8）。
           TASK-122：失败落 bodyById failed 态（卡片内联重试入口可用） */
        const msg = extractError(e);
        markBodyFailed(id, stamp, msg);
        get().showToast(`正文加载失败：${msg}`, { label: '重试', run: () => get().ensureArticleContent(id, { extractFulltext: true }) });
      });
      return;
    }
    /* 列表卡片（社交/通知）水合：bodyById 已有记录（含空正文终态 loading/ready/
       cleared/missing/failed）则短路，否则并入批量队列。不能只判正文有无——
       content_html 为 NULL 的条目水合后 content 仍为空串，仅按内容判定会让它
       每次挂载都重新入队（重复 IPC 洪峰 + 永挂「加载正文…」）。 */
    if (getBodyEntry(id)) return;
    enqueueHydration(id);
  },

  /** 水合失败重试：删记录回「未请求」态后重新入队（卡片内联重试入口）。
      TASK-122：missing/failed 都是可重试终态（重试幂等：missing 重试后仍是
      missing），与旧「清 hydrationErrors 后入队」同语义。 */
  retryHydration: (id) => {
    dropBodyEntry(id);
    enqueueHydration(id);
  },

  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰）。
      TASK-103 终态机 → TASK-122：状态收敛为 bodyById 记录的判别态
      （loading → ready / missing / failed，见 bodyCache 模块头注状态机）：
      - 命中行 → applyBodyRow 写 ready（正文/AI/译文落记录；视图行同步
        snippet/url 轻字段；AI 列 NULL 时保留现值——流式产物不丢）；
      - 响应中缺行的 id → markBodyMissing「文章不存在」终态（卡片呈现明确
        文案，不得静默留加载占位）；空 rows 即整批不存在，同口径；
      - 请求失败 → markBodyFailed（retryHydration 内联重试入口保留）；
      - 应用与终态标记都按**当前 store 状态**逐 id 复核：请求在途期间条目可能
        已被快照替换移除、或已经他路水合（selectArticle 详情拉取）——迟到的
        旧响应不得覆盖新状态。这是与 reloadGeneration 同目标的乱序防护，用
        「按 id 现态复核」而非整批代际号：滚动时并发多批是常态，整批代际会把
        旧批的有效行一并丢弃、卡片反而回到无请求死区；在途去重（loading
        记录）已保证同 id 同时至多一个请求在途。 */
  hydrateArticleContent: (ids) => {
    if (get().dataMode !== 'tauri') return;
    /* 过滤出「仍存在且无记录」的 id（幂等 + 去重）：loading=在途、
       ready/cleared/missing/failed=终态，都不入队（TASK-122 判定改从 bodyById） */
    const pending = ids.filter((id) => {
      if (getBodyEntry(id)) return false;
      return get().entries.some((e) => e.id === id);
    });
    if (pending.length === 0) return;
    /* 发起即置 loading（签发失效戳）：同帧重复入队与跨帧重复 IPC 都被
       「记录存在」拦下；响应落地按 stamp 比对丢弃失效后的迟到响应 */
    const stamps = new Map<string, number>(pending.map((id) => [id, markBodyLoading(id)] as const));
    const pendingSet = new Set(pending);
    void api.getArticles(pending.map(Number)).then((rows) => {
      /* 构建 id → 详情 映射，一次性合并（单次 map，单次 set） */
      const byId = new Map((rows ?? []).map((r) => [String(r.id), r] as const));
      /* TASK-122：视图行侧只同步轻字段（snippet/url）；正文/AI 落 bodyById */
      set((s) => {
        let changed = false;
        const entries = s.entries.map((a) => {
          if (!pendingSet.has(a.id)) return a;
          const row = byId.get(a.id);
          if (!row) return a;
          const nextUrl = a.url ?? row.url ?? undefined;
          const nextSnippet = row.snippet || a.snippet;
          if (nextUrl === a.url && nextSnippet === a.snippet) return a;
          changed = true;
          return { ...a, url: nextUrl, snippet: nextSnippet };
        });
        if (changed) syncCurrentViewCache(entries);
        return changed ? { entries } : {};
      });
      /* 正文/AI 落记录：逐 id 现态复核——只写仍是 **loading** 的记录（本批的
         在途标记）：他路已水合（selectArticle 详情先落地 → ready）、显式失效
         （cleared）的 id 一律跳过——迟到的旧批次不得覆盖新状态（TASK-103 语义，
         TASK-122 以记录状态表达）。
         条目已被快照替换移除的 id **照常落记录**（TASK-122 裁定）：记录是文章
         实体缓存，不随视图行存活——行真值是该实体的合法缓存，条目若在后续快照
         重现可直接命中（不产生任何视图污染；旧「滞留 hydratedIds 阻断重水合」
         的危害形态在结构上不存在：内容与终态同体存活，重现即有正文）。
         缺行（missing）/失败（failed）同理按实体落账——响应缺行即 DB 已删，
         与视图是否还在无关；失败落账避免 loading 记录无人收尾的死区。 */
      for (const id of pending) {
        const rec = getBodyEntry(id);
        if (!rec || rec.state !== 'loading') continue;
        const row = byId.get(id);
        if (!row) {
          markBodyMissing(id, stamps.get(id) ?? 0);
          continue;
        }
        const generating = !!(get().summarizingIds[id] || get().translatingIds[id]);
        const entry = get().entries.find((a) => a.id === id);
        applyBodyRow(id, row, stamps.get(id) ?? 0, {
          aiSummary: generating ? (entry?.aiSummary || '') : (rec.aiSummary || entry?.aiSummary || ''),
          translatedContent: generating ? (entry?.translatedContent || '') : (rec.translatedContent || entry?.translatedContent || ''),
        });
      }
    }).catch((e: unknown) => {
      /* 批量水合失败：错误落到对应记录（卡片内联重试），不再静默假加载。
         TASK-103：只对「仍在途（loading）」的 id 落 failed——过期失败不得污染
         已被快照替换/他路水合/显式失效的新状态 */
      const msg = extractError(e);
      for (const id of pending) {
        const rec = getBodyEntry(id);
        if (rec?.state === 'loading') markBodyFailed(id, stamps.get(id) ?? 0, msg);
      }
    });
  },

  /* TASK-115②：关闭阅读器（Esc / App 全局快捷键的唯一关闭路径）。修前只清
     四个字段、无焦点归还——关闭后焦点落空、J/K 从列表顶部重新起步（阅读
     上下文丢失）。修法：bump readerCloseNonce 发出关闭信号，Timeline 消费
     信号把焦点归还原选中卡（原卡不在当前列表 → 归零回落不聚焦）。
     选型（Timeline 消费关闭信号 vs 本 action 记录原 id）：选前者——本 action
     只 bump 不透明计数器，原选中卡由 Timeline 在 activeArticleId 跟随 effect
     里记录（关闭时刻它必然持有最后选中的文章 id）。理由：store 不反向依赖
     components 层（timelineAnchor 在 components/ 下，store→components 是
     倒挂方向，会开循环依赖的口子）；且「只有原 id 字段无法区分连续两次开关
     同一篇」（值不变不触发 effect），nonce 计数器才是可靠的重触发信号。 */
  clearReaderSelection: () =>
    set((s) => ({
      readerCloseNonce: s.readerCloseNonce + 1,
      activeArticleId: null,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
    })),

  /** 手动全文提取：Readability 拉原文网页存 content（TASK-122：落 bodyById）；
      rawContent 始终保留 RSS 原文（供「全文 ↔ RSS 正文」切换回跳）。
      提取成功后进入全文视图。 */
  extractCurrentArticle: () => {
    const { activeArticleId, entries, dataMode, showToast } = get();
    if (!activeArticleId || dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    const rec = getBodyEntry(activeArticleId);
    if (!rec) {
      /* 正文尚未水合（无记录）：先取详情再提取（与打开文章同一详情链路） */
      get().ensureArticleContent(activeArticleId, { extractFulltext: true });
      return;
    }
    if (!art.url) {
      showToast('该条目没有原文链接');
      return;
    }
    showToast(rec.fulltextExtracted ? '正在刷新全文…' : '正在提取全文…');
    void api
      .extractFulltext(Number(activeArticleId))
      .then((res) => {
        if (!res) return;
        const id = activeArticleId;
        /* TASK-076：改判结构化 degraded 标志。此前靠「返回内容 == 当前正文」猜，
           且为了绕开「已提取过的刷新本来就会相同」还要额外判断 fulltextExtracted，
           逻辑脆弱；现在后端直接说明本次是否采用了提取结果，手动刷新与自动全文
           两条路径用同一判据。 */
        if (res.degraded) {
          showToast(`未采用全文提取：${res.reason ?? '提取结果不可用'}`);
          return;
        }
        applyExtractedFulltext(id, res.html);
        set({ showFulltext: true });
        showToast('全文提取完成');
      })
      .catch((e: unknown) => {
        const msg = extractError(e);
        showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
      });
  },

  /** 全文视图切换：已提取全文 → 在 RSS 原文与全文间切换（不重复请求）；
      未提取 → 触发提取（同 extractCurrentArticle）。
      TASK-122：fulltextExtracted 从 bodyById 记录取（真值源迁移）。 */
  toggleReaderFulltext: () => {
    const { activeArticleId, showFulltext } = get();
    if (!activeArticleId) return;
    const rec = getBodyEntry(activeArticleId);
    if (!rec) return;
    if (rec.fulltextExtracted) {
      // 已提取：直接切换视图
      set({ showFulltext: !showFulltext });
    } else {
      // 未提取：触发提取
      get().extractCurrentArticle();
    }
  },

  /* F3（Batch 1/2 独立审查 P3）：阅读器两条切换入口与卡片路径共用
     optimisticEntryFlagToggle——修前这里先弹乐观「已标为已读」再 fire-and-forget，
     失败既不回滚也留着成功提示（与卡片路径删假成功 toast 的口径相反）。
     现在：乐观翻转仍立即生效；成功提示经 onSuccess 只在落库成功后出现
     （P1-5 去假成功同口径）；失败回滚「仅当当前值仍等于乐观值」并给失败 toast
     （文案沿用既有）。mock 模式 request 立即 resolve：本地翻转 + toast、无 IPC，
     与修前一致。 */
  toggleCurrentReadStatus: () => {
    const { activeArticleId, dataMode } = get();
    if (!activeArticleId) return;
    optimisticEntryFlagToggle(
      activeArticleId,
      'isRead',
      (next) => (dataMode === 'tauri' ? api.setRead(Number(activeArticleId), next) : Promise.resolve()),
      (msg) => `标读状态保存失败：${msg}`,
      (next) => get().showToast(next ? '已标为已读' : '已标为未读'),
    );
  },

  toggleCurrentStar: () => {
    const { activeArticleId, dataMode } = get();
    if (!activeArticleId) return;
    optimisticEntryFlagToggle(
      activeArticleId,
      'isStarred',
      (next) => (dataMode === 'tauri' ? api.setStarred(Number(activeArticleId), next) : Promise.resolve()),
      (msg) => `收藏状态保存失败：${msg}`,
    );
  },

  toggleReaderRenderMode: () => set((s) => ({ isRawRenderMode: !s.isRawRenderMode })),

  markEntriesReadBulk: (ids) => {
    if (ids.length === 0) return;
    const { dataMode, entries } = get();
    /* 只处理未读项：已读的重复 setRead 无意义（还会刷 sync_queue）。
       L1：先建一次 id → 条目的索引表，避免对每个 id 都做一次 entries.find
       （O(n·m)）——与 markEntriesRead 的一次遍历口径一致；「全部已读/
       滚动标读」传入几百个 id 时这是可感知的开销。 */
    const byId = new Map(entries.map((e) => [e.id, e] as const));
    const unread = ids.filter((id) => {
      const e = byId.get(id);
      return e ? !e.isRead : false;
    });
    if (unread.length === 0) return;
    const marked = new Set(unread);
    markEntriesRead(marked);
    set((s) => ({
      /* 标读的卡片原地变灰保留（未读筛选下不消失）——合并而非替换：
         批量标读（滚动/全部已读）不能抹掉之前"打开过"的保留记录，
         否则那些卡片会在未读筛选下突然消失（体验为"已读的直接隐藏"）；
         切视图/布局/筛选时的清理逻辑统一把它们移除 */
      openedReadIds: {
        ...s.openedReadIds,
        ...Object.fromEntries(unread.map((id) => [id, true])),
      },
    }));
    if (dataMode === 'tauri') {
      /* AUDIT P3[F4]（TASK-084）：改为**一次**批量 IPC。
         修前是 `Promise.allSettled(unread.map((id) => api.setRead(...)))` —— 每个 id 一次
         invoke，几百个 id 就是几百次往返。Rust 侧 set_read_bulk 在同一把锁内逐 id 走
         record_read_state（本地写入 + 入队口径与逐条路径完全一致），故语义等价。
         失败提示：整批一次（比修前的“部分失败”粒度更粗，但不再有 toast 洪峰，
         且 catch 保证不会产生 unhandled rejection）。
         TASK-118（审计相邻缺口）：乐观置位后快照各 id 的 isRead 字段版本，
         失败走统一回滚助手 rollbackEntryClaims——与单条 toggle / 打开即标读
         同一份实现：按版本守卫恢复被翻转行、逐 feed 回补 unread（Math.max(0,)），
         不再只提示不回滚。openedReadIds 不回撤：「打开过」的原地变灰保留语义
         不随落库失败撤销（与打开即标读回滚的边界裁定同口径）。 */
      const claims: EntryRollbackClaim[] = unread.map((id) => ({ id, field: 'isRead', prev: false, version: getEntryVersion(id, 'isRead') }));
      void api.setReadBulk(unread.map((id) => Number(id)), true).catch(() => {
        rollbackEntryClaims(claims);
        get().showToast('批量标读失败');
      });
    }
  },

  /* ================= 卡片就地操作 ================= */

  toggleEntryFlag: (id, field) => {
    const { dataMode } = get();
    /* F2（Batch 1/2 独立审查 P3）：失败回滚改走共用 helper——「仅当当前值仍等于
       乐观写入值才恢复点击前值」。修前是「再翻一次当前值」：连点两次、第一次
       失败第二次成功时，第一次迟到的 catch 会把第二次已落库的新值踩回旧值
       （探针实测 UI isRead=true / DB is_read=false）。失败 toast 文案沿用既有。 */
    optimisticEntryFlagToggle(
      id,
      field,
      (next) => {
        if (dataMode !== 'tauri') return Promise.resolve();
        return field === 'isRead' ? api.setRead(Number(id), next) : api.setStarred(Number(id), next);
      },
      (msg) => (field === 'isRead' ? `标读保存失败：${msg}` : `收藏保存失败：${msg}`),
    );
  },
});

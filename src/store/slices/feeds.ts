import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import {
  applyCategoryAiFlagPatch,
  applyFeedAiModePatch,
  reconcileCategories,
} from '../internals';
import { issueAiMultiWrite, issueAiWrite } from '../aiConfigWrites';
import { numericId } from '../selectors';
import { normalizeFeedAiMode } from '../../types';
import type { FeedAiMode, FeedItem } from '../../types';
import type { AppState } from '../types';

/** 订阅管理 slice：分类/订阅源的增删改、布局与 AI 开关、折叠状态。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type FeedsSlice = Pick<
  AppState,
  | 'createCategory'
  | 'deleteCategory'
  | 'renameCategory'
  | 'addFeed'
  | 'deleteFeed'
  | 'editFeed'
  | 'refreshOneFeed'
  | 'updateCatLayout'
  | 'updateFeedLayout'
  | 'toggleCatSummary'
  | 'toggleCatTranslate'
  | 'toggleFeedSummary'
  | 'toggleFeedTranslate'
  | 'toggleFolderCollapse'
  | 'toggleAllFolders'
  | 'toggleSettingsCatCollapse'
>;

/** OPT-011：AI 开关写入统一收口到 store/aiConfigWrites 的协议入口——
    四入口只负责「定位实体 + 判无变化 + 组装 apply/request」，待写意图/确认态/
    读取凭据、per-键 wire 队列与失败恢复都在协议一处实现（见其头注）。 */
export const createFeedsSlice: StateCreator<AppState, [], [], FeedsSlice> = (set, get) => ({
  /* ================= 订阅管理 ================= */

  createCategory: (name, layout) => {
    if (get().dataMode === 'tauri') {
      void api
        .createFolder(name, layout)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('创建分类失败'));
      return;
    }
    set((s) => {
      const nextCategories = [
        ...s.categories,
        {
          id: 'cat-' + Date.now(),
          name,
          collapsed: true,
          settingsCollapsed: false,
          layout,
          autoSummary: false,
          autoTranslate: false,
          feeds: [],
        },
      ];
      /* 统一走收口：新分类无订阅源，条目集合不变但解析表重建 */
      return reconcileCategories(s, nextCategories);
    });
    get().showToast(`已创建分类：${name}`);
  },

  deleteCategory: (catId) => {
    if (get().dataMode === 'tauri') {
      const id = Number(catId.replace('cat-', ''));
      /* fix-3（自检 P2-1）：删除当前正浏览的分类 → 范围/选中一并复位（与 mock
         分支同口径）。否则 entries 按已消失的 cat-N 过滤恒为空，时间流停在
         幽灵范围显示空白，须用户自行点别处才能恢复。 */
      if (get().activeFeedFilter === catId) {
        set({ activeFeedFilter: 'all', activeArticleId: null });
      }
      void api
        .deleteFolder(id)
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('删除分类失败'));
      return;
    }
    set((s) => {
      const nextCategories = s.categories.filter((c) => c.id !== catId);
      return {
        ...reconcileCategories(s, nextCategories),
        activeFeedFilter: s.activeFeedFilter === catId ? 'all' : s.activeFeedFilter,
      };
    });
    get().showToast('分类已删除');
  },

  renameCategory: (catId, name) => {
    const trimmed = name.trim();
    if (!trimmed) {
      get().showToast('分类名称不能为空');
      return;
    }
    if (get().dataMode === 'tauri') {
      void api
        .renameFolder(Number(catId.replace('cat-', '')), trimmed)
        .then(() => get().reloadFromBackend())
        .then(() => get().showToast(`分类已改名：${trimmed}`))
        .catch((e: unknown) => {
          const msg = extractError(e);
          get().showToast(`改名失败：${msg}`);
        });
      return;
    }
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, name: trimmed } : c))));
    get().showToast(`分类已改名：${trimmed}`);
  },

  addFeed: (catId, url, title, layout, summaryMode, translateMode, syncToBackend = true) => {
    if (get().dataMode === 'tauri') {
      /* fix-1（自检 P1-1）：提不出数字 id（如全新安装 0 个分类时 AddFeedModal 传
         空串，Number('') === 0）必须传 null——后端 add_feed 对 folder_id=None 有
         「自动落到未分类（不存在则创建）」的兜底契约；传 0 会直接触发
         feeds.folder_id 外键违约，首次使用添加订阅必然失败。 */
      const parsed = Number(catId.replace('cat-', ''));
      const folderId = Number.isFinite(parsed) && parsed > 0 ? parsed : null;
      set({ syncStatus: 'syncing' });
      /* OPT-011：AI 开关以三态 mode 为真值传给后端；旧 bool 参数传 null
         （后端合并规则：mode 优先，缺 mode 才按 bool 映射）。 */
      /* OPT-007-UI 补件：add_feed 已落库后的尾链分两段，语义严格分开——
         ① 添加本身失败（IPC reject）→「添加失败」+ 重试=重新添加（唯一允许
            重新 addFeed 的场景：那次添加根本没发生）；
         ② 添加已落库后的回读/同步失败（首次刷新失败、syncLocalFeeds 失败、
            同步后回读失败）→ 如实说「订阅已添加，同步/刷新失败」+ 重试只重跑
            本段——绝不重新 addFeed（那会再新增一条），也绝不静默吞掉。
         勾选同步且已连接时，syncLocalFeeds 会导入远端订阅/回填本地源身份——
         无论它整体成功还是部分失败，都必须再回读一次本地真实结果：已写入的
         订阅（含失败前已导入的部分）不能不可见。无同步选择/未连接时保持原有
         单次回读，不额外请求。 */
      const finishAdd = async (): Promise<void> => {
        /* ① 首次回读：本地新订阅立即可见（无同步选择/未连接时就是唯一一次）。 */
        try {
          await get().reloadFromBackend();
        } catch (e) {
          set({ syncStatus: 'error' });
          get().showToast(`订阅已添加，刷新失败：${extractError(e)}`, {
            label: '重试',
            run: () => { void finishAdd(); },
          });
          return;
        }
        if (!(syncToBackend && get().syncConnected)) {
          set({ syncStatus: 'synced' });
          get().showToast(`已添加订阅源：${title || url}`);
          return;
        }
        /* ② 勾选同步且已连接：syncLocalFeeds（推送本地直连源、回查绑定；前端
           契约按「可能导入/修改订阅」对待）——成功或部分失败都再回读。 */
        let syncError: string | null = null;
        try {
          await api.syncLocalFeeds();
        } catch (e) {
          syncError = extractError(e);
        }
        let readbackError: string | null = null;
        try {
          await get().reloadFromBackend();
        } catch (e) {
          readbackError = extractError(e);
        }
        if (syncError || readbackError) {
          set({ syncStatus: 'error' });
          const detail = [syncError ? `同步失败：${syncError}` : '', readbackError ? `刷新失败：${readbackError}` : '']
            .filter(Boolean)
            .join('；');
          get().showToast(`订阅已添加，${detail}`, {
            label: '重试',
            run: () => { void finishAdd(); },
          });
          return;
        }
        set({ syncStatus: 'synced' });
        get().showToast(`已添加订阅源：${title || url}`);
      };
      void api
        .addFeed(url, title || null, folderId, layout, null, null, syncToBackend, summaryMode, translateMode)
        .then(finishAdd, (e: unknown) => {
          set({ syncStatus: 'error' });
          const msg = extractError(e);
          get().showToast(`添加失败：${msg}`, {
            label: '重试',
            run: () => get().addFeed(catId, url, title, layout, summaryMode, translateMode, syncToBackend),
          });
        });
      return;
    }
    /* TASK-100 P3-10：无匹配目标分类时不再静默假成功——此前 categories.map
       找不到 catId 时 feed 无处挂载，却仍弹「已添加订阅源」（仅浏览器演示
       模式可达）。 */
    if (!get().categories.some((c) => c.id === catId)) {
      get().showToast('添加失败：目标分类不存在');
      return;
    }
    set((s) => {
      const nextCategories = s.categories.map((c) =>
        c.id === catId
          ? {
              ...c,
              feeds: [
                ...c.feeds,
                {
                  id: 'feed-' + Date.now(),
                  name: title || url,
                  url,
                  favicon: '',
                  layout: layout as 'inherit',
                  /* OPT-011：mock 与 tauri 同语义——旧 bool 位按显式选择映射
                     （on→true；inherit/off→false），mode 是显示真值。 */
                  autoSummary: summaryMode === 'on',
                  autoTranslate: translateMode === 'on',
                  summaryMode,
                  translateMode,
                },
              ],
            }
          : c,
      );
      return reconcileCategories(s, nextCategories);
    });
    get().showToast(`已添加订阅源：${title || url}`);
  },

  deleteFeed: (catId, feedId) => {
    if (get().dataMode === 'tauri') {
      /* fix-3（自检 P2-1）：删除当前正浏览的订阅源 → 范围/选中一并复位
         （与 mock 分支同口径，理由见 deleteCategory 处注释）。 */
      if (get().activeFeedFilter === feedId) {
        set({ activeFeedFilter: 'all', activeArticleId: null });
      }
      void api
        .deleteFeed(Number(feedId))
        .then(() => get().reloadFromBackend())
        .catch(() => get().showToast('删除订阅源失败'));
      return;
    }
    set((s) => {
      const nextCategories = s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.filter((f) => f.id !== feedId) } : c,
      );
      return {
        ...reconcileCategories(s, nextCategories),
        activeFeedFilter: s.activeFeedFilter === feedId ? 'all' : s.activeFeedFilter,
      };
    });
    get().showToast('订阅源已删除');
  },

  editFeed: (feedId, next) => {
    if (get().dataMode === 'tauri') {
      /* 只提交变化字段：标题为空 = 不改名；分类/布局/AI 开关与当前一致则省略。
         OPT-011：AI 开关以三态 mode 提交（真值），不再把 bool 回显当输入。 */
      const binding = get().feedIndex.get(feedId);
      const cur = binding?.feed;
      if (!cur) return;
      const curSummaryMode = normalizeFeedAiMode(cur.summaryMode, cur.autoSummary);
      const curTranslateMode = normalizeFeedAiMode(cur.translateMode, cur.autoTranslate);
      const targetFolderId = Number(next.catId.replace('cat-', ''));
      const args: Parameters<typeof api.updateFeed>[0] = { id: Number(feedId) };
      if (next.title.trim() && next.title.trim() !== cur.name) args.title = next.title.trim();
      if (binding && binding.cat.id !== next.catId) args.folderId = targetFolderId;
      if (next.layout !== cur.layout) args.layout = next.layout;
      const summaryChanged = next.summaryMode !== curSummaryMode;
      const translateChanged = next.translateMode !== curTranslateMode;
      if (summaryChanged) args.summaryMode = next.summaryMode;
      if (translateChanged) args.translateMode = next.translateMode;
      if (args.title === undefined && args.folderId === undefined && args.layout === undefined
        && args.summaryMode === undefined && args.translateMode === undefined) {
        get().showToast('未做任何修改');
        return;
      }
      if (!summaryChanged && !translateChanged) {
        /* 纯元数据编辑（不含 AI 更改）：不触碰 AI 键，保持既有直连路径。 */
        void api
          .updateFeed(args)
          .then(() => get().reloadFromBackend())
          .then(() => get().showToast('订阅源已更新'))
          .catch((e: unknown) => {
            const msg = extractError(e);
            get().showToast(`保存失败：${msg}`);
          });
        return;
      }
      /* OPT-011 R5：含 AI 更改的编辑 = 一笔多键操作——只登记实际改变的 AI 字段，
         完整 update_feed 只发一次（不拆事务、不丢标题/分类/布局、不回显未改变
         的 AI 字段），经协议与四快捷入口共用同一 per-键队列：请求排在涉及键
         已排队的写之后（同域有序），乐观落地为一次原子索引更新（避免重复失败
         toast——落定合并为一条）。 */
      const modePatch: { summaryMode?: FeedAiMode; translateMode?: FeedAiMode } = {};
      const writes = [];
      if (summaryChanged) {
        modePatch.summaryMode = next.summaryMode;
        const target = next.summaryMode;
        writes.push({
          key: `feed:${feedId}:summary`,
          value: target,
          confirmed: curSummaryMode,
          apply: (v: unknown) => applyFeedAiModePatch(feedId, { summaryMode: v as FeedAiMode }),
          label: 'AI 摘要开关',
        });
      }
      if (translateChanged) {
        modePatch.translateMode = next.translateMode;
        const target = next.translateMode;
        writes.push({
          key: `feed:${feedId}:translate`,
          value: target,
          confirmed: curTranslateMode,
          apply: (v: unknown) => applyFeedAiModePatch(feedId, { translateMode: v as FeedAiMode }),
          label: 'AI 翻译开关',
        });
      }
      issueAiMultiWrite({
        writes,
        applyAll: () => applyFeedAiModePatch(feedId, modePatch),
        request: () =>
          api.updateFeed(args).then((r) => {
            /* 元数据（标题/分类/布局）落库后的既有全量刷新；失败不判定本笔写入
               失败（写入已成功），由刷新路径自身提示。 */
            void get().reloadFromBackend().catch(() => null);
            return r;
          }),
        label: '保存',
        successToast: '订阅源已更新',
      });
      return;
    }
    /* mock 模式：改内存树（改名/改属性 + 跨分类移动一次完成） */
    set((s) => {
      const moving = s.categories.flatMap((c) => c.feeds).find((f) => f.id === feedId);
      if (!moving) return s;
      const updated: FeedItem = {
        ...moving,
        name: next.title.trim() || moving.name,
        layout: next.layout as FeedItem['layout'],
        summaryMode: next.summaryMode,
        translateMode: next.translateMode,
        /* 旧 bool 兼容位同步（旧输入语义：缺 mode 时映射来源）——on→true，其余 false */
        autoSummary: next.summaryMode === 'on',
        autoTranslate: next.translateMode === 'on',
      };
      /* 先全部摘除，再放进目标分类 */
      const stripped = s.categories.map((c) => ({ ...c, feeds: c.feeds.filter((f) => f.id !== feedId) }));
      const finalCategories = stripped.map((c) => (c.id === next.catId ? { ...c, feeds: [...c.feeds, updated] } : c));
      return reconcileCategories(s, finalCategories);
    });
    get().showToast('订阅源已更新');
  },

  refreshOneFeed: (feedId) => {
    if (get().dataMode !== 'tauri') {
      get().showToast('演示模式不支持直连');
      return;
    }
    get().showToast('正在刷新此源…');
    void api
      .refreshFeed(Number(feedId))
      .then((n) => {
        if (n === null) return;
        get().showToast(n > 0 ? `已刷新，新增 ${n} 条` : '已刷新，无新文章');
        /* TASK-111②：单源刷新与后台刷新（feeds-updated）同属内容刷新——
           reload 携带保位请求，落地后 Timeline 消费顶条锚回位（审计「后台
           刷新保留当前阅读位置」）。 */
        return get().reloadFromBackend({ keepReadingPosition: true });
      })
      .catch((e: unknown) => {
        const msg = extractError(e);
        get().showToast(`刷新失败：${msg}`, { label: '重试', run: () => get().refreshOneFeed(feedId) });
      });
  },

  /* 布局/AI 开关统一走乐观更新：先改 store（设置页与主界面同帧生效，
     不再依赖 reloadFromBackend 全量重拉——异步竞态会让设置页显示回旧值），
     落库 fire-and-forget，失败 toast 提醒（本地状态不回滚，下次同步对齐）。 */
  updateCatLayout: (catId, layout) => {
    set((s) => reconcileCategories(s, s.categories.map((c) => (c.id === catId ? { ...c, layout } : c))));
    get().showToast('布局已更新');
    void api
      .updateFolderLayout(Number(catId.replace('cat-', '')), layout)
      .catch(() => get().showToast('布局未能保存，重启后可能回退'));
  },

  updateFeedLayout: (catId, feedId, layout) => {
    /* layout 参数来自 LAYOUT_OPTIONS（'inherit' | 五布局之一），原样存进
       feed——此前误写 as 'inherit' 把任何选择强转成继承，导致独立布局
       设了不生效、reload 后回落到分类布局（历史 bug 模式 C 契约混淆） */
    const next = layout as FeedItem['layout'];
    set((s) =>
      reconcileCategories(
        s,
        s.categories.map((c) =>
          c.id === catId
            ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, layout: next } : f)) }
            : c,
        ),
      ),
    );
    get().showToast('布局已更新');
    void api
      .updateFeedLayout(numericId(feedId), next)
      .catch(() => get().showToast('布局未能保存，重启后可能回退'));
  },

  /* OPT-011 R4：AI 开关统一走 issueAiWrite（aiConfigWrites 协议单点）——
     - 原子更新：categories 与 feedIndex 同一次 setState，消费者当帧同值；
     - 单字段 patch：只带被改字段（分类 Option 参数 / 源单字段参数），两字段
       的写入互不覆盖；
     - per-键 wire 队列 + 待写意图/确认态：同键串行、迟到失败不回滚更晚写入；
       最新失败恢复最近一次可确认值；读取快照接受时保留未落定意图
       （《AI配置写入与快照对账》：读取凭据 / 意图 / 确认态分离）。 */
  toggleCatSummary: (catId, val) => {
    const cat = get().categories.find((c) => c.id === catId);
    if (!cat || cat.autoSummary === val) return;
    issueAiWrite({
      key: `cat:${catId}:summary`,
      value: val,
      confirmed: cat.autoSummary,
      apply: (v) => applyCategoryAiFlagPatch(catId, 'autoSummary', v),
      request: () => api.setFolderAiFlags(numericId(catId), val, undefined),
      label: 'AI 摘要开关',
    });
  },

  toggleCatTranslate: (catId, val) => {
    const cat = get().categories.find((c) => c.id === catId);
    if (!cat || cat.autoTranslate === val) return;
    issueAiWrite({
      key: `cat:${catId}:translate`,
      value: val,
      confirmed: cat.autoTranslate,
      apply: (v) => applyCategoryAiFlagPatch(catId, 'autoTranslate', v),
      request: () => api.setFolderAiFlags(numericId(catId), undefined, val),
      label: 'AI 翻译开关',
    });
  },

  toggleFeedSummary: (catId, feedId, mode) => {
    const cur = get()
      .categories.find((c) => c.id === catId)
      ?.feeds.find((f) => f.id === feedId);
    if (!cur) return;
    const prevMode = normalizeFeedAiMode(cur.summaryMode, cur.autoSummary);
    if (prevMode === mode) return;
    issueAiWrite({
      key: `feed:${feedId}:summary`,
      value: mode,
      confirmed: prevMode,
      apply: (v) => applyFeedAiModePatch(feedId, { summaryMode: v }),
      request: () => api.setFeedAiModes(numericId(feedId), mode, undefined),
      label: 'AI 摘要开关',
    });
  },

  toggleFeedTranslate: (catId, feedId, mode) => {
    const cur = get()
      .categories.find((c) => c.id === catId)
      ?.feeds.find((f) => f.id === feedId);
    if (!cur) return;
    const prevMode = normalizeFeedAiMode(cur.translateMode, cur.autoTranslate);
    if (prevMode === mode) return;
    issueAiWrite({
      key: `feed:${feedId}:translate`,
      value: mode,
      confirmed: prevMode,
      apply: (v) => applyFeedAiModePatch(feedId, { translateMode: v }),
      request: () => api.setFeedAiModes(numericId(feedId), undefined, mode),
      label: 'AI 翻译开关',
    });
  },

  toggleFolderCollapse: (catId) => {
    set((s) => ({
      categories: s.categories.map((c) => (c.id === catId ? { ...c, collapsed: !c.collapsed } : c)),
    }));
    /* 落库：分类折叠状态 */
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderCollapsed(numericId(catId), cat.collapsed).catch(() => get().showToast('折叠状态未能保存，重启后可能回退'));
  },

  toggleAllFolders: () => {
    const anyOpen = get().categories.some((c) => !c.collapsed);
    set((s) => ({ categories: s.categories.map((c) => ({ ...c, collapsed: anyOpen })) }));
    get().showToast(anyOpen ? '已收起全部分类' : '已展开全部分类');
    /* 批量落库折叠状态：TASK-100 P3-4——此前逐分类发 N 次 IPC、失败逐个 toast
       （几十个分类一次点击几十次往返 + 连环弹窗）。改为并发聚合，失败合并成
       一条 toast（含失败个数）。 */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      const cats = get().categories;
      void Promise.allSettled(cats.map((c) => api.setFolderCollapsed(numericId(c.id), anyOpen))).then((settled) => {
        const failed = settled.filter((r) => r.status === 'rejected').length;
        if (failed > 0) get().showToast(`${failed} 个分类的折叠状态未能保存，重启后可能回退`);
      });
    }
  },

  toggleSettingsCatCollapse: (catId) =>
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, settingsCollapsed: !c.settingsCollapsed } : c,
      ),
    })),
});

import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { reconcileCategories } from '../internals';
import { numericId } from '../selectors';
import type { FeedItem } from '../../types';
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

  addFeed: (catId, url, title, layout, autoSummary, autoTranslate, syncToBackend = true) => {
    if (get().dataMode === 'tauri') {
      const folderId = Number(catId.replace('cat-', ''));
      set({ syncStatus: 'syncing' });
      void api
        .addFeed(url, title || null, folderId, layout, autoSummary, autoTranslate, syncToBackend)
        .then(() => get().reloadFromBackend())
        .then(async () => {
          /* 勾选「同步到后端」→ 添加后立即跑 feeds 阶段推送新订阅到远端
             （add_feed 只入队，这里触发推送让勾选语义即时生效） */
          if (syncToBackend && get().syncConnected) {
            await api.syncLocalFeeds().catch(() => null);
          }
          set({ syncStatus: 'synced' });
          get().showToast(`已添加订阅源：${title || url}`);
        })
        .catch((e: unknown) => {
          set({ syncStatus: 'error' });
          const msg = extractError(e);
          get().showToast(`添加失败：${msg}`, {
            label: '重试',
            run: () => get().addFeed(catId, url, title, layout, autoSummary, autoTranslate, syncToBackend),
          });
        });
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
                  autoSummary,
                  autoTranslate,
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
      /* 只提交变化字段：标题为空 = 不改名；分类/布局/AI 开关与当前一致则省略 */
      const binding = get().feedIndex.get(feedId);
      const cur = binding?.feed;
      if (!cur) return;
      const targetFolderId = Number(next.catId.replace('cat-', ''));
      const args: Parameters<typeof api.updateFeed>[0] = { id: Number(feedId) };
      if (next.title.trim() && next.title.trim() !== cur.name) args.title = next.title.trim();
      if (binding && binding.cat.id !== next.catId) args.folderId = targetFolderId;
      if (next.layout !== cur.layout) args.layout = next.layout;
      if (next.autoSummary !== cur.autoSummary) args.autoSummary = next.autoSummary;
      if (next.autoTranslate !== cur.autoTranslate) args.autoTranslate = next.autoTranslate;
      if (args.title === undefined && args.folderId === undefined && args.layout === undefined
        && args.autoSummary === undefined && args.autoTranslate === undefined) {
        get().showToast('未做任何修改');
        return;
      }
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
    /* mock 模式：改内存树（改名/改属性 + 跨分类移动一次完成） */
    set((s) => {
      const moving = s.categories.flatMap((c) => c.feeds).find((f) => f.id === feedId);
      if (!moving) return s;
      const updated: FeedItem = {
        ...moving,
        name: next.title.trim() || moving.name,
        layout: next.layout as FeedItem['layout'],
        autoSummary: next.autoSummary,
        autoTranslate: next.autoTranslate,
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
        return get().reloadFromBackend();
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

  toggleCatSummary: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoSummary: val } : c)) }));
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(numericId(catId), val, cat.autoTranslate).catch(() => get().showToast('AI 摘要开关未能保存，重启后可能回退'));
  },

  toggleCatTranslate: (catId, val) => {
    set((s) => ({ categories: s.categories.map((c) => (c.id === catId ? { ...c, autoTranslate: val } : c)) }));
    const cat = get().categories.find((c) => c.id === catId);
    if (cat) void api.setFolderAiFlags(numericId(catId), cat.autoSummary, val).catch(() => get().showToast('AI 翻译开关未能保存，重启后可能回退'));
  },

  toggleFeedSummary: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoSummary: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(numericId(feedId), val, get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoTranslate ?? false).catch(() => get().showToast('AI 摘要开关未能保存，重启后可能回退'));
  },

  toggleFeedTranslate: (catId, feedId, val) => {
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, feeds: c.feeds.map((f) => (f.id === feedId ? { ...f, autoTranslate: val } : f)) } : c,
      ),
    }));
    void api.setFeedAiFlags(numericId(feedId), get().categories.find((c) => c.id === catId)?.feeds.find((f) => f.id === feedId)?.autoSummary ?? false, val).catch(() => get().showToast('AI 翻译开关未能保存，重启后可能回退'));
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
    /* 批量落库折叠状态 */
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      for (const c of get().categories) {
        void api.setFolderCollapsed(numericId(c.id), anyOpen).catch(() => get().showToast('折叠状态未能保存，重启后可能回退'));
      }
    }
  },

  toggleSettingsCatCollapse: (catId) =>
    set((s) => ({
      categories: s.categories.map((c) =>
        c.id === catId ? { ...c, settingsCollapsed: !c.settingsCollapsed } : c,
      ),
    })),
});

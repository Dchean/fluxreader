import { create } from 'zustand';
import type { AppState } from './store/types';
import { bindAppStore } from './store/internals';
import { createNavSlice } from './store/slices/nav';
import { createReaderSlice } from './store/slices/reader';
import { createAiSlice } from './store/slices/ai';
import { createBootstrapSlice } from './store/slices/bootstrap';
import { createPlayerSlice } from './store/slices/player';
import { createUiSlice } from './store/slices/ui';
import { createSyncSlice } from './store/slices/sync';
import { createFeedsSlice } from './store/slices/feeds';
import { createSettingsSlice } from './store/slices/settings';

/* ============================================================
   全局客户端状态机 —— 对应规范 §2.2 + 原型交互引擎

   设计原则：
   1. store 只放「被跨组件共享的可变状态」与 action；派生数据
      （过滤/排序后的列表、计数）由 selector 钩子在组件层计算，
      避免每次渲染都全量重算。
   2. 条目数据（articles by layout）放在 store 里而不是
      模块级可变数组：消除 splice 副作用，任何变更都走 set()，
      React 才能可靠地重渲染（Tauri 环境下由 SQLite 快照整体替换）。
   3. 导航类 action 统一重置「已读保留快照」，保证未读筛选语义。

   本文件只做组合：状态与 action 按领域拆到 src/store/slices/*.ts，
   每个 slice 是 StateCreator<AppState, [], [], XxxSlice>。各 slice 的键集
   两两不相交（用 Pick<AppState, …> 声明，见各文件），因此这里的展开顺序
   不参与任何取值——不会出现同名 key 被后展开的 slice 覆盖。
   ============================================================ */

export { bootstrapGithubAuth } from './store/slices/bootstrap';

/* Note: 为何按领域拆 slice（而不是像后端那样按文件搬函数），以及键集如何保证不重不漏：
   slice 类型写成 Pick<AppState, …>，9 个 Pick 的并集必须恰好等于 AppState ——
   少一个键组合结果就不再是 AppState、多一个键触发多余属性检查，两者都由 tsc -b 拦下；
   正文是逐行搬移（所有 slice 的非空行多重集与原文完全相等，非重写）。
   完整取舍、替代方案与核验方式见本任务实施报告 §1/§3。 */
export const useAppStore = create<AppState>((...a) => ({
  ...createNavSlice(...a),
  ...createReaderSlice(...a),
  ...createAiSlice(...a),
  ...createBootstrapSlice(...a),
  ...createPlayerSlice(...a),
  ...createUiSlice(...a),
  ...createSyncSlice(...a),
  ...createFeedsSlice(...a),
  ...createSettingsSlice(...a),
}));

/* 注入 store 句柄：供跨 slice 的模块级 helper（视图缓存、标读/收藏收口、
   GitHub 轮询）读写。与原实现时序一致——模块求值期不会调用它们。 */
bindAppStore(useAppStore);

export * from './store/selectors';

export type { AppState, PodcastPlayerState, SettingsState, ToastMessage } from './store/types';

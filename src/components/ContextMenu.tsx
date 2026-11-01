import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useAppStore } from '../store';
import { Icons } from './icons';
import { openExternal } from '../lib/external';

/* ============================================================
   全局右键菜单 —— 替换 WebView2 默认右键菜单（Copy/Cut/Paste/Select All）
   与客户端功能区对齐，按右键目标分上下文：

   - article：文章卡片（时间流列表）→ 收藏/已读/打开源网页/复制链接
   - feed：订阅源（侧栏）→ 刷新/编辑/删除
   - reader：正文阅读区 → 复制/全文/翻译/摘要（若已选中文字则优先复制）
   - 空白/其他 → 全局（新建订阅/刷新全部/搜索/设置）

   通过 data-ctx + data-id 等属性标记可右键元素，ContextMenuHost 挂在 App
   顶层，全局 contextmenu 捕获 + preventDefault 禁用系统默认菜单。
   ============================================================ */

interface MenuItem {
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

export function ContextMenuHost() {
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  /* 打开前持有焦点的元素：关闭时还原（键盘用户回到触发处，不落 body）。 */
  const previousFocusRef = useRef<HTMLElement | null>(null);

  const close = useCallback(() => setMenu(null), []);

  /** 当前可用（未禁用）菜单项，按 DOM 顺序——方向键/Home/End 的移动集合。 */
  const enabledItems = (): HTMLElement[] =>
    Array.from(menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])') ?? []);

  /* 打开即焦点入首项（WAI-ARIA menu 模式，审计卡2-7）：此前菜单仅鼠标可达度
     完整，键盘用户要先 Tab 逐项摸索。焦点定位在提交后（portal 已挂载真实
     DOM），preventScroll 避免定位引发的意外滚动。审计返工 P3-2：打开时记录
     打开前的焦点元素，菜单关闭（effect 清理 = 卸载/重建）时还原——触发元素
     已被移出文档则跳过（不得聚焦游离节点）。 */
  useEffect(() => {
    if (!menu) return;
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    enabledItems()[0]?.focus({ preventScroll: true });
    return () => {
      const prev = previousFocusRef.current;
      previousFocusRef.current = null;
      if (prev && prev.isConnected) prev.focus({ preventScroll: true });
    };
    // Note: exhaustive-deps — enabledItems 读 menuRef（ref 不入依赖）；随 menu 重开执行一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menu]);

  /* 全局 contextmenu：禁用默认菜单 + 按目标构建自定义菜单 */
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      // 若菜单已打开且用户在其上右键，先关闭再重建（避免叠加）
      const items = buildMenuFor(e.target as HTMLElement);
      e.preventDefault();
      e.stopPropagation();
      if (items.length === 0) {
        setMenu(null);
        return;
      }
      setMenu({ x: e.clientX, y: e.clientY, items });
    };
    // capture：在所有子元素 contextmenu 冒泡前拦截
    document.addEventListener('contextmenu', onCtx, true);
    return () => document.removeEventListener('contextmenu', onCtx, true);
  }, []);

  /* 点击外部 / Esc 关闭 */
  useEffect(() => {
    if (!menu) return;
    const onDown = (e: MouseEvent) => {
      if (e.button === 2) return; // 右键交给 contextmenu 处理
      if (menuRef.current?.contains(e.target as Node)) return;
      close();
    };
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        close();
        /* 审计返工 P3-2：菜单优先消费 Esc 并截停——防止事件继续到达 App 全局
           监听后按序关闭下一层浮层（菜单是当前最顶层，一次 Esc 只关一层）。 */
        e.stopPropagation();
      }
    };
    const onScroll = () => close();
    const onResize = () => close();
    window.addEventListener('mousedown', onDown, true);
    document.addEventListener('keydown', onEsc, true);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onResize);
    return () => {
      window.removeEventListener('mousedown', onDown, true);
      document.removeEventListener('keydown', onEsc, true);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onResize);
    };
  }, [menu, close]);

  if (!menu) return null;

  /* 位置钳制：菜单不超出视口右下角 */
  const estW = 200;
  const estH = menu.items.length * 34 + 12;
  const style: CSSProperties = {
    position: 'fixed',
    left: Math.min(menu.x, window.innerWidth - estW - 8),
    top: Math.min(menu.y, window.innerHeight - estH - 8),
    zIndex: 3000,
  };

  return createPortal(
    /* role=menu：菜单项可被键盘 Tab 进入并操作（此前仅鼠标可达）。
       ref 上仍是同一个 mousedown/Esc/scroll 关闭逻辑，行为未变。 */
    <div
      className="ctx-menu"
      style={style}
      ref={menuRef}
      role="menu"
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        /* 单键快捷键让路（审计返工 P2-2）：菜单不在 shortcutYield 的浮层集合
           内，j/k/s/m（含大写）若继续冒泡会到达 App 全局监听并作用于菜单
           背后的当前文章（inInput=false、浮层让路判定不含本菜单）——在容器
           层截停。方向键分支已有 preventDefault，不受影响。 */
        if (/^[smjkSMJK]$/.test(e.key)) {
          e.stopPropagation();
          return;
        }
        /* 方向键导航（WAI-ARIA menu 模式，审计卡2-7）：↑↓ 循环、Home/End
           跳首/末，跳过禁用项（不在移动集合里）。Esc 关闭由 document 捕获级
           监听承担（既有逻辑，不变）。焦点不在项上（如刚点开未落焦）时
           ↓ 视为到首项、↑ 视为到末项。 */
        if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
        const items = enabledItems();
        if (items.length === 0) return;
        e.preventDefault();
        const current = items.indexOf(document.activeElement as HTMLElement);
        let next: number;
        if (e.key === 'Home') next = 0;
        else if (e.key === 'End') next = items.length - 1;
        else if (e.key === 'ArrowDown') next = current < 0 ? 0 : (current + 1) % items.length;
        else if (e.key === 'ArrowUp') next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length;
        else return;
        items[next]?.focus({ preventScroll: true });
      }}
    >
      {menu.items.map((it, i) => (
        <div
          key={i}
          className={`ctx-menu-item ${it.danger ? 'danger' : ''} ${it.disabled ? 'disabled' : ''}`}
          role="menuitem"
          tabIndex={it.disabled ? -1 : 0}
          aria-disabled={it.disabled || undefined}
          onKeyDown={(e) => {
            if (it.disabled) return;
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              close();
              it.onSelect();
            }
          }}
          onClick={() => {
            if (it.disabled) return;
            close();
            it.onSelect();
          }}
        >
          <span className="ctx-menu-icon">{it.icon}</span>
          <span className="ctx-menu-label">{it.label}</span>
        </div>
      ))}
    </div>,
    document.body,
  );
}

/* ============================================================
   菜单构建：根据右键目标类型生成菜单项
   ============================================================ */

function buildMenuFor(target: HTMLElement): MenuItem[] {
  const st = useAppStore.getState();
  // 从右键点向上找最近的 data-ctx 标记元素
  const el = target.closest<HTMLElement>('[data-ctx]');
  const ctx = el?.getAttribute('data-ctx');

  switch (ctx) {
    case 'article': {
      const id = el!.getAttribute('data-id') ?? '';
      const art = st.entries.find((a) => a.id === id);
      if (!art) return [];
      return [
        {
          label: art.isStarred ? '取消收藏' : '收藏',
          icon: art.isStarred ? <Icons.starFilled /> : <Icons.star />,
          onSelect: () => st.toggleEntryFlag(id, 'isStarred'),
        },
        {
          label: art.isRead ? '标为未读' : '标为已读',
          icon: art.isRead ? <Icons.unreadDot /> : <Icons.check />,
          onSelect: () => st.toggleEntryFlag(id, 'isRead'),
        },
        ...(art.url
          ? [{
              label: '查看原文',
              icon: <Icons.externalLink />,
              onSelect: () => { void openExternal(art.url); },
            },
            {
              label: '复制链接',
              icon: <Icons.copy />,
              onSelect: () => { void copyText(art.url!); },
            }]
          : []),
      ];
    }

    case 'feed': {
      const feedId = el!.getAttribute('data-id') ?? '';
      const catId = el!.getAttribute('data-cat') ?? '';
      return [
        {
          label: '刷新此源',
          icon: <Icons.refresh />,
          onSelect: () => st.refreshOneFeed(feedId),
        },
        {
          label: '编辑订阅源',
          icon: <Icons.edit />,
          onSelect: () => st.openEditFeedModal(feedId),
        },
        {
          label: '删除订阅源',
          icon: <Icons.trash />,
          danger: true,
          onSelect: () => st.deleteFeed(catId, feedId),
        },
      ];
    }

    case 'reader': {
      const items: MenuItem[] = [];
      // 有选中文字 → 优先提供「复制」
      const sel = window.getSelection()?.toString() ?? '';
      if (sel) {
        items.push({
          label: '复制',
          icon: <Icons.copy />,
          onSelect: () => { void copyText(sel); },
        });
      }
      const art = st.activeArticleId ? st.entries.find((a) => a.id === st.activeArticleId) : null;
      if (art) {
        items.push({
          label: art.isStarred ? '取消收藏' : '收藏',
          icon: art.isStarred ? <Icons.starFilled /> : <Icons.star />,
          onSelect: () => st.toggleCurrentStar(),
        });
        items.push({
          label: art.isRead ? '标为未读' : '标为已读',
          icon: art.isRead ? <Icons.unreadDot /> : <Icons.check />,
          onSelect: () => st.toggleCurrentReadStatus(),
        });
        items.push({
          label: st.showFulltext ? 'RSS 原文' : '显示全文',
          icon: <Icons.doc />,
          onSelect: () => st.toggleReaderFulltext(),
        });
        items.push({
          label: st.isShowingTranslatedProse ? '显示原文' : '翻译',
          icon: <Icons.globe />,
          onSelect: () => st.toggleReaderTranslation(),
        });
        items.push({
          label: '摘要',
          icon: <Icons.spark />,
          onSelect: () => st.triggerReaderSummary(),
        });
        if (art.url) {
          items.push({
            label: '复制链接',
            icon: <Icons.copy />,
            onSelect: () => { void copyText(art.url!); },
          });
        }
      }
      return items;
    }

    default: {
      // 空白区域 / 未标记元素 → 全局菜单
      return [
        {
          /* TASK-100 U2：叫法统一「添加订阅源」——与侧栏/命令面板/弹窗标题同一套说法 */
          label: '添加订阅源',
          icon: <Icons.plus />,
          onSelect: () => st.openAddFeedModal(''),
        },
        {
          label: '刷新全部订阅源',
          icon: <Icons.refresh />,
          onSelect: () => st.triggerManualSync(),
        },
        {
          label: '搜索',
          icon: <Icons.search />,
          onSelect: () => st.openSearch(),
        },
        {
          label: '设置',
          icon: <Icons.settings />,
          onSelect: () => st.openSettings(),
        },
      ];
    }
  }
}

/* 复制文本到剪贴板（失败静默，toast 提示） */
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    useAppStore.getState().showToast('已复制到剪贴板');
  } catch {
    // 降级：execCommand（部分 webview 环境 clipboard API 不可用）
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
      useAppStore.getState().showToast('已复制到剪贴板');
    } catch {
      useAppStore.getState().showToast('复制失败');
    }
  }
}

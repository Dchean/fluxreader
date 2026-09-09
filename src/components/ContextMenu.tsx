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

  const close = useCallback(() => setMenu(null), []);

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
      if (e.key === 'Escape') close();
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
    <div className="ctx-menu" style={style} ref={menuRef} onClick={(e) => e.stopPropagation()}>
      {menu.items.map((it, i) => (
        <div
          key={i}
          className={`ctx-menu-item ${it.danger ? 'danger' : ''} ${it.disabled ? 'disabled' : ''}`}
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
              label: '打开源网页',
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
          onSelect: () => {
            /* P0-6：右键删除源走全局确认框（与设置页一致，I-UI-4） */
            st.openConfirm({
              title: '删除订阅源',
              message: '删除后该源及其文章将从本地移除（若已同步到服务端，远端订阅保留）。确定删除吗？',
              confirmText: '删除',
              onConfirm: () => st.deleteFeed(catId, feedId),
            });
          },
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
          label: st.showFulltext ? 'RSS 原文' : '全文',
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
          label: '新建订阅源',
          icon: <Icons.plus />,
          onSelect: () => st.openAddFeedModal(''),
        },
        {
          label: '刷新全部订阅',
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

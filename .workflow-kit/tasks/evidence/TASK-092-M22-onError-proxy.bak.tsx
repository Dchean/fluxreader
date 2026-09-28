import { useEffect, useSyncExternalStore, type ReactNode } from 'react';
import { Icons } from './icons';
import { coverView, driveCover, getCoverState, onCoverError, subscribeCover } from '../lib/coverImage';

/* ============================================================
   封面图片位共享组件（TASK-092 / REQ-106 ③）
   文章卡、播客卡、迷你播放条、全屏播放器、灯箱五处统一使用：
   - 取图路径与失败状态全部来自 lib/coverImage.ts（代理判定仍只在 lib/imageProxy.ts）；
   - 代理未返回 / 失败时渲染占位节点：沿用调用方的类名（尺寸、圆角、比例不变）
     + 回退类 cover-fallback（背景与图标沿用画廊「无图」占位的视觉语言），不出现破图图标；
   - 失败时由 driveCover 上报 report_broken_cover（同条目同 URL 幂等）。
   cover 为空时返回 empty（默认 null）：不渲染 img、不代理、不上报。
   ============================================================ */

export interface CoverImageProps {
  /** 封面 URL（文章条目的 cover / image_url）；空 = 无封面 */
  src: string | null | undefined;
  /** 封面所属文章条目 id（上报 report_broken_cover 用）；没有则只回退不上报 */
  articleId?: string | null;
  /** 文章原文 URL（后端 fetch_image 的 Referer 候选链末项） */
  pageUrl?: string;
  className: string;
  alt: string;
  loading?: 'lazy' | 'eager';
  /** 失败/加载中占位的类名；缺省为 `${className} cover-fallback` */
  fallbackClassName?: string;
  /** 无封面时渲染的内容（缺省不渲染任何节点，保持各处既有空态） */
  empty?: ReactNode;
}

export function CoverImage({ src, articleId, pageUrl, className, alt, loading, fallbackClassName, empty = null }: CoverImageProps) {
  const url = src ?? '';
  const state = useSyncExternalStore(subscribeCover, () => getCoverState(url), () => getCoverState(url));
  /* 副作用唯一入口：需要代理且未请求 → 发起一次；已失败 → 幂等上报 */
  useEffect(() => { driveCover(url, pageUrl, articleId); }, [url, pageUrl, articleId, state]);

  const view = coverView(url, state);
  if (view.kind === 'none') return <>{empty}</>;
  if (view.kind === 'placeholder') {
    return (
      <div
        className={fallbackClassName ?? `${className} cover-fallback`}
        data-cover-state={view.state}
        {...(alt ? { role: 'img', 'aria-label': alt } : { 'aria-hidden': true })}
      >
        <Icons.image />
      </div>
    );
  }
  /* 代理分支同样挂 onError：字节嗅探命中但 WebView2 不可解码（如 HEIC）时退回占位并幂等上报 */
  if (!view.direct) return <img src={view.src} className={className} alt={alt} loading={loading} data-cover-route="proxy" onError={() => onCoverError(url)} />;
  return (
    <img
      src={view.src}
      className={className}
      alt={alt}
      loading={loading}
      referrerPolicy="no-referrer"
      data-cover-route="direct"
      onError={() => onCoverError(url)}
    />
  );
}

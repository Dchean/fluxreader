import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Icons } from './icons';
import { useAppStore } from '../store';
import { reportCoverFailure } from '../lib/coverImage';
import { canProxyImages, needsImageProxy } from '../lib/imageProxy';
import { useImageResource } from './useImageResource';

export interface CoverImageProps {
  src: string | null | undefined;
  articleId?: string | null;
  pageUrl?: string;
  className: string;
  alt: string;
  loading?: 'lazy' | 'eager';
  fallbackClassName?: string;
  empty?: ReactNode;
  enabled?: boolean;
}
export function CoverImage({ src, articleId, pageUrl, className, alt, loading, fallbackClassName, empty = null, enabled = true }: CoverImageProps) {
  const busy = useAppStore(s => s.libraryRestoreBusy);
  const generation = useAppStore(s => s.libraryGeneration);
  const node = useRef<HTMLElement | null>(null);
  const [visible, setVisible] = useState(enabled);
  // Player panels remain mounted. Keep the old lease during their actual exit animation.
  useEffect(() => {
    let frame = 0;
    const check = () => {
      if (enabled) setVisible(true);
      else if (!node.current?.isConnected || node.current.getClientRects().length === 0) setVisible(false);
      else frame = requestAnimationFrame(check);
    };
    frame = requestAnimationFrame(check);
    return () => cancelAnimationFrame(frame);
  }, [enabled]);
  const { lease, snapshot, retry } = useImageResource(src, pageUrl, !busy && (enabled || visible), 1, `${generation}:${articleId ?? ''}`);
  useEffect(() => {
    if (!lease) return;
    let current = true;
    reportCoverFailure(lease, articleId, () => {
      const state = useAppStore.getState();
      const entry = state.entries.find(item => item.id === articleId);
      return current && !state.libraryRestoreBusy && state.libraryGeneration === generation
        && !!entry && !entry.unavailable && entry.cover === src;
    });
    return () => { current = false; };
  }, [lease, snapshot, src, articleId, generation]);
  if (!src?.trim()) return <>{empty}</>;
  const proxy = canProxyImages() && needsImageProxy(src);
  const ready = snapshot.status === 'ready' ? snapshot : !proxy && !lease ? { src, direct: true } : null;
  if (ready && !busy && (enabled || visible)) return <img ref={el => { node.current = el; }} src={ready.src} className={className} alt={alt} loading={loading}
    referrerPolicy={ready.direct ? 'no-referrer' : undefined} data-cover-route={ready.direct ? 'direct' : 'proxy'}
    onError={() => lease?.failDecode()} />;
  const failed = snapshot.status === 'failed';
  return <div ref={el => { node.current = el; }} className={fallbackClassName ?? `${className} cover-fallback`}
    data-cover-state={failed ? 'failed' : 'pending'} {...(!failed ? alt ? { role: 'img', 'aria-label': alt } : { 'aria-hidden': true } : {})}>
    <Icons.image />
    {failed && <button type="button" className="image-retry" aria-label="重新加载图片" title={snapshot.status === 'failed' && snapshot.reason === 'too-large' ? '图片过大，无法加载' : '图片加载失败，重试'}
      onClick={event => { event.stopPropagation(); retry(); }}>重试</button>}
  </div>;
}



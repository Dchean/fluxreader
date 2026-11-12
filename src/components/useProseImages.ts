import { useEffect, useMemo, useRef } from 'react';
import { useAppStore } from '../store';
import { acquireImage, canProxyImages, needsImageProxy, retryImage, type ImageLease } from '../lib/imageProxy';

interface Source { url: string; pageUrl?: string }
const originals = new WeakMap<HTMLImageElement, Source>();
/** Only nodes bound by this helper have a proxied original identity. Never persist a Blob URL. */
export function originalProseImage(node: HTMLImageElement): Source | null {
  const owned = originals.get(node);
  if (owned) return owned;
  const src = node.currentSrc || node.src;
  return /^https?:\/\//.test(src) ? { url: src } : null;
}
interface Prepared { __html: string; sources: { index: number; url: string }[] }
function prepare(html: string): Prepared {
  if (!canProxyImages() || typeof document === 'undefined' || !html.toLowerCase().includes('<img')) return { __html: html, sources: [] };
  // Template contents are inert; preparation itself must not start remote image/media loads.
  const template = document.createElement('template');
  template.innerHTML = html;
  const sources: Prepared['sources'] = [];
  [...template.content.querySelectorAll('img')].forEach((img, index) => {
    const src = img.getAttribute('src') ?? '';
    if (!/^https?:\/\//.test(src) || !needsImageProxy(src)) return;
    sources.push({ index, url: src });
    // Remove before DOM insertion: a later effect cannot prevent the initial browser request.
    img.removeAttribute('src'); img.removeAttribute('srcset');
  });
  return { __html: sources.length ? template.innerHTML : html, sources };
}
/** The existing sanitized HTML and its video nodes remain stable while only individual img srcs change. */
export function useProseImages(html: string, pageUrl: string | undefined, owner: string, enabled: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  const retryAdmission = useRef<(() => void) | null>(null);
  const capacityRevision = useAppStore(s => s.mediaCapacityRevision);
  const prepared = useMemo(() => prepare(html), [html]);
  const markup = useMemo(() => ({ __html: prepared.__html }), [prepared]);
  useEffect(() => {
    const root = ref.current;
    if (!root || !enabled) return;
    const nodes = [...root.querySelectorAll('img')];
    for (const img of nodes) {
      const src = img.getAttribute('src') ?? '';
      if (/^https?:\/\//.test(src)) originals.set(img, { url: src, pageUrl });
    }
    const bindings = prepared.sources.flatMap(source => {
      const img = nodes[source.index];
      if (!img) return [];
      originals.set(img, { url: source.url, pageUrl });
      let lease: ImageLease | null = null;
      let unsubscribe = () => {};
      let active = false;
      let dead = false;
      const style = { width: img.style.width, height: img.style.height, aspectRatio: img.style.aspectRatio, visibility: img.style.visibility };
      const button = document.createElement('button');
      button.type = 'button'; button.className = 'image-retry'; button.hidden = true;
      img.after(button);
      const restoreStyle = () => Object.assign(img.style, style);
      const reserveGeometry = () => {
        const rect = img.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0 && img.naturalWidth > 0) {
          img.style.width = `${rect.width}px`; img.style.height = 'auto';
          img.style.aspectRatio = `${img.naturalWidth} / ${img.naturalHeight}`;
        }
      };
      const paint = () => {
        if (dead || !lease) return;
        const state = lease.snapshot();
        if (state.status === 'ready') {
          button.hidden = true;
          if (img.getAttribute('src') !== state.src) img.setAttribute('src', state.src);
          img.style.visibility = style.visibility;
        } else {
          reserveGeometry(); img.removeAttribute('src'); img.style.visibility = 'hidden';
          button.hidden = state.status !== 'failed' && state.status !== 'deferred';
          button.textContent = state.status === 'failed' && state.reason === 'too-large'
            ? '图片过大，重试' : state.status === 'failed' ? '图片加载失败，重试' : '图片加载暂缓，重试';
          // src is gone before release can trim/revoke. The local failed handle keeps retry UI stable.
          if (state.status === 'failed' && state.reason === 'decode') { unsubscribe(); lease.release(); }
        }
      };
      const acquire = () => {
        if (dead || lease?.admitted) return;
        unsubscribe(); lease?.release();
        lease = acquireImage(source.url, pageUrl, 0); unsubscribe = lease.subscribe(paint); paint();
      };
      const release = () => {
        reserveGeometry(); img.removeAttribute('src'); img.style.visibility = 'hidden';
        unsubscribe(); lease?.release(); lease = null; button.hidden = true;
      };
      const load = () => { if (lease?.snapshot().status === 'ready') restoreStyle(); };
      const error = () => { if (img.hasAttribute('src')) lease?.failDecode(); };
      const retry = (event: MouseEvent) => {
        event.preventDefault(); event.stopPropagation();
        retryImage(source.url, pageUrl); release(); if (active) acquire();
      };
      img.addEventListener('load', load); img.addEventListener('error', error); button.addEventListener('click', retry);
      return [{ img, retryAdmission() { if (active && !lease?.admitted) acquire(); }, visibility(visible: boolean) { active = visible; if (visible) acquire(); else release(); },
        close() { dead = true; release(); restoreStyle(); button.remove(); originals.delete(img);
          img.removeEventListener('load', load); img.removeEventListener('error', error); button.removeEventListener('click', retry); } }];
    });
    retryAdmission.current = () => { for (const binding of bindings) binding.retryAdmission(); };
    const byNode = new Map(bindings.map(binding => [binding.img, binding]));
    const observer = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver(changes => {
      for (const change of changes) byNode.get(change.target as HTMLImageElement)?.visibility(change.isIntersecting);
    }, { root: root.closest('.reader-scroll-content'), rootMargin: '300px' });
    for (const binding of bindings) { if (observer) observer.observe(binding.img); else binding.visibility(true); }
    return () => { retryAdmission.current = null; observer?.disconnect(); for (const binding of bindings) binding.close(); for (const img of nodes) originals.delete(img); };
  }, [prepared, pageUrl, owner, enabled]);
  useEffect(() => { retryAdmission.current?.(); }, [capacityRevision]);
  return { ref, markup };
}





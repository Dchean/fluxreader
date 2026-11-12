import { useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import { useAppStore } from '../store';
import { acquireImage, imageResourceKey, retryImage, type ImageLease, type ImageSnapshot } from '../lib/imageProxy';
const PENDING: ImageSnapshot = { status: 'deferred' };
interface BindingSnapshot { lease: ImageLease | null; snapshot: ImageSnapshot; identity: string }
const EMPTY: BindingSnapshot = { lease: null, snapshot: PENDING, identity: '' };
/** One binding belongs to an existing React subscriber; the service stores no extra waiters. */
function bindingFor(url: string | null | undefined, pageUrl: string | undefined, enabled: boolean, priority: number, identity: string) {
  let lease: ImageLease | null = null;
  const idle: BindingSnapshot = { ...EMPTY, identity };
  let current: BindingSnapshot = idle;
  let unsubscribe = () => {};
  const listeners = new Set<() => void>();
  const update = () => {
    current = lease ? { lease, snapshot: lease.snapshot(), identity } : idle;
    for (const listener of listeners) listener();
  };
  const acquire = () => {
    if (!url || !enabled || lease?.admitted || !listeners.size) return;
    unsubscribe(); lease?.release();
    lease = acquireImage(url, pageUrl, priority);
    unsubscribe = lease.subscribe(update);
    update();
  };
  return {
    snapshot: () => current,
    subscribe(listener: () => void) {
      listeners.add(listener); acquire();
      return () => {
        listeners.delete(listener);
        if (!listeners.size) { unsubscribe(); lease?.release(); lease = null; current = idle; }
      };
    },
    retryAdmission: acquire,
    releaseDecoded(committedLease: ImageLease | null) {
      if (!lease || lease !== committedLease || current.snapshot.status !== 'failed' || current.snapshot.reason !== 'decode') return;
      // Keep this owner's failed snapshot for retry, but no longer pin the backing entry.
      unsubscribe(); lease.release();
    },
  };
}
/** Acquire occurs when React subscribes after commit, and cleanup follows the DOM owner. */
export function useImageResource(url: string | null | undefined, pageUrl?: string, enabled = true, priority = 1, owner = '') {
  const capacityRevision = useAppStore(s => s.mediaCapacityRevision);
  const key = url ? imageResourceKey(url, pageUrl) : '';
  const [attempt, setAttempt] = useState(0);
  // These factors identify the DOM owner, not the shared network key; same URL with a new owner must rebind.
  const identity = JSON.stringify([key, owner, attempt]);
  const binding = useMemo(() => bindingFor(url, pageUrl, enabled, priority, identity), [url, pageUrl, enabled, priority, identity]);
  const value = useSyncExternalStore(binding.subscribe, binding.snapshot, () => EMPTY);
  useEffect(() => { binding.retryAdmission(); }, [binding, capacityRevision]);
  // Every hook consumer replaces failed img with its fallback in this commit first.
  useEffect(() => {
    if (value.snapshot.status === 'failed' && value.snapshot.reason === 'decode') binding.releaseDecoded(value.lease);
  }, [binding, value]);
  return { ...value, retry: () => { if (url) retryImage(url, pageUrl); setAttempt(n => n + 1); } };
}


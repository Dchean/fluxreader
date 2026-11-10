import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown } from './component-tests/harness.mjs';
armLifecycleWatchdog();
await import('./component-tests/gallery-media.test.mjs');
await import('./component-tests/gallery-virtual.test.mjs');
await import('./component-tests/entry-actions.test.mjs');
await import('./component-tests/reader-prose.test.mjs');
after(async () => { try { await runFinalTeardown(); } finally { disarmLifecycleWatchdog(); } });

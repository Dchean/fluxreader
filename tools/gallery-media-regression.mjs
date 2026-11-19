// 手工入口，不属 npm 门禁（package.json 只登记 test:frontend / test:components /
// test:ui-layout / docs:check）；带生命周期 watchdog 与终检（component-tests/harness.mjs）。
import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown } from './component-tests/harness.mjs';
armLifecycleWatchdog();
await import('./component-tests/gallery-media.test.mjs');
await import('./component-tests/gallery-virtual.test.mjs');
await import('./component-tests/entry-actions.test.mjs');
await import('./component-tests/reader-prose.test.mjs');
after(async () => { try { await runFinalTeardown(); } finally { disarmLifecycleWatchdog(); } });

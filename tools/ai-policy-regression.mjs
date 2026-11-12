import { after } from 'node:test';
import { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown } from './component-tests/harness.mjs';
armLifecycleWatchdog();
await import('./component-tests/ai-policy.test.mjs');
await import('./component-tests/ai-lifecycle.test.mjs');
after(async () => { try { await runFinalTeardown(); } finally { disarmLifecycleWatchdog(); } });

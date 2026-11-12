// OPT-016B 真实挂载回归入口（Node 内置 test 运行）。
//
// 运行：npm run test:components
//   = tsc -p tsconfig.test.json && node ./tools/component-regression.mjs
// 默认串行执行 business 与 selfcheck 两个独立进程，任一失败即整体非 0；
// 显式 --group business|selfcheck 仅供定向诊断，未知/多余参数立即非 0。
//
// 与 tools/frontend-regression.mjs（纯逻辑状态机回归）分工：
// - 旧通道不挂 DOM、不跑 effect——保护 store/selector 行为；
// - 本通道用 jsdom + React createRoot/act 真实挂载生产组件，保护 effect/
//   事件/异步链与真实 DOM 契约；两通道共享 loader 和 dist-test store/bodyCache。
// - jsdom 的布局/网络 API 与外部 IPC 使用明确 fake，不代替真实 Tauri/Edge。
//
// Note: 业务组件与runner自检各保留原ref看门狗和完整收尾；总入口不漏组、失败继续并聚合。
// 见 .agents/notes/implemented/testing/2026-09-16-测试门禁与断言纪律.md
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const groups = ['business', 'selfcheck'];
if (args.length === 0) {
  for (const group of groups) {
    console.log(`[component-regression] group ${group}: starting`);
    try {
      const result = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--group', group], {
        cwd: fileURLToPath(new URL('../', import.meta.url)),
        env: process.env,
        stdio: 'inherit',
      });
      if (result.error || result.signal || result.status !== 0) {
        process.exitCode = 1;
        console.error(`[component-regression] group ${group}: failed (status=${result.status}, signal=${result.signal ?? 'none'})`,
          result.error?.message ?? '');
      } else {
        console.log(`[component-regression] group ${group}: completed`);
      }
    } catch (error) {
      process.exitCode = 1;
      console.error(`[component-regression] group ${group}: spawn failed`, error);
    }
  }
} else if (args.length !== 2 || args[0] !== '--group' || !groups.includes(args[1])) {
  console.error('用法: node tools/component-regression.mjs [--group business|selfcheck]');
  process.exitCode = 2;
} else {
  const { after } = await import('node:test');
  const { armLifecycleWatchdog, disarmLifecycleWatchdog, runFinalTeardown } =
    await import('./component-tests/harness.mjs');

  // 原120秒ref看门狗；用例/after pending仍按原规则exit 3，不能自然空转假绿。
  armLifecycleWatchdog();
  if (args[1] === 'business') {
    /* 用例文件按「先最小可运行、后复杂链路」顺序注册（node:test 在 import 时登记）。 */
    await import('./component-tests/reader-prose.test.mjs');
    await import('./component-tests/layouts.test.mjs');
    /* OPT-017C：画廊有界渲染——5000 项挂载数上界/键盘焦点/灯箱返回/加载更多/列分配。 */
    await import('./component-tests/gallery-virtual.test.mjs');
    await import('./component-tests/gallery-controls-startup.test.mjs');
    await import('./component-tests/gallery-media.test.mjs');
    await import('./component-tests/image-resources.test.mjs');
    await import('./component-tests/close-ask.test.mjs');
    await import('./component-tests/modifier-shortcuts.test.mjs');
    await import('./component-tests/entry-actions.test.mjs');
    await import('./component-tests/reading-session.test.mjs');
    await import('./component-tests/reading-session-audit.test.mjs');
    /* 资料库恢复替代旧OAuth授权关闭守卫；正常设置关闭仍覆盖。 */
    await import('./component-tests/local-library.test.mjs');
    await import('./component-tests/restore-gallery.test.mjs');
    /* 审计卡2（前端交互正确性与感知体验）：IME Escape 守卫、Reader 正文三态、
       右键菜单键盘导航、空态/加载态分流。 */
    await import('./component-tests/ime-escape.test.mjs');
    await import('./component-tests/reader-body-states.test.mjs');
    await import('./component-tests/reader-body-recovery.test.mjs');
    await import('./component-tests/context-menu.test.mjs');
    await import('./component-tests/audit-repairs.test.mjs');
    await import('./component-tests/player-focus-ai.test.mjs');
    await import('./component-tests/timeline-empty-loading.test.mjs');
    /* 2026-11-04：五布局 J/K 选中/焦点/aria、收藏移除焦点迁移、源行 Tab/动作按钮、
       播放器 Space 让路、live region polite/atomic、设置页表单可访问名。 */
    await import('./component-tests/keyboard-a11y.test.mjs');
    /* 审计返工 P2-1：防抖窗口内删回同值——searching 不卡真、不重发。 */
    await import('./component-tests/search-debounce.test.mjs');
    /* OPT-017E：错误中心——侧栏入口可见/面板可关闭/重试与忽略真实生效/普通 toast 不回归。 */
    await import('./component-tests/error-center.test.mjs');
    await import('./component-tests/hydrate.test.mjs');
    /* 2026-11-03：社交正文首帧折叠——长文落地首 commit 已 collapsed（受控 RO），短文正常展开。 */
    await import('./component-tests/social-collapse.test.mjs');
    await import('./component-tests/mutation-reader-prose.test.mjs');
    /* OPT-008B R2（主控清单②）：搜索跨布局结果——真实 SearchModal 点击打开 +
       Reader/布局实际呈现 + 返回/续页边界（数组存在不能自证渲染可见）。 */
    await import('./component-tests/search-cross-layout.test.mjs');
    /* OPT-008B R3（主控清单①）：Reader 主动修订恢复——无 Timeline 卡片也自动重水合；
       在途旧响应丢戳后新响应落地。 */
    await import('./component-tests/reader-revision-recovery.test.mjs');
    /* OPT-008B R3（主控清单⑥）：五布局共享正文修订——失效清空 + 真实重水合 +
       Reader/卡片渲染证据（social 含同窗口兄弟条目）。 */
    await import('./component-tests/revision-shared-layouts.test.mjs');
    /* OPT-007（F11）：feeds-updated 数据变化事件 → 前端真实监听器 → 保位 reload。 */
    await import('./component-tests/sync-data-events.test.mjs');
    /* 2026-11-04：同步灯/账户入口、SyncTab 分类摘要与 AddFeed 写入反馈。 */
    await import('./component-tests/sync-indicator.test.mjs');
    /* OPT-011（F13）：AI 开关三态——Reader 自动请求订阅有效配置 + FeedsTab 三态控件。 */
    await import('./component-tests/ai-settings.test.mjs');
    await import('./component-tests/ai-policy.test.mjs');
    await import('./component-tests/ai-lifecycle.test.mjs');
    /* OPT-013 B1：AI 产物缓存签名——「未标记版本」标记、未命中不短路（真实点击发
       起生成）、命中不重复计费、同键在途只发一次。 */
    await import('./component-tests/ai-cache-signature.test.mjs');
    /* 便携恢复/导出经真实UI接入写屏障与分阶段回读；旧快照同步已退役。 */
    await import('./component-tests/config-apply.test.mjs');
    /* 数据安全：手动数据库备份与缓存清理边界文案（真实挂载）。 */
    await import('./component-tests/data-safety.test.mjs');
    /* OPT-009 B3：订阅写回能力声明与持久回执（FeedsTab 确认文案、Fever 仅本地、
       subscription-outbox-changed 事件链驱动摘要卡）。 */
    await import('./component-tests/subscription-writeback.test.mjs');
    await import('./component-tests/sync-recovery.test.mjs');
    await import('./component-tests/translation-links.test.mjs');
    /* 2026-11-04：用户文案精简——能力矩阵折叠、Fever 本地边界、搜索错误重试、
       危险确认保留与低价值成功 toast 清零。 */
    await import('./component-tests/copy-reduction.test.mjs');
    /* W4（2026-11-01 前端体验批次）：产品树 ErrorBoundary、模态焦点陷阱与
       dialog 语义、AiTab 读改写卡内串行化（新 Key 竞态回归）。 */
    await import('./component-tests/error-boundary.test.mjs');
    await import('./component-tests/modal-focus.test.mjs');
    /* 2026-11-02：关闭态浮层输入不抢焦——ModalOverlay 常驻挂载 + 条件 autoFocus
       的真实挂载回归（jsdom 不尊崇 inert，修前该通道必红）。 */
    await import('./component-tests/modal-autofocus.test.mjs');
    await import('./component-tests/ai-config-race.test.mjs');
  } else {
    await import('./component-tests/runner-selfcheck.test.mjs');
  }

  // 每组都完成清理、错误账本终检及晚错守卫；原final teardown负责显式非0。
  // pending时finally不可达，ref看门狗仍会判红，不吞hook异常。
  after(async () => {
    try {
      await runFinalTeardown();
    } finally {
      disarmLifecycleWatchdog();
    }
  });
}

// W4（2026-11-01 前端体验批次）：产品树 ErrorBoundary 回归。
//
// 取证方式（证据分层，与仓库既有口径一致）：
// - 行为腿：真实挂载生产 ErrorBoundary，子树为**崩溃探针**（渲染时抛错的测试
//   组件——不是替身被测组件，是触发边界的标准手段）。断言：全屏兜底态
//   （role=alert + 复用 bootstrap-error 文案视觉 + 自带全屏居中）、
//   componentDidCatch 记 log 且带
//   componentStack（归因需求，roadmap NotifCard #185）、重试清除错误态后子树
//   重新渲染。
// - 接线腿（App 是否真的包了产品树、窗口控制是否在边界外）：源码形态断言
//   （ui-contracts 域），本文件不重复。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, allowConsoleError, allowReactError, consoleErrors } from './harness.mjs';

/* 崩溃探针：持续抛错直到测试显式解除。React 19 并发渲染失败后会同步重试整棵根——
   探针若只抛一次，重试即「恢复成功」，边界根本不接手（实测教训）；故必须持续抛，
   让 getDerivedStateFromError 兜住，再经 disarm 验证重试路径。 */
function makeBomb() {
  const state = { disarmed: false };
  function Bomb() {
    if (!state.disarmed) throw new Error('探针崩溃：渲染期异常');
    return null;
  }
  Bomb.disarm = () => { state.disarmed = true; };
  return Bomb;
}

componentTest('W4 ErrorBoundary：子树崩溃出全屏兜底 + componentDidCatch 留 componentStack + 重试恢复', { timeout: 40000 }, async () => {
  /* 预期错误（one-shot 例外，未消费终检即红）：componentDidCatch 的 log 与
     React 边界捕获上报（harness reactErrors 通道）。 */
  allowConsoleError(/产品树渲染崩溃/);
  allowReactError(/探针崩溃/);

  const { createElement, act } = await import('react');
  const { ErrorBoundary } = await import('../../src/components/ErrorBoundary.tsx');
  const Bomb = makeBomb();
  const entry = await mount(createElement(ErrorBoundary, null, createElement(Bomb)));
  await flush(2);

  /* 兜底态：全屏视觉（error-boundary-splash 自带居中，bootstrap-error 提供文案样式）
     + role=alert + 消息 + 重试入口 */
  const fallback = entry.container.querySelector('.error-boundary-splash.bootstrap-error');
  assert.ok(fallback, '崩溃后应渲染全屏兜底态');
  assert.equal(fallback.getAttribute('role'), 'alert', '兜底态应带 role="alert"');
  assert.ok(fallback.textContent.includes('探针崩溃'), '兜底态应展示错误消息');
  const retry = fallback.querySelector('.bootstrap-error-retry');
  assert.ok(retry, '兜底态应提供重试入口');

  /* componentDidCatch 归因证据：log 带 componentStack（缺了它崩溃只能靠复现猜） */
  const logged = consoleErrors.find((m) => m.includes('产品树渲染崩溃'));
  assert.ok(logged, 'componentDidCatch 应记 error log');
  const stackArg = consoleErrors.find((m) => m.includes('componentStack'));
  assert.ok(stackArg, 'log 应携带 componentStack（归因需求）');

  /* 重试：清除错误态后子树重新渲染（探针 disarm → 不再抛） */
  Bomb.disarm();
  await act(async () => { retry.click(); });
  await flush(2);
  assert.equal(entry.container.querySelector('.error-boundary-splash.bootstrap-error'), null,
    '重试后兜底态应消失（子树重新渲染）');
});

// W4（2026-11-01 前端体验批次）：AiTab 读改写卡内串行化回归。
//
// 场景（用户路径）：点「测试连通性并保存」（t1，连通性测试在途——测试按钮此时
// 禁用，但 Key 输入框与「保存提示词」仍可用）→ 编辑 API Key → 点「保存提示词」
// （s2）。testAndSave 与 savePrompts 两入口并发：
// - 修复前：s2 立即落库（仅提示词字段）；t1 随后用**点击时的旧表单快照整包落库**
//   → 在途编辑的新 Key 丢失（回归前必红）。
// - 修复后：两入口按点击顺序入队；每个写入执行时重读库内最新配置，且字段值取
//   **执行时刻的表单态**（cfgRef）——t1 的端点写入携带编辑后的新 Key，
//   s2 只重放提示词字段、不回退 Key → 新 Key 必须落库。
//
// 证据边界：真实挂载生产 AiTab，IPC 走 fixtures 假后端（get/save_ai_config 语义
// 即「读改写 SQLite 单值」），连通性测试用受控 deferred 模拟「t1 慢、t2 快」。

import assert from 'node:assert/strict';
import { componentTest, mount, flush, resetStore, dom } from './harness.mjs';
import { installBackend, clearInvokeCalls, invokeCalls, setPlan, resetPlan } from './fixtures.mjs';

componentTest('W4 AiTab 串行化：测试在途时编辑 Key 再保存，新 Key 必须落库（不被旧快照覆盖）', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  let saved = null;
  const savedSeq = [];
  const listSeq = [];
  let releaseList1;
  const gate1 = new Promise((r) => { releaseList1 = r; });
  setPlan({
    get_ai_config: () => Promise.resolve(saved ? JSON.stringify(saved) : null),
    save_ai_config: (args) => { saved = JSON.parse(args.value); savedSeq.push(saved); return Promise.resolve(null); },
    /* t1（第一次调用）挂起；t2 及以后立即返回——构造「后点先测完」的乱序 */
    ai_list_models: (args) => { listSeq.push(args); return listSeq.length === 1 ? gate1 : Promise.resolve(['model-a']); },
  });

  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  const entry = await mount(createElement(AiTab));
  await flush(2);

  const baseUrlInput = entry.container.querySelector('input[placeholder^="https://"]');
  assert.ok(baseUrlInput, 'Base URL 输入框存在');
  const keyInput = entry.container.querySelector('input[type="password"]');
  assert.ok(keyInput, 'API Key 输入框存在');
  const setInput = async (el, value) => {
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set;
      setter.call(el, value);
      el.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    });
  };
  await setInput(baseUrlInput, 'https://api.example.com/v1');
  await setInput(keyInput, 'sk-OLD');

  const testBtn = [...entry.container.querySelectorAll('button')].find((b) => b.textContent.includes('测试连通性并保存'));
  assert.ok(testBtn, '「测试连通性并保存」按钮存在');

  /* t1：点击（连通性测试在途，持有 gate1；此后测试按钮禁用） */
  await act(async () => { testBtn.click(); });
  await flush(1);
  assert.equal(listSeq.length, 1, 't1 连通性测试已发起且在途');
  assert.equal(testBtn.disabled, true, '测试在途：按钮禁用（场景成立的前提）');

  /* 在途编辑 Key → 点「保存提示词」（另一入口，不禁用） */
  await setInput(keyInput, 'sk-NEW');
  const promptsBtn = [...entry.container.querySelectorAll('button')].find((b) => b.textContent.includes('保存提示词'));
  assert.ok(promptsBtn && !promptsBtn.disabled, '「保存提示词」按钮可用（并发入口成立）');
  await act(async () => { promptsBtn.click(); });
  await flush(2);

  /* 修复前此处即已偏差：s2 已先落库（仅提示词）。放行 t1，看最终落库。 */
  await act(async () => { releaseList1(['model-a']); });
  await flush(4);

  /* 落库顺序 = 点击顺序：t1（端点四字段，取执行时刻表单态 = 新 Key）先、
     s2（仅提示词）后；最终 sk-NEW 必须存活 */
  assert.equal(listSeq.length, 1, '仅 t1 发起连通性测试（s2 不触达端点）');
  assert.equal(savedSeq.length, 2, '两次落库都已执行（串行排队不丢写）');
  assert.equal(savedSeq[0].apiKey, 'sk-NEW', 't1 落库取执行时刻表单态（在途编辑的新 Key 随之落库）');
  assert.equal(savedSeq[1].apiKey, 'sk-NEW', 's2 只重放提示词字段，不回退 t1 已落的 Key');
  assert.ok(savedSeq[1].summaryPrompt && savedSeq[1].translatePrompt, 's2 落库携带两个提示词字段');
  assert.equal(saved.apiKey, 'sk-NEW', '最终库内 Key 必须是新 Key（修复前被 t1 旧快照覆盖）');
  assert.equal(saved.baseUrl, 'https://api.example.com/v1', '端点配置随最后一次写入保留');
  const cmdSeq = invokeCalls.map((c) => c.cmd);
  const firstSave = cmdSeq.indexOf('save_ai_config');
  const secondSave = cmdSeq.lastIndexOf('save_ai_config');
  assert.ok(firstSave < secondSave, '落库按点击顺序：t1 先、s2 后（修复前 s2 先落、t1 后落）');
  resetPlan();
});

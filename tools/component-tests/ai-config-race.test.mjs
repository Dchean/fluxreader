// AiTab 配置事务：真实组件 + 受控 IPC，分别验证草稿、已测试身份与已保存值。
// 2026-11-09：旧首例要求“测试旧 Key、保存新 Key”，把未测试配置存盘当作成功。
// 现保留提示词并发不丢写，另用审计 AUX-01/AUX-02 的操作序列约束身份和草稿。
// 允许保存已测快照并保留新草稿，或拒绝迟到结果；不固定内部队列/refs/写入次数。

import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, waitFor, resetStore, dom, useAppStore } from './harness.mjs';
import { installBackend, clearInvokeCalls, invokeCalls, setPlan, resetPlan, deferred } from './fixtures.mjs';

async function edit(el, value) {
  assert.ok(el, '被编辑的真实控件存在');
  const { act } = await import('react');
  await act(async () => {
    const proto = el.tagName === 'TEXTAREA'
      ? dom.window.HTMLTextAreaElement.prototype : dom.window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
    el.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
  });
}

function button(entry, text) {
  const el = [...entry.container.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
  assert.ok(el, `真实按钮存在：${text}`);
  return el;
}

componentTest('AiTab 测试在途保存提示词：端点与提示词均保留，不让迟到整包写覆盖新提示词', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetPlan();
  resetStore();
  let saved = null;
  const pending = deferred();
  setPlan({
    get_ai_config: () => Promise.resolve(saved ? JSON.stringify(saved) : null),
    save_ai_config: (args) => { saved = JSON.parse(args.value); return Promise.resolve(null); },
    ai_list_models: () => pending.promise,
  });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const entry = await mount(createElement(AiTab));
    await flush(2);
    await edit(entry.container.querySelector('[aria-label="API Base URL"]'), 'https://tested.example/v1');
    await edit(entry.container.querySelector('[aria-label="API Key"]'), 'fake-tested-key');
    await act(async () => { button(entry, '测试连通性并保存').click(); });
    await waitFor(() => invokeCalls.some((c) => c.cmd === 'ai_list_models'));
    await edit(entry.container.querySelector('[aria-label="摘要提示词"]'), '并发编辑后的摘要提示词');
    await edit(entry.container.querySelector('[aria-label="翻译提示词"]'), '并发编辑后的翻译提示词');
    await act(async () => { button(entry, '保存提示词').click(); });
    await act(async () => { pending.resolve(['model-a']); });
    await waitFor(() => saved?.summaryPrompt === '并发编辑后的摘要提示词');
    await flush(2);
    assert.equal(saved.baseUrl, 'https://tested.example/v1', '提示词保存不覆盖已测试端点');
    assert.equal(saved.apiKey, 'fake-tested-key', '测试完成后端点可用配置保留');
    assert.equal(saved.translatePrompt, '并发编辑后的翻译提示词', '迟到测试结果不丢新提示词');
    assert.equal(invokeCalls.filter((c) => c.cmd === 'ai_list_models').length, 1, '只保存提示词不触发额外端点测试');
  } finally {
    await act(async () => { pending.resolve(['model-a']); });
    await flush(2);
    resetPlan();
  }
});

componentTest('AiTab 测试 A 期间改成 B：保存身份必须已测试，迟到结果不丢 B 草稿', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  const pending = deferred();
  const writes = [];
  let saved = null;
  setPlan({
    get_ai_config: () => Promise.resolve(saved ? JSON.stringify(saved) : null),
    ai_list_models: (args) => args.baseUrl === 'https://tested-a.example/v1'
      ? pending.promise : Promise.reject(new Error('B 未通过连通测试')),
    save_ai_config: (args) => { saved = JSON.parse(args.value); writes.push(saved); return Promise.resolve(null); },
  });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const entry = await mount(createElement(AiTab));
    await flush(2);
    const base = entry.container.querySelector('[aria-label="API Base URL"]');
    const key = entry.container.querySelector('[aria-label="API Key"]');
    await edit(base, 'https://tested-a.example/v1');
    await edit(key, 'fake-key-a');
    await act(async () => { button(entry, '测试连通性并保存').click(); });
    await waitFor(() => invokeCalls.some((c) => c.cmd === 'ai_list_models'));
    assert.deepEqual(invokeCalls.find((c) => c.cmd === 'ai_list_models').args,
      { baseUrl: 'https://tested-a.example/v1', apiKey: 'fake-key-a' }, '受控慢请求确实测试 A');
    await edit(base, 'https://untested-b.example/v1');
    await edit(key, 'fake-invalid-key-b');
    await act(async () => { pending.resolve(['model-from-a']); });
    await waitFor(() => [...entry.container.querySelectorAll('button')]
      .some((b) => b.textContent.trim() === '测试连通性并保存' && !b.disabled));
    assert.equal(base.value, 'https://untested-b.example/v1', '已测试 A 的返回不得覆盖新地址草稿');
    assert.equal(key.value, 'fake-invalid-key-b', '已测试 A 的返回不得覆盖新 Key 草稿');
    assert.ok(writes.every((value) => value.baseUrl === 'https://tested-a.example/v1' && value.apiKey === 'fake-key-a'),
      `只可存已测快照 A 或拒绝陈旧结果；实际落库身份：${JSON.stringify(writes.map(({ baseUrl, apiKey }) => ({ baseUrl, apiKey })))}`);
    if (writes.length === 0) {
      assert.ok(!useAppStore.getState().toasts.some((t) => t.text.startsWith('连通成功')),
        '拒绝陈旧结果时不能把当前未测试草稿提示为连通成功');
    }
  } finally {
    await act(async () => { pending.resolve(['model-from-a']); });
    await flush(2);
    resetPlan();
  }
});

for (const edited of [false, true]) {
  componentTest(`AiTab 初始慢读取：${edited ? '不覆盖已输入的端点、Key 与提示词草稿' : '未编辑时仍恢复已存配置'}`, { timeout: 40000 }, async () => {
    installBackend();
    resetPlan();
    resetStore();
    const pending = deferred();
    setPlan({ get_ai_config: () => pending.promise });
    const { createElement, act } = await import('react');
    const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
    const stored = { baseUrl: 'https://stored-old.example/v1', apiKey: 'fake-stored-key', summaryPrompt: '库内摘要提示词' };
    try {
      const entry = await mount(createElement(AiTab));
      const base = entry.container.querySelector('[aria-label="API Base URL"]');
      const key = entry.container.querySelector('[aria-label="API Key"]');
      const prompt = entry.container.querySelector('[aria-label="摘要提示词"]');
      if (edited) {
        await edit(base, 'https://new-draft.example/v1');
        await edit(key, 'fake-new-draft-key');
        await edit(prompt, '新摘要草稿');
      }
      await act(async () => { pending.resolve(JSON.stringify(stored)); });
      await flush(3);
      assert.deepEqual({ baseUrl: base.value, apiKey: key.value, summaryPrompt: prompt.value }, edited
        ? { baseUrl: 'https://new-draft.example/v1', apiKey: 'fake-new-draft-key', summaryPrompt: '新摘要草稿' }
        : stored, edited ? '迟到回读不丢用户草稿' : '配置恢复不能整体禁用来规避竞态');
      assert.equal(invokeCalls.filter((c) => c.cmd === 'save_ai_config').length, 0, '初始读取或输入草稿不会自动落库');
    } finally {
      await act(async () => { pending.resolve(JSON.stringify(stored)); });
      await flush(2);
      resetPlan();
    }
  });
}

const INITIAL_CONFIG = {
  preset: 'custom', baseUrl: 'https://stored-b.example/v1', apiKey: 'stored-key', model: 'stored-model',
  summaryPrompt: 'stored-summary', translatePrompt: 'stored-translate',
};

const field = (entry, name) => entry.container.querySelector(`[aria-label="${name}"]`);
const testFinished = (entry) => [...entry.container.querySelectorAll('button')]
  .some((el) => el.textContent.trim() === '测试连通性并保存' && !el.disabled);

// 覆盖所有 async 边界。旧“初始恢复改变端点→仅作废在途测试”允许成功后的旧回读
// 覆盖表单；现在显式测试即接管端点，初始快照在这些阶段均无端点写权。
for (const phase of ['测试在途', '保存前读取在途', '保存请求在途', '保存成功后', '测试失败后', '保存失败后']) {
  componentTest(`AiTab 初始配置在${phase}返回：不接管已测试端点，提示词仍独立恢复`, { timeout: 20000 }, async () => {
    installBackend(); resetPlan(); resetStore();
    const initial = deferred();
    const models = deferred();
    const persistedRead = deferred();
    const save = deferred();
    const writes = [];
    let saved = { ...INITIAL_CONFIG };
    let reads = 0;
    let modelRequests = 0;
    let failSave = phase === '保存失败后';
    setPlan({
      get_ai_config: () => {
        const snapshot = JSON.stringify(saved);
        if (++reads === 1) return initial.promise;
        return phase === '保存前读取在途' && reads === 2 ? persistedRead.promise.then(() => snapshot) : Promise.resolve(snapshot);
      },
      ai_list_models: () => ++modelRequests === 1 ? models.promise : Promise.resolve(['deepseek-chat']),
      save_ai_config: (args) => {
        const value = JSON.parse(args.value);
        writes.push(value);
        if (failSave) { failSave = false; return Promise.reject(new Error('controlled save failure')); }
        return (phase === '保存请求在途' ? save.promise : Promise.resolve()).then(() => { saved = value; });
      },
    });
    const { createElement, act } = await import('react');
    const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
    try {
      const entry = await mount(createElement(AiTab));
      await waitFor(() => reads === 1);
      await edit(field(entry, 'API Key'), 'draft-key');
      const requested = { preset: 'deepseek', baseUrl: field(entry, 'API Base URL').value, apiKey: 'draft-key', model: field(entry, '当前使用模型').value };
      await act(async () => { button(entry, '测试连通性并保存').click(); });
      await waitFor(() => modelRequests === 1);
      assert.deepEqual(invokeCalls.find((call) => call.cmd === 'ai_list_models').args,
        { baseUrl: requested.baseUrl, apiKey: requested.apiKey }, '真实请求测试的是点击时显示的端点');

      if (phase === '测试失败后') {
        await act(async () => { models.reject(new Error('controlled test failure')); });
      } else if (phase !== '测试在途') {
        await act(async () => { models.resolve([requested.model]); });
      }
      if (phase === '保存前读取在途') await waitFor(() => reads === 2);
      if (phase === '保存请求在途') await waitFor(() => writes.length === 1);
      if (phase.endsWith('后')) await waitFor(() => testFinished(entry));

      await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); });
      await flush(2);
      assert.equal(field(entry, 'API Base URL').value, requested.baseUrl, '测试已接管未敲过键的默认地址');
      assert.equal(field(entry, 'API Key').value, requested.apiKey, '已输入Key保留');
      assert.equal(field(entry, '摘要提示词').value, INITIAL_CONFIG.summaryPrompt, '测试没有接管提示词，仍可正常恢复');
      assert.equal(field(entry, '翻译提示词').value, INITIAL_CONFIG.translatePrompt, '另一提示词仍可正常恢复');

      await act(async () => { models.resolve([requested.model]); persistedRead.resolve(); save.resolve(); });
      await waitFor(() => testFinished(entry));
      if (phase === '测试失败后' || phase === '保存失败后') {
        assert.equal(saved.baseUrl, INITIAL_CONFIG.baseUrl, '失败不会冒充持久化成功');
        assert.ok(!useAppStore.getState().toasts.some((toast) => toast.text.startsWith('连通成功')));
        await act(async () => { button(entry, '测试连通性并保存').click(); });
        await waitFor(() => modelRequests === 2 && testFinished(entry));
      }
      assert.deepEqual(Object.fromEntries(Object.keys(requested).map((key) => [key, saved[key]])), requested,
        '最终持久化的四字段与实际成功测试的快照一致');
      assert.equal(saved.summaryPrompt, INITIAL_CONFIG.summaryPrompt, '端点保存不覆盖提示词');
      assert.equal(field(entry, 'API Base URL').value, saved.baseUrl, '旧initial返回后显示仍对应已保存身份');
      assert.ok(writes.every((value) => value.baseUrl === requested.baseUrl && value.apiKey === requested.apiKey));
    } finally {
      await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); models.resolve(['deepseek-chat']); persistedRead.resolve(); save.resolve(); });
      await flush(2);
      resetPlan();
    }
  });
}

componentTest('AiTab 初始读取先完成：测试恢复的端点；单字段编辑仍允许其它字段恢复', { timeout: 20000 }, async () => {
  installBackend(); resetPlan(); resetStore();
  const initial = deferred();
  let reads = 0;
  let saved = { ...INITIAL_CONFIG };
  setPlan({
    get_ai_config: () => ++reads === 1 ? initial.promise : Promise.resolve(JSON.stringify(saved)),
    ai_list_models: () => Promise.resolve(['stored-model']),
    save_ai_config: (args) => { saved = JSON.parse(args.value); return Promise.resolve(null); },
  });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const entry = await mount(createElement(AiTab));
    await edit(field(entry, 'API Key'), 'edited-key');
    await edit(field(entry, '摘要提示词'), 'edited-summary');
    await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); });
    await flush(2);
    assert.equal(field(entry, 'API Base URL').value, INITIAL_CONFIG.baseUrl);
    assert.equal(field(entry, '当前使用模型').value, INITIAL_CONFIG.model);
    assert.equal(field(entry, 'API Key').value, 'edited-key');
    assert.equal(field(entry, '摘要提示词').value, 'edited-summary');
    assert.equal(field(entry, '翻译提示词').value, INITIAL_CONFIG.translatePrompt);
    await act(async () => { button(entry, '测试连通性并保存').click(); });
    await waitFor(() => testFinished(entry));
    assert.deepEqual(invokeCalls.find((call) => call.cmd === 'ai_list_models').args,
      { baseUrl: INITIAL_CONFIG.baseUrl, apiKey: 'edited-key' });
    assert.equal(saved.baseUrl, INITIAL_CONFIG.baseUrl);
    assert.equal(saved.apiKey, 'edited-key');
    assert.equal(saved.model, INITIAL_CONFIG.model);
    assert.equal(saved.summaryPrompt, INITIAL_CONFIG.summaryPrompt, '测试不偷偷持久化未保存的摘要草稿');
    assert.equal(field(entry, '摘要提示词').value, 'edited-summary');
  } finally {
    await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); });
    await flush(2); resetPlan();
  }
});

for (const editSummary of [false, true]) {
  componentTest(`AiTab 初始读取未完成即保存${editSummary ? '新摘要与默认翻译' : '默认提示词'}：旧initial不覆盖已提交提示词，端点仍恢复`, { timeout: 20000 }, async () => {
    installBackend(); resetPlan(); resetStore();
    const initial = deferred();
    let reads = 0;
    let saved = { ...INITIAL_CONFIG };
    setPlan({
      get_ai_config: () => ++reads === 1 ? initial.promise : Promise.resolve(JSON.stringify(saved)),
      save_ai_config: (args) => { saved = JSON.parse(args.value); return Promise.resolve(null); },
    });
    const { createElement, act } = await import('react');
    const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
    try {
      const entry = await mount(createElement(AiTab));
      await waitFor(() => reads === 1);
      if (editSummary) await edit(field(entry, '摘要提示词'), 'new-summary');
      const prompts = { summaryPrompt: field(entry, '摘要提示词').value, translatePrompt: field(entry, '翻译提示词').value };
      await act(async () => { button(entry, '保存提示词').click(); });
      await waitFor(() => saved.summaryPrompt === prompts.summaryPrompt && !button(entry, '保存提示词').disabled);
      await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); });
      await flush(2);
      assert.equal(field(entry, '摘要提示词').value, prompts.summaryPrompt);
      assert.equal(field(entry, '翻译提示词').value, prompts.translatePrompt, '显式保存的默认值同样有所有权');
      assert.equal(field(entry, 'API Base URL').value, INITIAL_CONFIG.baseUrl, '提示词保存不冻结端点恢复');
      assert.equal(field(entry, 'API Key').value, INITIAL_CONFIG.apiKey);
      assert.equal(saved.baseUrl, INITIAL_CONFIG.baseUrl);
      assert.equal(saved.translatePrompt, prompts.translatePrompt);
      assert.equal(invokeCalls.filter((call) => call.cmd === 'ai_list_models').length, 0);
    } finally {
      await act(async () => { initial.resolve(JSON.stringify(INITIAL_CONFIG)); });
      await flush(2); resetPlan();
    }
  });
}

componentTest('AiTab 卸载后的初始响应不能污染重挂页面的新配置', { timeout: 20000 }, async () => {
  installBackend(); resetPlan(); resetStore();
  const oldRead = deferred();
  let reads = 0;
  const current = { ...INITIAL_CONFIG, baseUrl: 'https://new-instance.example/v1', apiKey: 'new-instance-key' };
  setPlan({ get_ai_config: () => ++reads === 1 ? oldRead.promise : Promise.resolve(JSON.stringify(current)) });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const old = await mount(createElement(AiTab));
    await waitFor(() => reads === 1);
    await unmount(old);
    const next = await mount(createElement(AiTab));
    await waitFor(() => field(next, 'API Base URL').value === current.baseUrl);
    await act(async () => { oldRead.resolve(JSON.stringify(INITIAL_CONFIG)); });
    await flush(2);
    assert.equal(field(next, 'API Base URL').value, current.baseUrl);
    assert.equal(field(next, 'API Key').value, current.apiKey);
    assert.equal(invokeCalls.filter((call) => call.cmd === 'save_ai_config').length, 0);
  } finally {
    await act(async () => { oldRead.resolve(JSON.stringify(INITIAL_CONFIG)); });
    await flush(2); resetPlan();
  }
});

for (const editNewDraft of [false, true]) {
  componentTest(`AiTab 保存请求在途卸载重挂：初始恢复等待既有写落定${editNewDraft ? '且保留新页草稿' : ''}`, { timeout: 20000 }, async () => {
    installBackend(); resetPlan(); resetStore();
    const oldRead = deferred();
    const oldSave = deferred();
    let saved = { ...INITIAL_CONFIG };
    let reads = 0;
    let writes = 0;
    setPlan({
      get_ai_config: () => ++reads === 1 ? oldRead.promise : Promise.resolve(JSON.stringify(saved)),
      ai_list_models: () => Promise.resolve(['requested-model']),
      save_ai_config: (args) => {
        const payload = JSON.parse(args.value);
        writes++;
        return (writes === 1 ? oldSave.promise : Promise.resolve()).then(() => { saved = payload; });
      },
    });
    const { createElement, act } = await import('react');
    const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
    try {
      const old = await mount(createElement(AiTab));
      await waitFor(() => reads === 1);
      await edit(field(old, 'API Base URL'), 'https://confirmed-a.example/v1');
      await edit(field(old, 'API Key'), 'confirmed-key');
      await edit(field(old, '当前使用模型'), 'requested-model');
      await act(async () => { button(old, '测试连通性并保存').click(); });
      await waitFor(() => writes === 1);
      assert.equal(reads, 2, '初始读取与端点写前读取均已发起');
      await unmount(old);
      const next = await mount(createElement(AiTab));
      if (editNewDraft) {
        await edit(field(next, 'API Base URL'), 'https://new-draft.example/v1');
        await edit(field(next, 'API Key'), 'new-draft-key');
        await edit(field(next, '摘要提示词'), 'new-instance-summary');
      }
      await flush(2);
      assert.equal(reads, 2, '新页初始快照不能早于之前已经发出的写入');
      assert.equal(button(next, '测试连通性并保存').disabled, false, '等待初始读取不阻止新页操作');
      assert.equal(button(next, '保存提示词').disabled, false, '提示词操作独立可用');
      await act(async () => { oldSave.resolve(); });
      await waitFor(() => reads === 3 && field(next, '当前使用模型').value === 'requested-model');
      await act(async () => { oldRead.resolve(JSON.stringify(INITIAL_CONFIG)); });
      await flush(2);
      assert.equal(saved.baseUrl, 'https://confirmed-a.example/v1', '旧页已授权写入安全完成');
      assert.equal(field(next, 'API Base URL').value, editNewDraft ? 'https://new-draft.example/v1' : saved.baseUrl);
      assert.equal(field(next, 'API Key').value, editNewDraft ? 'new-draft-key' : saved.apiKey);
      assert.equal(field(next, '摘要提示词').value, editNewDraft ? 'new-instance-summary' : INITIAL_CONFIG.summaryPrompt);
      assert.equal(useAppStore.getState().toasts.length, 0, '已卸载页面的迟到成功不冒充当前页面结果');
      assert.equal(next.container.querySelectorAll('[role="combobox"]').length, 1,
        '旧页完成不能给新页安装旧模型选择结果');
      if (editNewDraft) {
        await act(async () => { button(next, '测试连通性并保存').click(); });
        await waitFor(() => writes === 2 && testFinished(next));
        assert.equal(saved.baseUrl, 'https://new-draft.example/v1');
        assert.equal(saved.apiKey, 'new-draft-key');
        assert.equal(field(next, '摘要提示词').value, 'new-instance-summary');
        assert.equal(saved.summaryPrompt, INITIAL_CONFIG.summaryPrompt, '新页端点保存仍不偷带未保存提示词');
      }
    } finally {
      await act(async () => { oldRead.resolve(JSON.stringify(INITIAL_CONFIG)); oldSave.resolve(); });
      await flush(2); resetPlan();
    }
  });
}

for (const kind of ['读取拒绝', '坏JSON', '非对象', '字段类型错误']) {
  componentTest(`AiTab 初始${kind}：可恢复提示、拒绝破坏性覆盖，重试仍保留草稿`, { timeout: 20000 }, async () => {
    installBackend(); resetPlan(); resetStore();
    let broken = true;
    const writes = [];
    setPlan({
      get_ai_config: () => {
        if (!broken) return Promise.resolve(JSON.stringify(INITIAL_CONFIG));
        if (kind === '读取拒绝') return Promise.reject(new Error('controlled read failure'));
        return Promise.resolve(kind === '坏JSON' ? '{' : kind === '非对象' ? '[]' : '{"apiKey":null}');
      },
      save_ai_config: (args) => { writes.push(JSON.parse(args.value)); return Promise.resolve(null); },
    });
    const { createElement, act } = await import('react');
    const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
    try {
      const entry = await mount(createElement(AiTab));
      await waitFor(() => entry.container.querySelector('[role="alert"]'));
      assert.match(entry.container.querySelector('[role="alert"]').textContent, /AI 配置读取失败/);
      await edit(field(entry, '摘要提示词'), 'error-kept-summary');
      await edit(field(entry, 'API Key'), 'error-kept-key');
      const currentTranslation = field(entry, '翻译提示词').value;
      await act(async () => { button(entry, '保存提示词').click(); });
      await waitFor(() => !button(entry, '保存提示词').disabled);
      assert.equal(writes.length, 0, '当前存储读取失败或内容损坏时不得当作空配置覆写');
      assert.ok(useAppStore.getState().toasts.some((toast) => toast.text === '保存失败'));
      broken = false;
      await act(async () => { button(entry, '重试读取').click(); });
      await waitFor(() => !entry.container.querySelector('[role="alert"]'));
      assert.equal(field(entry, 'API Base URL').value, INITIAL_CONFIG.baseUrl);
      assert.equal(field(entry, 'API Key').value, 'error-kept-key');
      assert.equal(field(entry, '摘要提示词').value, 'error-kept-summary');
      assert.equal(field(entry, '翻译提示词').value, currentTranslation, '失败保存不把已接管字段交回initial');
      await act(async () => { button(entry, '保存提示词').click(); });
      await waitFor(() => writes.length === 1 && !button(entry, '保存提示词').disabled);
      assert.equal(writes[0].summaryPrompt, 'error-kept-summary');
      assert.equal(writes[0].apiKey, INITIAL_CONFIG.apiKey, '提示词保存不带未测试的端点草稿');
    } finally { resetPlan(); }
  });
}

componentTest('AiTab 切换模型后保存提示词：模型落库且端点、既有字段均保留', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  let saved = { baseUrl: 'https://tested.example/v1', apiKey: 'fake-key', model: 'model-a', existing: '保留字段' };
  setPlan({
    get_ai_config: () => Promise.resolve(JSON.stringify(saved)),
    ai_list_models: () => Promise.resolve(['model-a', 'model-b']),
    save_ai_config: (args) => { saved = JSON.parse(args.value); return Promise.resolve(null); },
  });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const entry = await mount(createElement(AiTab));
    await flush(2);
    await act(async () => { button(entry, '测试连通性并保存').click(); });
    await waitFor(() => entry.container.querySelectorAll('[role="combobox"]').length === 2);
    const modelControl = [...entry.container.querySelectorAll('[role="combobox"]')].find((el) => el.textContent.includes('model-a'));
    assert.ok(modelControl, '测试成功后模型选择器可用');
    await act(async () => { modelControl.click(); });
    const option = [...document.querySelectorAll('[role="option"]')].find((el) => el.textContent.trim() === 'model-b');
    assert.ok(option, '真实模型列表包含 model-b');
    await act(async () => { option.click(); });
    await waitFor(() => saved.model === 'model-b');
    await edit(entry.container.querySelector('[aria-label="摘要提示词"]'), '模型切换后编辑的提示词');
    await act(async () => { button(entry, '保存提示词').click(); });
    await waitFor(() => saved.summaryPrompt === '模型切换后编辑的提示词');
    assert.equal(saved.model, 'model-b', '保存提示词不回退已选择模型');
    assert.equal(saved.baseUrl, 'https://tested.example/v1', '模型/提示词保存均不改端点');
    assert.equal(saved.apiKey, 'fake-key', '模型/提示词保存均不改 Key');
    assert.equal(saved.existing, '保留字段', '字段合并不丢其它已存值');
  } finally { resetPlan(); }
});

componentTest('AiTab 模型读取挂起期间保存提示词：迟到旧快照不得覆盖新提示词', { timeout: 40000 }, async () => {
  installBackend();
  resetPlan();
  resetStore();
  let saved = {
    baseUrl: 'https://tested.example/v1', apiKey: 'fake-key', model: 'model-a',
    summaryPrompt: 'old-summary', translatePrompt: 'old-translate',
  };
  const pending = deferred();
  let pauseNextRead = false;
  let modelReadStarted = false;
  setPlan({
    get_ai_config: () => {
      // 快照在读取发起时捕获；放行时才读 saved 会把旧响应伪装成新值，掩盖丢写。
      const snapshot = JSON.stringify(saved);
      if (pauseNextRead) {
        pauseNextRead = false;
        modelReadStarted = true;
        return pending.promise.then(() => snapshot);
      }
      return Promise.resolve(snapshot);
    },
    ai_list_models: () => Promise.resolve(['model-a', 'model-b']),
    save_ai_config: (args) => { saved = JSON.parse(args.value); return Promise.resolve(null); },
  });
  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  try {
    const entry = await mount(createElement(AiTab));
    await flush(2);
    await act(async () => { button(entry, '测试连通性并保存').click(); });
    await waitFor(() => entry.container.querySelectorAll('[role="combobox"]').length === 2
      && !button(entry, '测试连通性并保存').disabled);
    const modelControl = [...entry.container.querySelectorAll('[role="combobox"]')].find((el) => el.textContent.includes('model-a'));
    assert.ok(modelControl, '已测试端点的模型选择器存在');
    await act(async () => { modelControl.click(); });
    const option = [...document.querySelectorAll('[role="option"]')].find((el) => el.textContent.trim() === 'model-b');
    assert.ok(option, '真实列表可选 model-b');
    pauseNextRead = true;
    await act(async () => { option.click(); });
    await waitFor(() => modelReadStarted, { label: '模型保存已读到旧配置但尚未返回' });
    await edit(entry.container.querySelector('[aria-label="摘要提示词"]'), 'new-summary');
    await act(async () => { button(entry, '保存提示词').click(); });
    await flush(2);
    await act(async () => { pending.resolve(); });
    await waitFor(() => saved.model === 'model-b' && !button(entry, '保存提示词').disabled,
      { label: '模型保存和提示词保存均已结束' });
    assert.equal(saved.summaryPrompt, 'new-summary', '迟到模型写入不得覆盖新提示词');
    assert.equal(saved.translatePrompt, 'old-translate', '未编辑的翻译提示词保留');
    assert.equal(saved.baseUrl, 'https://tested.example/v1', '并发模型/提示词写入不改端点');
    assert.equal(saved.apiKey, 'fake-key', '并发模型/提示词写入不改 Key');
  } finally {
    await act(async () => { pending.resolve(); });
    await flush(2);
    resetPlan();
  }
});

componentTest('AiTab 首笔保存失败后仍可再次保存提示词，成功保留新值', { timeout: 40000 }, async () => {
  installBackend();
  clearInvokeCalls();
  resetStore();
  let saved = null;
  let failNextSave = true;
  setPlan({
    get_ai_config: () => Promise.resolve(saved ? JSON.stringify(saved) : null),
    /* 第一笔落库 reject（模拟 IPC/DB 失败）；其后恢复成功——锁定 enqueueAiConfigWrite
       的 then(op, op)：链不因前序失败卡死，后笔照常执行。 */
    save_ai_config: (args) => {
      if (failNextSave) { failNextSave = false; return Promise.reject(new Error('db busy（注入失败）')); }
      saved = JSON.parse(args.value);
      return Promise.resolve(null);
    },
  });

  const { createElement, act } = await import('react');
  const { AiTab } = await import('../../src/components/settings/AiTab.tsx');
  const entry = await mount(createElement(AiTab));
  await flush(2);

  const promptsBtn = [...entry.container.querySelectorAll('button')].find((b) => b.textContent.includes('保存提示词'));
  assert.ok(promptsBtn, '「保存提示词」按钮存在');

  /* 第一笔：落库 reject → 入口报「保存失败」，但链必须活着 */
  await act(async () => { promptsBtn.click(); });
  await flush(3);
  assert.ok(useAppStore.getState().toasts.some((t) => t.text === '保存失败'),
    '第一笔失败应如实提示（不静默）');

  /* 第二笔：编辑提示词后再保存——若链被前序失败卡死，此笔永不执行 */
  const summaryInput = entry.container.querySelector('.setting-prompt-textarea');
  assert.ok(summaryInput, '摘要提示词编辑器存在');
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(summaryInput, '自定义摘要提示词');
    summaryInput.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
  });
  await act(async () => { promptsBtn.click(); });
  await flush(4);

  assert.equal(failNextSave, false, '注入失败只发生一次（场景成立）');
  assert.ok(saved, '第二笔必须真实落库（链失败续跑即本用例判据）');
  assert.equal(saved.summaryPrompt, '自定义摘要提示词', '后笔重放执行时刻的表单字段');
  assert.ok(saved.translatePrompt, '第二笔基于库内最新读取合并（旧 blob 为空时以 fields 重建，两提示词齐备）');
  const saveCalls = invokeCalls.filter((c) => c.cmd === 'save_ai_config');
  assert.equal(saveCalls.length, 2, '两笔写请求都已发出（一次 reject、一次成功）');
  resetPlan();
});

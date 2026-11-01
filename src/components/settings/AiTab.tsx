import { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { Icons } from '../icons';
import { FluxDropdown, SettingCard } from '../primitives';
import { AI_PRESETS, DEFAULT_PROMPTS, type AiConfigState } from './shared';
import { enqueueAiConfigWrite, mergeConfigFields, mergePromptsOnly } from './aiConfig';

/* ---------- TAB 5: AI服务 ---------- */

export function AiTab() {
  const showToast = useAppStore((s) => s.showToast);
  const [cfg, setCfg] = useState<AiConfigState>({
    preset: 'deepseek',
    baseUrl: AI_PRESETS.deepseek.url,
    apiKey: '',
    model: AI_PRESETS.deepseek.model,
    summaryPrompt: DEFAULT_PROMPTS.summary,
    translatePrompt: DEFAULT_PROMPTS.translate,
  });
  const [models, setModels] = useState<string[]>([]);
  const [testing, setTesting] = useState(false);
  const [savingPrompts, setSavingPrompts] = useState(false);
  /* 队列写入执行时读最新表单态：入口排队期间用户仍可编辑（测试按钮 testing
     期间禁用，但 Key 输入框与提示词编辑器不禁用）——后写者重放的是**执行时刻**
     的字段值，不是点击时的闭包快照（否则「测试在途改 Key」的编辑会丢）。 */
  const cfgRef = useRef(cfg);
  useEffect(() => {
    cfgRef.current = cfg;
  });

  /* 打开设置时从后端恢复已存配置 */
  useEffect(() => {
    void api.getAiConfig().then((raw) => {
      if (!raw) return;
      try {
        const saved = JSON.parse(raw) as Partial<AiConfigState>;
        setCfg((c) => ({
          ...c,
          preset: saved.preset ?? c.preset,
          baseUrl: saved.baseUrl ?? c.baseUrl,
          apiKey: saved.apiKey ?? c.apiKey,
          model: saved.model ?? c.model,
          summaryPrompt: saved.summaryPrompt ?? c.summaryPrompt,
          translatePrompt: saved.translatePrompt ?? c.translatePrompt,
        }));
      } catch { /* 忽略坏 JSON */ }
    }).catch(() => { /* TASK-067 N10：挂载期配置恢复失败静默兜底（默认配置可用） */ });
  }, []);

  const applyPreset = (p: string) => {
    const preset = AI_PRESETS[p];
    setCfg((c) => ({
      ...c,
      preset: p,
      baseUrl: p === 'custom' ? c.baseUrl : preset.url,
      model: p === 'custom' ? c.model : preset.model,
    }));
    setModels([]);
  };

  /** 测试连通性（/models）→ 拉模型列表 → 保存配置 */
  const testAndSave = async () => {
    if (!cfg.baseUrl.trim()) { showToast('请填写 Base URL'); return; }
    if (!cfg.apiKey.trim()) { showToast('请填写 API Key'); return; }
    setTesting(true);
    try {
      /* 整个入口（含连通性测试）入队：串行化判据是「点击顺序」。若只把落库段
         入队，后点先测完的写入会先执行，仍会乱序覆盖。
         落库只重放端点四字段（不再整包快照 stringify）——测试+保存不应
         顺手覆盖并发中的提示词/模型写入；写时重读库内最新配置合并。 */
      await enqueueAiConfigWrite(async () => {
        const list = await api.aiListModels(cfgRef.current.baseUrl.trim(), cfgRef.current.apiKey.trim());
        if (!list) { showToast('演示模式无法测试'); return; }
        setModels(list);
        const persisted = await api.getAiConfig();
        /* 落库时刻重读表单态：入队/在途期间的编辑（如改 Key）一并落库 */
        const cur = cfgRef.current;
        await api.saveAiConfig(mergeConfigFields(persisted, {
          preset: cur.preset,
          baseUrl: cur.baseUrl.trim(),
          apiKey: cur.apiKey.trim(),
          model: cur.model,
        }));
        showToast(`连通成功：${list.length} 个可用模型`);
      });
    } catch (e) {
      showToast(`连通失败：${extractError(e)}`);
    } finally {
      setTesting(false);
    }
  };

  /** 仅保存提示词（不动端点配置）：先读回库里的现有配置，只覆盖两个提示词字段。
      P2-4：此前直接把表单整份快照落库，等于把端点配置（含未确认可用的
      API Key）一起保存，与按钮语义相反。 */
  const savePrompts = async () => {
    setSavingPrompts(true);
    try {
      await enqueueAiConfigWrite(async () => {
        const persisted = await api.getAiConfig();
        await api.saveAiConfig(mergePromptsOnly(persisted, {
          summaryPrompt: cfgRef.current.summaryPrompt,
          translatePrompt: cfgRef.current.translatePrompt,
        }));
      });
      showToast('提示词已保存');
    } catch {
      showToast('保存失败');
    } finally {
      setSavingPrompts(false);
    }
  };

  const modelOptions = (models.length > 0 ? models : [cfg.model || '（先测试连通性）'])
    .map((m) => ({ value: m, label: m }));

  /** 切换模型即落库（P2-4 前半）：只在连通性测试成功后模型下拉才出现，此时库里
      已有测试时保存的端点配置；此前改模型只改本地表单，不点「测试连通性并保存」
      就不生效（后端 load_ai_config 读的是库里的 model）——用户以为换了模型，
      实际仍用旧模型。这里只覆盖 model 一个字段，不动其它端点配置。 */
  const selectModel = async (v: string) => {
    setCfg((c) => ({ ...c, model: v }));
    try {
      await enqueueAiConfigWrite(async () => {
        const persisted = await api.getAiConfig();
        await api.saveAiConfig(mergeConfigFields(persisted, { model: v }));
      });
      showToast(`已切换模型：${v}`);
    } catch (e) {
      showToast(`模型保存失败：${extractError(e)}`);
    }
  };

  return (
    <>
      <div className="settings-group-title">API 端点</div>
      <SettingCard title="服务商预设" desc="官方端点一键填入；newapi 中转选自定义">
        <FluxDropdown
          width={200}
          value={cfg.preset}
          onChange={applyPreset}
          options={Object.entries(AI_PRESETS).map(([k, v]) => ({ value: k, label: v.label }))}
        />
      </SettingCard>
      <SettingCard title="API Base URL" desc={cfg.preset === 'custom' ? '任意 OpenAI 兼容地址（newapi 等）' : '预设自动填入，可覆盖'}>
        <input
          type="text"
          className="setting-input"
          placeholder="https://your-newapi.example.com/v1"
          value={cfg.baseUrl}
          onChange={(e) => setCfg((c) => ({ ...c, baseUrl: e.target.value }))}
        />
      </SettingCard>
      <SettingCard title="API Key" desc="密钥仅存本地 SQLite">
        <input
          type="password"
          className="setting-input"
          placeholder="sk-…"
          value={cfg.apiKey}
          onChange={(e) => setCfg((c) => ({ ...c, apiKey: e.target.value }))}
        />
      </SettingCard>
      <SettingCard title="当前使用模型" desc={models.length > 0 ? `端点返回 ${models.length} 个模型` : '测试连通性后自动拉取'}>
        {models.length > 0 ? (
          <FluxDropdown
            width={200}
            value={cfg.model}
            onChange={(v) => void selectModel(v)}
            options={modelOptions}
          />
        ) : (
          <input
            type="text"
            className="setting-input"
            placeholder="模型名（测试后可选）"
            value={cfg.model}
            onChange={(e) => setCfg((c) => ({ ...c, model: e.target.value }))}
          />
        )}
      </SettingCard>
      <div className="settings-action-row">
        <button
          className="toggle-action-btn btn-primary"
          disabled={testing}
          onClick={() => void testAndSave()}
        >
          <Icons.spark />
          <span>{testing ? '测试中…' : '测试连通性并保存'}</span>
        </button>
      </div>

      <div className="settings-group-title">系统提示词</div>
      <div className="prompt-editor-card">
        <div className="prompt-editor-title">AI 摘要提示词 (Prompt)</div>
        <textarea
          className="setting-prompt-textarea"
          value={cfg.summaryPrompt}
          onChange={(e) => setCfg((c) => ({ ...c, summaryPrompt: e.target.value }))}
        />
      </div>
      <div className="prompt-editor-card">
        <div className="prompt-editor-title">AI 翻译提示词 (Prompt)</div>
        <textarea
          className="setting-prompt-textarea"
          value={cfg.translatePrompt}
          onChange={(e) => setCfg((c) => ({ ...c, translatePrompt: e.target.value }))}
        />
      </div>
      <div className="settings-action-row">
        <button
          className="toggle-action-btn btn-primary"
          disabled={savingPrompts}
          onClick={() => void savePrompts()}
        >
          <Icons.save />
          <span>保存提示词</span>
        </button>
      </div>
    </>
  );
}

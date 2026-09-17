import { useEffect, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { Icons } from '../icons';
import { FluxDropdown, SettingCard } from '../primitives';
import { AI_PRESETS, DEFAULT_PROMPTS, type AiConfigState } from './shared';

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
    });
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
      const list = await api.aiListModels(cfg.baseUrl.trim(), cfg.apiKey.trim());
      if (!list) { showToast('演示模式无法测试'); return; }
      setModels(list);
      await api.saveAiConfig(JSON.stringify(cfg));
      showToast(`连通成功：${list.length} 个可用模型`);
    } catch (e) {
      showToast(`连通失败：${extractError(e)}`);
    } finally {
      setTesting(false);
    }
  };

  /** 仅保存提示词（不动端点配置） */
  const savePrompts = async () => {
    setSavingPrompts(true);
    try {
      await api.saveAiConfig(JSON.stringify(cfg));
      showToast('提示词已保存');
    } catch {
      showToast('保存失败');
    } finally {
      setSavingPrompts(false);
    }
  };

  const modelOptions = (models.length > 0 ? models : [cfg.model || '（先测试连通性）'])
    .map((m) => ({ value: m, label: m }));

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
            onChange={(v) => setCfg((c) => ({ ...c, model: v }))}
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

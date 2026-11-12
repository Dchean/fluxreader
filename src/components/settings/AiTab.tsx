import { useEffect, useRef, useState } from 'react';
import { enqueueAiConfigWrite, useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { Icons } from '../icons';
import { FluxDropdown, SettingCard } from '../primitives';
import { AI_PRESETS, DEFAULT_PROMPTS, type AiConfigState } from './shared';
import { mergeConfigFields, mergePromptsOnly } from './aiConfig';

/* ---------- TAB 5: AI服务 ---------- */

const ENDPOINT_FIELDS = ['preset', 'baseUrl', 'apiKey', 'model'] as const;
const PROMPT_FIELDS = ['summaryPrompt', 'translatePrompt'] as const;

function parseSavedConfig(raw: string | null): Partial<AiConfigState> {
  if (raw === null) return {};
  const value: unknown = JSON.parse(raw);
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('AI 配置格式无效');
  const saved = value as Record<string, unknown>;
  for (const key of [...ENDPOINT_FIELDS, ...PROMPT_FIELDS]) {
    if (saved[key] !== undefined && typeof saved[key] !== 'string') throw new Error('AI 配置格式无效');
  }
  return saved as Partial<AiConfigState>;
}

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
  const [loadError, setLoadError] = useState('');
  const [loadAttempt, setLoadAttempt] = useState(0);
  /* 初始回读只拥有尚未被用户意图接管的字段；编辑和显式操作均会接管。
     接管直到卸载才结束，成功/失败不能把字段重新交给更早的初始快照。
     Note: .agents/notes/implemented/bug-fix/2026-11-01-AI配置读改写卡内串行化.md */
  const cfgRef = useRef(cfg);
  const claimedFields = useRef(new Set<keyof AiConfigState>());
  const endpointRevision = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const claimFields = (fields: readonly (keyof AiConfigState)[]) => {
    for (const key of fields) claimedFields.current.add(key);
  };
  const showFeedback = (message: string) => {
    if (mounted.current) showToast(message);
  };
  const changeDraft = (patch: Partial<AiConfigState>) => {
    claimFields(Object.keys(patch) as (keyof AiConfigState)[]);
    if (ENDPOINT_FIELDS.some((key) => key in patch)) endpointRevision.current++;
    if (['preset', 'baseUrl', 'apiKey'].some((key) => key in patch)) setModels([]);
    cfgRef.current = { ...cfgRef.current, ...patch };
    setCfg(cfgRef.current);
  };

  /* 重挂载先等待此前已授权写入的前沿。仅空屏障入写链，慢配置读取本身不占链，
     当前页面仍可编辑、测试和保存提示词；迟到响应统一按字段所有权决定资格。 */
  useEffect(() => {
    let active = true;
    const frontier = enqueueAiConfigWrite(async () => {});
    void frontier.then(() => active ? api.getAiConfig() : null).then((raw) => {
      if (!active) return;
      const saved = parseSavedConfig(raw);
      const next = { ...cfgRef.current };
      for (const key of Object.keys(next) as (keyof AiConfigState)[]) {
        if (!claimedFields.current.has(key) && typeof saved[key] === 'string') next[key] = saved[key];
      }
      if (ENDPOINT_FIELDS.some((key) => next[key] !== cfgRef.current[key])) {
        endpointRevision.current++;
        setModels([]);
      }
      cfgRef.current = next;
      setCfg(next);
      setLoadError('');
    }).catch((error: unknown) => {
      if (active) setLoadError(`AI 配置读取失败：${extractError(error)}`);
    });
    return () => { active = false; };
  }, [loadAttempt]);

  const applyPreset = (p: string) => {
    const preset = AI_PRESETS[p];
    changeDraft({
      preset: p,
      baseUrl: p === 'custom' ? cfgRef.current.baseUrl : preset.url,
      model: p === 'custom' ? cfgRef.current.model : preset.model,
    });
  };

  /** 测试连通性（/models）→ 拉模型列表 → 保存配置 */
  const testAndSave = async () => {
    if (!cfg.baseUrl.trim()) { showFeedback('请填写 Base URL'); return; }
    if (!cfg.apiKey.trim()) { showFeedback('请填写 API Key'); return; }
    claimFields(ENDPOINT_FIELDS);
    const tested = {
      preset: cfg.preset, baseUrl: cfg.baseUrl.trim(), apiKey: cfg.apiKey.trim(), model: cfg.model,
    };
    const revision = endpointRevision.current;
    const unchanged = () => endpointRevision.current === revision;
    const changedMessage = '配置已更改，请重新测试并保存';
    setTesting(true);
    try {
      /* 整个入口（含连通性测试）入队：串行化判据是「点击顺序」。若只把落库段
         入队，后点先测完的写入会先执行，仍会乱序覆盖。
         落库只重放端点四字段（不再整包快照 stringify）——测试+保存不应
         顺手覆盖并发中的提示词/模型写入；写时重读库内最新配置合并。 */
      await enqueueAiConfigWrite(async () => {
        if (!unchanged()) { showFeedback(changedMessage); return; }
        const list = await api.aiListModels(tested.baseUrl, tested.apiKey);
        if (!list) { showFeedback('演示模式无法测试'); return; }
        if (!unchanged()) { showFeedback(changedMessage); return; }
        const persisted = await api.getAiConfig();
        parseSavedConfig(persisted);
        if (!unchanged()) { showFeedback(changedMessage); return; }
        await api.saveAiConfig(mergeConfigFields(persisted, tested));
        if (!mounted.current) return;
        if (!unchanged()) { showFeedback('已保存测试配置；当前更改尚未保存'); return; }
        setModels(list);
        showFeedback(`连通成功：${list.length} 个可用模型`);
      });
    } catch (e) {
      showFeedback(`连通失败：${extractError(e)}`);
    } finally {
      if (mounted.current) setTesting(false);
    }
  };

  /** 仅保存提示词（不动端点配置）：先读回库里的现有配置，只覆盖两个提示词字段。
      P2-4：此前直接把表单整份快照落库，等于把端点配置（含未确认可用的
      API Key）一起保存，与按钮语义相反。 */
  const savePrompts = async () => {
    claimFields(PROMPT_FIELDS);
    setSavingPrompts(true);
    try {
      await enqueueAiConfigWrite(async () => {
        const persisted = await api.getAiConfig();
        parseSavedConfig(persisted);
        await api.saveAiConfig(mergePromptsOnly(persisted, {
          summaryPrompt: cfgRef.current.summaryPrompt,
          translatePrompt: cfgRef.current.translatePrompt,
        }));
      });
      showFeedback('提示词已保存');
    } catch {
      showFeedback('保存失败');
    } finally {
      if (mounted.current) setSavingPrompts(false);
    }
  };

  const modelOptions = (models.length > 0 ? models : [cfg.model || '（先测试连通性）'])
    .map((m) => ({ value: m, label: m }));

  /** 切换模型即落库（P2-4 前半）：只在连通性测试成功后模型下拉才出现，此时库里
      已有测试时保存的端点配置；此前改模型只改本地表单，不点「测试连通性并保存」
      就不生效（后端 load_ai_config 读的是库里的 model）——用户以为换了模型，
      实际仍用旧模型。这里只覆盖 model 一个字段，不动其它端点配置。 */
  const selectModel = async (v: string) => {
    changeDraft({ model: v });
    try {
      await enqueueAiConfigWrite(async () => {
        const persisted = await api.getAiConfig();
        parseSavedConfig(persisted);
        await api.saveAiConfig(mergeConfigFields(persisted, { model: v }));
      });
      showFeedback(`已切换模型：${v}`);
    } catch (e) {
      showFeedback(`模型保存失败：${extractError(e)}`);
    }
  };

  return (
    <>
      {loadError && (
        <div role="alert" className="settings-action-row">
          <span>{loadError}</span>
          <button className="toggle-action-btn" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>重试读取</button>
        </div>
      )}
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
          aria-label="API Base URL"
          className="setting-input"
          placeholder="https://your-newapi.example.com/v1"
          value={cfg.baseUrl}
          onChange={(e) => changeDraft({ baseUrl: e.target.value })}
        />
      </SettingCard>
      <SettingCard title="API Key" desc="密钥仅存本地 SQLite">
        <input
          type="password"
          aria-label="API Key"
          className="setting-input"
          placeholder="sk-…"
          value={cfg.apiKey}
          onChange={(e) => changeDraft({ apiKey: e.target.value })}
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
            aria-label="当前使用模型"
            className="setting-input"
            placeholder="模型名（测试后可选）"
            value={cfg.model}
            onChange={(e) => changeDraft({ model: e.target.value })}
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
        <div className="prompt-editor-title">摘要提示词</div>
        <textarea
          aria-label="摘要提示词"
          className="setting-prompt-textarea"
          value={cfg.summaryPrompt}
          onChange={(e) => changeDraft({ summaryPrompt: e.target.value })}
        />
      </div>
      <div className="prompt-editor-card">
        <div className="prompt-editor-title">翻译提示词</div>
        <textarea
          aria-label="翻译提示词"
          className="setting-prompt-textarea"
          value={cfg.translatePrompt}
          onChange={(e) => changeDraft({ translatePrompt: e.target.value })}
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

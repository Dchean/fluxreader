import { useEffect, useRef, useState } from 'react';
import { useAppStore, CONTENT_LAYOUTS, LAYOUT_NAMES } from '../store';
import { api } from '../lib/api';
import { Icons, LayoutIcon } from './icons';
import { FluxDropdown, Switch, SettingCard, ModalOverlay, ConfirmDialog } from './primitives';
import type { ContentLayoutType } from '../types';
import { openExternal } from '../lib/external';

/* ============================================================
   设置中心 —— 沉浸式双栏布局，左侧导航 8 页签
   ============================================================ */

const TAB_META: { id: string; title: string; subtitle: string; icon: () => React.ReactElement }[] = [
  { id: 'general', title: '通用', subtitle: '订阅源刷新、默认行为、启动方式', icon: Icons.settings },
  { id: 'appearance', title: '外观', subtitle: '深浅主题模式、全局调色盘方案', icon: Icons.appearance },
  { id: 'reading', title: '阅读', subtitle: '正文字体、字号、版面、打开方式', icon: Icons.article },
  { id: 'feeds', title: '订阅', subtitle: '分类管理、内容布局绑定、AI规则', icon: Icons.rss },
  { id: 'ai', title: 'AI服务', subtitle: '模型端点配置、连通性探测、自定义提示词', icon: Icons.spark },
  { id: 'sync', title: '同步', subtitle: 'Miniflux API 连接、双向增量同步', icon: Icons.refresh },
  { id: 'shortcuts', title: '快捷键', subtitle: '全键盘导航流转、全局指令', icon: Icons.keyboard },
  { id: 'about', title: '关于', subtitle: '客户端版本信息、底层架构', icon: Icons.info },
];

const PALETTES = [
  { id: 'blue', name: '幻境蓝', color: '#4880c8' },
  { id: 'zinc', name: '锌灰', color: '#6b7280' },
  { id: 'purple', name: '堇紫', color: '#7873b8' },
  { id: 'emerald', name: '翡翠', color: '#419e79' },
  { id: 'terracotta', name: '赤陶', color: '#c46d54' },
] as const;

const FONT_OPTIONS = [
  { value: "'Plus Jakarta Sans', -apple-system, 'Segoe UI', sans-serif", label: 'Plus Jakarta Sans (默认)' },
  { value: "'Microsoft YaHei', 'PingFang SC', sans-serif", label: '微软雅黑 / 苹方 (无衬线)' },
  { value: 'Georgia, Cambria, "Times New Roman", serif', label: 'Georgia (优雅衬线)' },
  { value: "'Songti SC', SimSun, 'Source Han Serif SC', serif", label: '宋体 / 思源宋体 (中文衬线)' },
  { value: "'JetBrains Mono', Consolas, monospace", label: 'JetBrains Mono (等宽代码)' },
];

const LAYOUT_OPTIONS = CONTENT_LAYOUTS.map((l) => {
  const Icon = LayoutIcon[l];
  return { value: l as string, label: LAYOUT_NAMES[l], icon: <Icon /> };
});

export function SettingsModal() {
  const settingsOpen = useAppStore((s) => s.settingsOpen);
  const settingsTab = useAppStore((s) => s.settingsTab);
  const closeSettings = useAppStore((s) => s.closeSettings);
  const switchSettingsTab = useAppStore((s) => s.switchSettingsTab);

  const meta = TAB_META.find((t) => t.id === settingsTab) ?? TAB_META[0];

  return (
    <ModalOverlay open={settingsOpen} onClose={closeSettings}>
      <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
        {/* 左侧导航 */}
        <div className="settings-sidebar">
          <div className="settings-sidebar-header">
            <span className="settings-title-text">设置</span>
            <span className="kbd-tag">Ctrl+,</span>
          </div>
          <nav className="settings-nav">
            {TAB_META.map((m) => (
              <button
                key={m.id}
                className={`settings-nav-item ${settingsTab === m.id ? 'active' : ''}`}
                onClick={() => switchSettingsTab(m.id)}
              >
                <m.icon />
                <span>{m.title}</span>
              </button>
            ))}
          </nav>
          <div className="settings-sidebar-footer">FluxReader 0.7.5</div>
        </div>

        {/* 右侧内容区 */}
        <div className="settings-content-area">
          <div className="settings-content-header">
            <h2 className="settings-pane-title">{meta.title}</h2>
            <div className="settings-pane-header-right">
              <span className="settings-pane-subtitle">{meta.subtitle}</span>
              <button className="win-btn close" onClick={closeSettings} title="关闭设置 (Esc)">✕</button>
            </div>
          </div>

          <div className="settings-content-pane">
            {settingsTab === 'general' && <GeneralTab />}
            {settingsTab === 'appearance' && <AppearanceTab />}
            {settingsTab === 'reading' && <ReadingTab />}
            {settingsTab === 'feeds' && <FeedsTab />}
            {settingsTab === 'ai' && <AiTab />}
            {settingsTab === 'sync' && <SyncTab />}
            {settingsTab === 'shortcuts' && <ShortcutsTab />}
            {settingsTab === 'about' && <AboutTab />}
          </div>
        </div>
      </div>
    </ModalOverlay>
  );
}

/* ---------- TAB 1: 通用 ---------- */

/** 自启动开关：真实读写注册表（tauri-plugin-autostart），设置值只做镜像 */
function AutoStartSwitch() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const showToast = useAppStore((s) => s.showToast);
  const [regEnabled, setRegEnabled] = useState<boolean | null>(null);

  /* 挂载时读注册表真值（可能与镜像不一致：用户在任务管理器里改过） */
  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    void import('@tauri-apps/plugin-autostart')
      .then(({ isEnabled }) => isEnabled())
      .then(setRegEnabled)
      .catch(() => setRegEnabled(null));
  }, []);

  const toggle = async (v: boolean) => {
    updateSettings({ autoStart: v });
    try {
      const { enable, disable } = await import('@tauri-apps/plugin-autostart');
      if (v) await enable();
      else await disable();
      setRegEnabled(v);
    } catch {
      showToast('自启动设置失败');
      setRegEnabled(!v);
    }
  };

  return (
    <SettingCard title="开机自启动" desc="登录系统后自动在后台启动 FluxReader">
      <Switch checked={regEnabled ?? settings.autoStart} onChange={(v) => void toggle(v)} />
    </SettingCard>
  );
}

function GeneralTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">刷新</div>
      <SettingCard title="自动刷新" desc="后台定期检查订阅源更新">
        <Switch checked={settings.autoRefresh} onChange={(v) => updateSettings({ autoRefresh: v })} />
      </SettingCard>
      <SettingCard title="刷新间隔" desc="时间越短，电池消耗越大">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={5}
            max={120}
            value={settings.refreshInterval}
            className="range-input"
            onChange={(e) => updateSettings({ refreshInterval: Number(e.target.value) })}
          />
          <span style={{ width: 50 }}>{settings.refreshInterval} 分钟</span>
        </div>
      </SettingCard>

      <div className="settings-group-title">已读行为</div>
      <SettingCard title="打开文章时标为已读" desc="点击选中文章后立即更新本地已读状态">
        <Switch checked={settings.markReadOnOpen} onChange={(v) => updateSettings({ markReadOnOpen: v })} />
      </SettingCard>
      <SettingCard title="滚动到底部时标为已读" desc="正文滚至末尾才算读过，适合深度阅读">
        <Switch checked={settings.markReadOnScrollBottom} onChange={(v) => updateSettings({ markReadOnScrollBottom: v })} />
      </SettingCard>
      <SettingCard title="滚动出列表区域时标为已读" desc="卡片滚出时间流上沿即视为已浏览">
        <Switch checked={settings.markReadOnScrollOut} onChange={(v) => updateSettings({ markReadOnScrollOut: v })} />
      </SettingCard>

      <div className="settings-group-title">启动</div>
      <AutoStartSwitch />
      <SettingCard title="启动时打开" desc="下次打开应用时默认进入的视图">
        <FluxDropdown
          width={130}
          value={settings.startupView}
          onChange={(v) => updateSettings({ startupView: v })}
          options={[
            { value: 'unread', label: '未读' },
            { value: 'all', label: '全部' },
            { value: 'today', label: '今天' },
            { value: 'article', label: '文章' },
          ]}
        />
      </SettingCard>
      <SettingCard title="启动时隐藏已读" desc="仅展示未读流内容">
        <Switch checked={settings.hideReadOnStartup} onChange={(v) => updateSettings({ hideReadOnStartup: v })} />
      </SettingCard>
    </>
  );
}

/* ---------- TAB 2: 外观 ---------- */

function AppearanceTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">主题模式</div>
      <div className="theme-mode-grid">
        {([
          { id: 'light', label: '☀️ 浅色模式' },
          { id: 'dark', label: '🌙 深色模式' },
          { id: 'auto', label: '💻 跟随系统' },
        ] as const).map((m) => (
          <button
            key={m.id}
            className={`toggle-action-btn theme-mode-btn ${settings.themeMode === m.id ? 'theme-active' : ''}`}
            onClick={() => updateSettings({ themeMode: m.id })}
          >
            {m.label}
          </button>
        ))}
      </div>

      <div className="settings-group-title">全局配色方案</div>
      <div className="palette-swatches-grid">
        {PALETTES.map((p) => (
          <button
            key={p.id}
            className={`palette-card-btn ${settings.palette === p.id ? 'active' : ''}`}
            onClick={() => updateSettings({ palette: p.id })}
          >
            <div className="palette-dot" style={{ background: p.color }} />
            <span className="palette-name">{p.name}</span>
          </button>
        ))}
      </div>
    </>
  );
}

/* ---------- TAB 3: 阅读 ---------- */

function ReadingTab() {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);

  return (
    <>
      <div className="settings-group-title">字体</div>
      <SettingCard title="正文字体" desc="选择阅读器正文渲染字体家族">
        <FluxDropdown
          width={200}
          value={settings.fontFamily}
          onChange={(v) => updateSettings({ fontFamily: v })}
          options={FONT_OPTIONS}
        />
      </SettingCard>
      <SettingCard title="字号" desc="调整正文基础显示大小">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={13}
            max={24}
            value={settings.fontSize}
            className="range-input"
            onChange={(e) => updateSettings({ fontSize: Number(e.target.value) })}
          />
          <span style={{ width: 45 }}>{settings.fontSize}px</span>
        </div>
      </SettingCard>
      <SettingCard title="行高" desc="调整正文段落行间距比例">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={130}
            max={240}
            value={settings.lineHeight}
            className="range-input"
            onChange={(e) => updateSettings({ lineHeight: Number(e.target.value) })}
          />
          <span style={{ width: 45 }}>{settings.lineHeight}%</span>
        </div>
      </SettingCard>

      <div className="settings-group-title">版面</div>
      <SettingCard title="正文最大宽度" desc="限制单行文本长度以优化可读性">
        <div className="range-slider-wrap">
          <input
            type="range"
            min={560}
            max={1100}
            step={20}
            value={settings.maxWidth}
            className="range-input"
            onChange={(e) => updateSettings({ maxWidth: Number(e.target.value) })}
          />
          <span style={{ width: 55 }}>{settings.maxWidth}px</span>
        </div>
      </SettingCard>
      <SettingCard title="显示预计阅读时间" desc="在文章信息栏显示估算阅读时长">
        <Switch checked={settings.showReadTime} onChange={(v) => updateSettings({ showReadTime: v })} />
      </SettingCard>

      <div className="settings-group-title">打开方式</div>
      <SettingCard title="默认打开方式" desc="遇到部分未提供全文的 RSS 订阅源时自动执行正文提取">
        <FluxDropdown
          width={140}
          value={settings.defaultOpenMode}
          onChange={(v) => updateSettings({ defaultOpenMode: v as 'rss' | 'fulltext' })}
          options={[
            { value: 'rss', label: 'RSS 正文' },
            { value: 'fulltext', label: '自动全文' },
          ]}
        />
      </SettingCard>
      <SettingCard title="智能去重" desc="同一篇文章被多个订阅源推送时只保留最先入库的一份">
        <Switch checked={settings.smartDedup} onChange={(v) => updateSettings({ smartDedup: v })} />
      </SettingCard>
    </>
  );
}

/* ---------- TAB 4: 订阅 ----------
   交互结构：
   - 分类头点击 → toggleSettingsCatCollapse（store 已补齐该 action）
   - 头部右侧控件区整体 stopPropagation，下拉/开关/按钮不会触发折叠
   - 删除分类/删除订阅源共用同一形态按钮（trash 图标 + danger 文字）
     与同一个 ConfirmDialog 二次确认；确认后执行、自动关闭
   ---------- */

/** 待确认的删除目标（null = 无确认弹窗） */
interface PendingDelete {
  kind: 'category' | 'feed';
  catId: string;
  catName: string;
  feedId?: string;
  feedName?: string;
}

function FeedsTab() {
  const categories = useAppStore((s) => s.categories);
  const openNewCategoryModal = useAppStore((s) => s.openNewCategoryModal);
  const openAddFeedModal = useAppStore((s) => s.openAddFeedModal);
  const deleteCategory = useAppStore((s) => s.deleteCategory);
  const deleteFeed = useAppStore((s) => s.deleteFeed);
  const updateCatLayout = useAppStore((s) => s.updateCatLayout);
  const updateFeedLayout = useAppStore((s) => s.updateFeedLayout);
  const toggleCatSummary = useAppStore((s) => s.toggleCatSummary);
  const toggleCatTranslate = useAppStore((s) => s.toggleCatTranslate);
  const toggleFeedSummary = useAppStore((s) => s.toggleFeedSummary);
  const toggleFeedTranslate = useAppStore((s) => s.toggleFeedTranslate);
  const toggleSettingsCatCollapse = useAppStore((s) => s.toggleSettingsCatCollapse);
  const showToast = useAppStore((s) => s.showToast);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const fileInputRef = useRef<HTMLInputElement>(null);

  /* OPML 导入：<input file> 读文本 → 后端解析入库 → 重载 */
  const handleOpmlImport = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ''; // 允许重复选同一文件
    if (!file) return;
    void file.text().then((content) => api.opmlImport(content)).then((r) => {
      if (!r) { showToast('浏览器环境不支持导入'); return; }
      showToast(`OPML 导入完成：新增 ${r.imported} 个源${r.skipped > 0 ? `，跳过 ${r.skipped} 个已存在` : ''}`);
      return reloadFromBackend();
    }).catch((err) => showToast(`OPML 导入失败：${err}`));
  };

  /* OPML 导出：后端生成 → Blob 下载 */
  const handleOpmlExport = () => {
    void api.opmlExport().then((xml) => {
      if (!xml) { showToast('浏览器环境不支持导出'); return; }
      const blob = new Blob([xml], { type: 'text/xml' });
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = `fluxreader-subscriptions-${new Date().toISOString().slice(0, 10)}.opml`;
      a.click();
      URL.revokeObjectURL(a.href);
      showToast('OPML 导出完成');
    }).catch(() => showToast('OPML 导出失败'));
  };

  const [pending, setPending] = useState<PendingDelete | null>(null);

  const confirmDelete = () => {
    if (!pending) return;
    if (pending.kind === 'category') deleteCategory(pending.catId);
    else if (pending.feedId) deleteFeed(pending.catId, pending.feedId);
    setPending(null);
  };

  /* 确认弹窗文案：让用户明确知道删的是什么、影响多大 */
  const dialogText = pending
    ? pending.kind === 'category'
      ? {
          title: '删除订阅分类',
          message: `确定删除分类「${pending.catName}」吗？其下 ${categories.find((c) => c.id === pending.catId)?.feeds.length ?? 0} 个订阅源及全部本地条目将一并移除。`,
        }
      : {
          title: '删除订阅源',
          message: `确定删除订阅源「${pending.feedName}」吗？该源的全部本地条目将一并移除，同步服务端不受影响。`,
        }
    : null;

  return (
    <>
      <div className="feeds-tab-toolbar">
        <span className="feeds-tab-hint">点击分类名可展开/收起管理组内源</span>
        <input ref={fileInputRef} type="file" accept=".opml,.xml" style={{ display: 'none' }} onChange={handleOpmlImport} />
        <button className="toggle-action-btn" onClick={() => fileInputRef.current?.click()} title="从 OPML 文件导入订阅">
          导入 OPML
        </button>
        <button className="toggle-action-btn" onClick={handleOpmlExport} title="导出全部订阅为 OPML 文件">
          导出 OPML
        </button>
        <button className="toggle-action-btn btn-primary" onClick={openNewCategoryModal}>
          + 新建分类
        </button>
      </div>

      {categories.map((cat) => {
        const CatIcon = LayoutIcon[cat.layout];
        return (
          <div className="feed-group-mgr-box" key={cat.id}>
            <div className="group-mgr-header">
              <button
                className="group-mgr-title"
                onClick={() => toggleSettingsCatCollapse(cat.id)}
                aria-expanded={!cat.settingsCollapsed}
                title="展开/收起该分类"
              >
                <span className={`group-mgr-chevron ${cat.settingsCollapsed ? 'collapsed' : ''}`}>
                  <Icons.chevronDown />
                </span>
                <span>{cat.name}</span>
                <span className="group-mgr-feed-count">{cat.feeds.length} 个源</span>
              </button>

              <div className="group-mgr-controls" onClick={(e) => e.stopPropagation()}>
                <div className="group-mgr-layout-control">
                  <span className="layout-svg-badge">
                    {CatIcon && <CatIcon />}
                    布局:
                  </span>
                  <FluxDropdown
                    width={95}
                    value={cat.layout}
                    onChange={(v) => updateCatLayout(cat.id, v as ContentLayoutType)}
                    options={LAYOUT_OPTIONS}
                  />
                </div>

                <label className="mgr-checkbox-label">
                  <input
                    type="checkbox"
                    checked={cat.autoSummary}
                    onChange={(e) => toggleCatSummary(cat.id, e.target.checked)}
                    style={{ accentColor: 'var(--accent)' }}
                  />
                  自动摘要
                </label>
                <label className="mgr-checkbox-label">
                  <input
                    type="checkbox"
                    checked={cat.autoTranslate}
                    onChange={(e) => toggleCatTranslate(cat.id, e.target.checked)}
                    style={{ accentColor: 'var(--accent)' }}
                  />
                  自动翻译
                </label>

                <button className="toggle-action-btn" style={{ fontSize: 11 }} onClick={() => openAddFeedModal(cat.id)}>
                  + 添加源
                </button>
                <button
                  className="toggle-action-btn btn-danger-text"
                  style={{ fontSize: 11 }}
                  title="删除该分类及其全部订阅源"
                  onClick={() => setPending({ kind: 'category', catId: cat.id, catName: cat.name })}
                >
                  <Icons.trash />
                  <span>删除分类</span>
                </button>
              </div>
            </div>

            {!cat.settingsCollapsed && (
              <div className="group-mgr-body">
                {cat.feeds.length === 0 && (
                  <div className="group-mgr-empty">该分类暂无订阅源，点击「+ 添加源」创建</div>
                )}
                {cat.feeds.map((f) => (
                  <div className="group-mgr-child-row" key={f.id}>
                    <div className="group-mgr-feed-info">
                      <div className="group-mgr-feed-name">{f.name}</div>
                      <div className="group-mgr-feed-url">{f.url}</div>
                    </div>
                    <div className="group-mgr-feed-controls">
                      <FluxDropdown
                        width={115}
                        value={f.layout}
                        onChange={(v) => updateFeedLayout(cat.id, f.id, v)}
                        options={[
                          { value: 'inherit', label: '继承组' },
                          ...LAYOUT_OPTIONS,
                        ]}
                      />
                      <label className="mgr-checkbox-label">
                        <input
                          type="checkbox"
                          checked={f.autoSummary}
                          onChange={(e) => toggleFeedSummary(cat.id, f.id, e.target.checked)}
                          style={{ accentColor: 'var(--accent)' }}
                        />
                        摘要
                      </label>
                      <label className="mgr-checkbox-label">
                        <input
                          type="checkbox"
                          checked={f.autoTranslate}
                          onChange={(e) => toggleFeedTranslate(cat.id, f.id, e.target.checked)}
                          style={{ accentColor: 'var(--accent)' }}
                        />
                        翻译
                      </label>
                      <button
                        className="toggle-action-btn btn-danger-text"
                        style={{ fontSize: 11 }}
                        title="删除该订阅源"
                        onClick={() => setPending({ kind: 'feed', catId: cat.id, catName: cat.name, feedId: f.id, feedName: f.name })}
                      >
                        <Icons.trash />
                        <span>删除</span>
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        );
      })}

      <ConfirmDialog
        open={pending !== null}
        title={dialogText?.title ?? ''}
        message={dialogText?.message ?? ''}
        confirmText="确认删除"
        onConfirm={confirmDelete}
        onCancel={() => setPending(null)}
      />
    </>
  );
}

/* ---------- TAB 5: AI服务 ---------- */

/** 预设 → 默认 base_url + 默认模型（custom = newapi 等任意 OpenAI 兼容中转） */
const AI_PRESETS: Record<string, { url: string; model: string; label: string }> = {
  deepseek: { url: 'https://api.deepseek.com', model: 'deepseek-chat', label: 'DeepSeek 官方 API' },
  openai: { url: 'https://api.openai.com/v1', model: 'gpt-4.1-mini', label: 'OpenAI 官方 API' },
  glm: { url: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash', label: '智谱 BigModel (GLM)' },
  custom: { url: '', model: '', label: '自定义 / newapi 中转' },
};

interface AiConfigState {
  preset: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  summaryPrompt: string;
  translatePrompt: string;
}

const DEFAULT_PROMPTS = {
  summary: '你是一名资讯编辑。请用简洁的中文总结这篇文章，输出 3-5 个要点，每个要点一行，以 - 开头。不要重复文章标题。',
  translate: '你是一名专业译者。请把用户提供的 HTML 片段翻译成简体中文：保留所有 HTML 标签和属性原样不动，只翻译标签内的文本内容。直接输出翻译后的 HTML，不要任何解释或代码块包裹。',
};

function AiTab() {
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
      if (!list) { showToast('浏览器环境无法测试'); return; }
      setModels(list);
      await api.saveAiConfig(JSON.stringify(cfg));
      showToast(`连通成功：${list.length} 个可用模型`);
    } catch (e) {
      showToast(`连通失败：${e}`);
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
          placeholder="sk-..."
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

/* ---------- TAB 6: 同步 ---------- */

function SyncTab() {
  const showToast = useAppStore((s) => s.showToast);
  const dataMode = useAppStore((s) => s.dataMode);
  const reloadFromBackend = useAppStore((s) => s.reloadFromBackend);
  const [endpoint, setEndpoint] = useState('');
  const [token, setToken] = useState('');
  const [connected, setConnected] = useState(false);
  const [lastSync, setLastSync] = useState(0);
  const [busy, setBusy] = useState(false);

  /* 打开设置时读取当前连接状态 */
  useEffect(() => {
    if (dataMode !== 'tauri') return;
    void api.syncStatus().then((st) => {
      if (!st) return;
      setConnected(st.connected);
      setLastSync(st.last_sync);
      if (st.connected && st.endpoint) setEndpoint(st.endpoint);
    });
  }, [dataMode]);

  const doConnect = async () => {
    if (!endpoint.trim() || !token.trim()) {
      showToast('请填写 Endpoint 和 API Token');
      return;
    }
    setBusy(true);
    try {
      const msg = await api.syncConnect(endpoint.trim(), token.trim());
      /* 首次连接拉下来的新源立即直连抓取一次，条目马上可见 */
      await api.refreshAllFeeds().catch(() => null);
      await reloadFromBackend();
      setConnected(true);
      showToast(msg ?? '已连接 Miniflux');
    } catch (e) {
      showToast(`连接失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const doDisconnect = async () => {
    setBusy(true);
    try {
      await api.syncDisconnect();
      setConnected(false);
      setToken('');
      showToast('已断开连接（本地数据保留）');
    } finally {
      setBusy(false);
    }
  };

  if (dataMode !== 'tauri') {
    return (
      <>
        <div className="settings-group-title">Miniflux 配置</div>
        <SettingCard title="浏览器开发模式" desc="同步功能需要运行在 Tauri 客户端内（npm run tauri dev）">
          <span className="about-arch-tag">Mock 模式</span>
        </SettingCard>
      </>
    );
  }

  return (
    <>
      <div className="settings-group-title">Miniflux 配置</div>
      <SettingCard
        title="连接状态"
        desc={connected
          ? `已连接 · 上次同步 ${lastSync > 0 ? new Date(lastSync * 1000).toLocaleString() : '从未'}`
          : '未连接（客户端可独立使用：直连抓取、阅读、收藏均正常）'}
      >
        <span className={`about-arch-tag ${connected ? '' : ''}`}>{connected ? '已连接' : '未连接'}</span>
      </SettingCard>
      <SettingCard title="Miniflux 服务端 Endpoint" desc="例如 https://reader.example.com">
        <input
          type="text"
          className="setting-input"
          placeholder="https://reader.example.com"
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
        />
      </SettingCard>
      <SettingCard title="API Token" desc="Miniflux 设置 → API Keys 生成，用于双向同步已读/收藏/订阅">
        <input
          type="password"
          className="setting-input"
          placeholder="X-Auth-Token"
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
      </SettingCard>
      <div className="settings-action-row">
        <button
          className="toggle-action-btn btn-primary"
          disabled={busy}
          onClick={() => void doConnect()}
        >
          {busy ? '同步中...' : '测试连接并立即同步'}
        </button>
        {connected && (
          <button className="toggle-action-btn" disabled={busy} onClick={() => void doDisconnect()}>
            断开连接
          </button>
        )}
      </div>
      <div className="mini-dialog-hint" style={{ marginTop: 8 }}>
        连接后：本地已读/收藏/订阅变更双向同步；直连失败的源自动从 Miniflux 兜底拉取条目。
        不连接也完全可用 —— 客户端直连源站抓取（第一优先级）。
      </div>
    </>
  );
}

/* ---------- TAB 7: 快捷键 ---------- */

function ShortcutsTab() {
  const rows = [
    ['Ctrl + K', '打开全局搜索', '全局'],
    ['Ctrl + ,', '打开设置中心', '全局'],
    ['J / K', '上下切换卡片', '时间流'],
    ['S', '收藏/取消收藏', '阅读器'],
    ['M', '切换已读/未读状态', '阅读器'],
    ['Esc', '清除选中/关闭浮层', '全局'],
  ];
  return (
    <table className="shortcuts-table">
      <tbody>
        {rows.map(([key, action, scope]) => (
          <tr key={key}>
            <td style={{ padding: '8px 0' }}><span className="kbd-tag" style={{ marginLeft: 0 }}>{key}</span></td>
            <td>{action}</td>
            <td>{scope}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/* ---------- TAB 8: 关于 ---------- */

function AboutTab() {
  const [version, setVersion] = useState('');
  const [updateState, setUpdateState] = useState<'idle' | 'checking' | 'available' | 'upToDate' | 'failed'>('idle');
  const [latestInfo, setLatestInfo] = useState<{ version: string; url: string } | null>(null);
  const showToast = useAppStore((s) => s.showToast);

  useEffect(() => {
    let alive = true;
    import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => alive && setVersion(v))
      .catch(() => alive && setVersion('0.8.0'));
    return () => { alive = false; };
  }, []);

  const checkUpdate = async () => {
    if (updateState === 'checking') return;
    setUpdateState('checking');
    try {
      const res = await fetch('https://api.github.com/repos/Dchean/fluxreader/releases/latest');
      if (!res.ok) throw new Error(String(res.status));
      const data = (await res.json()) as { tag_name?: string; html_url?: string };
      const remote = (data.tag_name ?? '').replace(/^v/, '');
      if (!remote) throw new Error('empty tag');
      setLatestInfo({ version: remote, url: data.html_url ?? 'https://github.com/Dchean/fluxreader/releases' });
      setUpdateState(compareVersions(remote, version) > 0 ? 'available' : 'upToDate');
    } catch {
      setUpdateState('failed');
      showToast('检查更新失败，请稍后重试');
    }
  };

  return (
    <>
      <SettingCard title="客户端版本" desc={`FluxReader v${version || '…'} (Build 2026.08)`}>
        <span className="about-arch-tag">Tauri 2 + Rust + SQLite</span>
      </SettingCard>
      <SettingCard title="检查更新" desc="检测 GitHub Releases 上的最新版本">
        {updateState === 'checking' ? (
          <span className="about-update-hint">正在检查…</span>
        ) : updateState === 'available' && latestInfo ? (
          <button className="toggle-action-btn about-update-btn" onClick={() => void openExternal(latestInfo.url)}>
            v{latestInfo.version} 可用 · 前往下载
          </button>
        ) : (
          <button className="toggle-action-btn about-update-btn" onClick={() => void checkUpdate()}>
            检查更新
          </button>
        )}
        {updateState === 'upToDate' && <span className="about-update-hint">已是最新版本</span>}
        {updateState === 'failed' && <span className="about-update-hint">检查失败</span>}
      </SettingCard>
    </>
  );
}

/** semver 比较：返回 >0 表示 a 更新 */
function compareVersions(a: string, b: string): number {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}


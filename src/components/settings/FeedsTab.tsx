import { useRef, useState } from 'react';
import { useAppStore } from '../../store';
import { api, extractError } from '../../lib/api';
import { Icons } from '../icons';
import { FluxDropdown, SwitchInline, ConfirmDialog } from '../primitives';
import type { ContentLayoutType } from '../../types';
import { LAYOUT_OPTIONS } from './layoutOptions';
import { LAYOUT_NO_AI, type PendingDelete } from './shared';

/* ---------- TAB 4: 订阅 ----------
   交互结构：
   - 分类头点击 → toggleSettingsCatCollapse（store 已补齐该 action）
   - 头部右侧控件区整体 stopPropagation，下拉/开关/按钮不会触发折叠
   - 删除分类/删除订阅源共用同一形态按钮（trash 图标 + danger 文字）
     与同一个 ConfirmDialog 二次确认；确认后执行、自动关闭
   ---------- */

export function FeedsTab() {
  const categories = useAppStore((s) => s.categories);
  const openNewCategoryModal = useAppStore((s) => s.openNewCategoryModal);
  const openAddFeedModal = useAppStore((s) => s.openAddFeedModal);
  const openEditFeedModal = useAppStore((s) => s.openEditFeedModal);
  const openRenameCatModal = useAppStore((s) => s.openRenameCatModal);
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
      if (!r) { showToast('演示模式不支持导入'); return; }
      showToast(`OPML 导入完成：新增 ${r.imported} 个源${r.skipped > 0 ? `，跳过 ${r.skipped} 个已存在` : ''}`);
      return reloadFromBackend();
    }).catch((err) => showToast(`OPML 导入失败：${extractError(err)}`));
  };

  /* OPML 导出：后端生成 → Blob 下载 */
  const handleOpmlExport = () => {
    void api.opmlExport().then((xml) => {
      if (!xml) { showToast('演示模式不支持导出'); return; }
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
                <button
                  className="toggle-action-btn"
                  onClick={() => openAddFeedModal(cat.id)}
                  title="在该分类下添加订阅源"
                >
                  <Icons.plus />
                  <span>添加源</span>
                </button>

                <label
                  className="mgr-checkbox-label"
                  title="分类内新文章自动生成 AI 摘要"
                  style={LAYOUT_NO_AI.has(cat.layout) ? { display: 'none' } : undefined}
                >
                  <SwitchInline
                    compact
                    checked={cat.autoSummary}
                    onChange={(v) => toggleCatSummary(cat.id, v)}
                  />
                  摘要
                </label>
                <label
                  className="mgr-checkbox-label"
                  title="分类内新文章自动翻译正文"
                  style={LAYOUT_NO_AI.has(cat.layout) ? { display: 'none' } : undefined}
                >
                  <SwitchInline
                    compact
                    checked={cat.autoTranslate}
                    onChange={(v) => toggleCatTranslate(cat.id, v)}
                  />
                  翻译
                </label>

                <div className="group-mgr-layout-control">
                  <FluxDropdown
                    width={96}
                    value={cat.layout}
                    onChange={(v) => updateCatLayout(cat.id, v as ContentLayoutType)}
                    options={LAYOUT_OPTIONS}
                  />
                </div>

                <button
                  className="toggle-action-btn icon-btn"
                  title="重命名该分类"
                  onClick={() => openRenameCatModal(cat.id)}
                >
                  <Icons.edit />
                </button>
                <button
                  className="toggle-action-btn btn-danger-text"
                 
                  title="删除该分类及其全部订阅源"
                  onClick={() => setPending({ kind: 'category', catId: cat.id, catName: cat.name })}
                >
                  <Icons.trash />
                  <span>删除</span>
                </button>
              </div>
            </div>

            <div className={`group-mgr-body${cat.settingsCollapsed ? '' : ' open'}`}>
              {cat.feeds.length === 0 && (
                <div className="group-mgr-empty">该分类还没有订阅源</div>
              )}
              {cat.feeds.map((f) => {
                /* AI 开关只对使用 AI 的布局有意义（文章/通知/社交——翻译）；
                   画廊/播客布局下隐藏，避免无效开关误导 */
                const effLayout = f.layout === 'inherit' ? cat.layout : (f.layout as ContentLayoutType);
                const noAi = LAYOUT_NO_AI.has(effLayout);
                return (
                <div className="group-mgr-child-row" key={f.id}>
                  <div className="group-mgr-feed-info">
                    {f.favicon ? (
                      <img src={f.favicon} alt="" className="feed-favicon" referrerPolicy="no-referrer" />
                    ) : (
                      <span className="feed-favicon-fallback"><Icons.dot /></span>
                    )}
                    <div className="group-mgr-feed-text">
                      <div className="group-mgr-feed-name">{f.name}</div>
                      <div className="group-mgr-feed-url">{f.url}</div>
                    </div>
                  </div>
                  <div className="group-mgr-feed-controls">
                    <label
                      className="mgr-checkbox-label"
                      title="该源新文章自动生成 AI 摘要"
                      style={noAi ? { display: 'none' } : undefined}
                    >
                      <SwitchInline
                        compact
                        checked={f.autoSummary}
                        onChange={(v) => toggleFeedSummary(cat.id, f.id, v)}
                      />
                      摘要
                    </label>
                    <label
                      className="mgr-checkbox-label"
                      title="该源新文章自动翻译正文"
                      style={noAi ? { display: 'none' } : undefined}
                    >
                      <SwitchInline
                        compact
                        checked={f.autoTranslate}
                        onChange={(v) => toggleFeedTranslate(cat.id, f.id, v)}
                      />
                      翻译
                    </label>
                    <FluxDropdown
                      width={96}
                      value={f.layout}
                      onChange={(v) => updateFeedLayout(cat.id, f.id, v)}
                      options={[
                        { value: 'inherit', label: '继承组' },
                        ...LAYOUT_OPTIONS,
                      ]}
                    />
                    <button
                      className="toggle-action-btn icon-btn"
                      title="编辑该订阅源（重命名/移动分类/布局/AI 开关）"
                      onClick={() => openEditFeedModal(f.id)}
                    >
                      <Icons.edit />
                    </button>
                    <button
                      className="toggle-action-btn btn-danger-text"

                      title="删除该订阅源"
                      onClick={() => setPending({ kind: 'feed', catId: cat.id, catName: cat.name, feedId: f.id, feedName: f.name })}
                    >
                      <Icons.trash />
                      <span>删除</span>
                    </button>
                  </div>
                </div>
                );
              })}
            </div>
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

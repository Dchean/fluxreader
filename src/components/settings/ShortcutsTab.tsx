/* ---------- TAB 7: 快捷键 ---------- */

export function ShortcutsTab() {
  /* TASK-100：快捷键提示统一紧凑加号形态（U1：Ctrl+K / Ctrl+, / Esc）；
     J/K 范围文案改准为「文章布局」（UI P2-2：跨布局 J/K 本轮不做，社交/画廊/
     播客/通知四种布局按 J/K 直接 return，不得宣传成整个时间流）；
     补 Space 行（P3-6：播放器激活时播放/暂停，App.tsx 已实现）。 */
  const rows = [
    ['Ctrl+K', '打开全局搜索', '全局'],
    ['Ctrl+,', '打开设置中心', '全局'],
    ['J / K', '上下切换文章', '文章布局'],
    ['Space', '播放 / 暂停（播放器激活时）', '播放器'],
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

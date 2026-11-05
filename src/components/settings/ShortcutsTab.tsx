/* ---------- TAB 7: 快捷键 ---------- */

export function ShortcutsTab() {
  /* TASK-100：快捷键提示统一紧凑加号形态（U1：Ctrl+K / Ctrl+, / Esc）；
     补 Space 行（P3-6：播放器激活时播放/暂停，App.tsx 已实现）。
     J/K 现已覆盖五种布局（选中即打开、焦点同步）；文案不再声明画廊例外。 */
  const rows = [
    ['Ctrl+K', '打开全局搜索', '全局'],
    ['Ctrl+,', '打开设置中心', '全局'],
    ['J / K', '上下切换选中条目', '全部内容布局'],
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

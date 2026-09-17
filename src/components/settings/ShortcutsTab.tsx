/* ---------- TAB 7: 快捷键 ---------- */

export function ShortcutsTab() {
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

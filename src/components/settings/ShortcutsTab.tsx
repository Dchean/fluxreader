/* ---------- TAB 7: 快捷键 ---------- */

export function ShortcutsTab() {
  /* TASK-100：快捷键提示统一紧凑加号形态（U1：Ctrl+K / Ctrl+, / Esc）；
     补 Space 行（P3-6：播放器激活时播放/暂停，App.tsx 已实现）。
     TASK-114 X3：J/K 从「仅文章布局」扩展到全部虚拟化布局（文章/社交/播客/
     通知，选中即打开），画廊非虚拟化不支持——范围文案随之改准并明示例外。 */
  const rows = [
    ['Ctrl+K', '打开全局搜索', '全局'],
    ['Ctrl+,', '打开设置中心', '全局'],
    ['J / K', '上下切换选中条目', '文章/社交/播客/通知（画廊不支持）'],
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

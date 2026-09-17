# TASK-047 基线（2026-09-17）：关闭态浮层可聚焦问题

采集于 TASK-045 完成之后（提交 `85eacf3` 之后），工作区干净。

## 缺陷现状（B-3，已实测）

**复核报告 B-3**（TASK-041/042 独立复核提出，TASK-043 报告 §5.7 复测确认）：

| 指标 | 实测值 |
| --- | --- |
| 关闭态设置弹窗内可 Tab 控件数 | **约 20 个** |
| `inert` | `false`（属性不存在） |
| `aria-hidden` | `null` |
| 隐藏方式 | 仅 `opacity: 0` + `pointer-events: none`（`.modal-overlay`，`base.css:1962`） |

根因见 UI 契约 §1：`ModalOverlay`（`primitives.tsx:298-318`）**无条件渲染** children，
只用 `open` 切类名。灯箱（`Overlays.tsx:314`）同模式。

`ConfirmDialog`（`primitives.tsx:279`）与 `ContextMenu`（`ContextMenu.tsx:77`）
已有 `if (!x) return null;`，**不受影响**。

## 前端基线（改动前，需保持一致）

本会话此前实跑（TASK-043/044/045 期间多次复现）：

```
npm run lint            => 0 warnings and 0 errors
npm run build           => ✓ built
npm run test:frontend   => 26/26 通过，退出码 0
```

`tools/frontend-regression.mjs` 为纯 Zustand 状态机测试，
grep `focus|querySelector|document\.|getComputedStyle|tabIndex|role=` **命中 0** ——
即该套件对可聚焦性**不能提供任何证据**，本次正确性只能靠实机 UI 证据。

## Rust 基线（本任务不改 Rust，仅作回归参照）

```
cargo test => 120 passed / 0 failed / 23 ignored
```

## Tab 序列基线（TASK-043 记录，本次不得改变）

默认全关状态下主视图 Tab 停靠点 **16 个**，顺序为：
窗口控件 → 侧栏搜索入口 → 视图四项 → 内容布局五项 → 工具栏。
其中除 `BODY` 外全部 `:focus-visible=true`、`outline: solid 1.75px`。

## 计量与行尾口径（本任务沿用，避免重犯）

- 行数用可信口径：LF 字节数 = Python `splitlines()` = .NET `ReadAllLines()`；
  **不用** PowerShell `Measure-Object -Line`（会少算，TASK-044 已记录）。
- 文本文件必须 **LF**：仓库 `.gitattributes` 为 `* text=auto eol=lf`。
  写文件须显式指定行尾——Python 文本模式在 Windows 上会把 `\n` 写成 `\r\n`
  （TASK-045 第 2 轮审查即因此判 FAIL，且三门禁对该缺陷完全不敏感）。

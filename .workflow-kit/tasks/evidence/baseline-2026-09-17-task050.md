# TASK-050 基线（2026-09-17）：SettingsModal.tsx 拆分前

采集于 TASK-049 验收之后（提交 `4d3ce36`），工作区干净。

## 可行性评估（本任务的核心结论）

`src/components/SettingsModal.tsx` **已经天然分解为 14 个顶层函数**，每个标签页/区块一个：

| 单元 | 行范围 | 行数 |
| --- | --- | --- |
| `SettingsModal`（外壳） | 54–130 | 77 |
| `AutoStartSwitch` | 131–165 | 35 |
| `GeneralTab` | 166–244 | 79 |
| `AppearanceTab` | 245–286 | 42 |
| `ReadingTab` | 287–383 | 97 |
| `FeedsTab` | 384–663 | **280** |
| `AiTab` | 664–832 | 169 |
| `SyncTab` | 833–1125 | **293** |
| `CacheCleanupSection` | 1126–1188 | 63 |
| `ConfigSyncSection` | 1189–1406 | 218 |
| `ShortcutsTab` | 1407–1431 | 25 |
| `SettingsSidebarFooter` | 1432–1446 | 15 |
| `AboutTab` | 1447–1503 | 57 |
| `compareVersions` | 1504–1513 | 10 |

各单元合计 1460 行 / 全文 1513 行（其余为 import 与模块级常量）。

**因此本任务是「纯移动」**：把既有函数搬到 `src/components/settings/` 的子模块，
不改 JSX/逻辑/文案。这与后端 `commands.rs`（TASK-044）同类，**可用「函数体逐字比对」机械证明等价**。

### 模块级共享物（需一个共享模块承载）

| 名称 | 行 | 用途 |
| --- | --- | --- |
| `TAB_META` | 14 | 标签页元数据 |
| `PALETTES` / `FONT_OPTIONS` / `LAYOUT_OPTIONS` | 25 / 36 / 44 | 外观与布局选项 |
| `LAYOUT_NO_AI` | 52 | 不使用 AI 的布局集合 |
| `PendingDelete`（interface） | 376 | 待删除项 |
| `AI_PRESETS` / `AiConfigState` / `DEFAULT_PROMPTS` | 643 / 650 / 659 | AI 配置 |
| `CACHE_PERIODS` | 1118 | 缓存周期选项 |

### 导入面（必须保持不变）

```
src/App.tsx:7    import { SettingsModal } from './components/SettingsModal';
src/App.tsx:317  <SettingsModal />
```

**唯一导出是 `export function SettingsModal()`**（`SettingsModal.tsx:54`）。
故拆分后 `src/components/SettingsModal.tsx` 必须继续导出它，且 `git diff HEAD -- src/App.tsx` 为空。

（`Overlays.tsx:8` 只在注释里提到 SettingsModal，不构成导入。）

## 为什么不需要组件测试库

- `tsconfig.test.json` 的 `include` 只含 `src/store.ts` / `mockData.ts` / `types.ts` / `lib/{api,format,external}.ts`
  ——**不含任何 `.tsx`**；
- `devDependencies` 只有 `@tauri-apps/cli`、`@types/*`、`@vitejs/plugin-react`、`oxlint`、`typescript`、`vite`
  ——**没有 jsdom / @testing-library/react / vitest**。

补组件测试需要**新增依赖**，而本任务 non_goals 明确禁止。替代方案（本任务采用）：

1. **函数体逐字等价证明**（脚本）——纯移动的等价性判据；
2. **`npm run lint` + `npm run build`**——`tsc -b` 会类型检查全部 JSX；
3. **CDP 实机冒烟**——真实 WebView2 下打开设置、逐个切换 8 个标签页、检查 console 错误、
   并验 2 处交互接线。这是**不引入依赖前提下**唯一能证明「拆分后仍渲染且可交互」的手段。

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors
npm run build         => ✓ built（含 tsc -b）
npm run test:frontend => 141/141 通过（既有 26 + TASK-048 新增 115），退出码 0
cargo test            => 120 passed / 0 failed / 23 ignored（本任务不改 Rust）
```

## 口径与行尾纪律（沿用前几次教训）

- 行数用可信口径（LF 字节数 / `splitlines()` / `ReadAllLines()`）；**不用** PowerShell `Measure-Object -Line`。
- 文本（含 `.tsx`）必须 **LF**；用 Python 写文件须显式指定行尾或走二进制模式
  （TASK-045 因文本模式写出 CRLF 被判 FAIL，且三门禁对该缺陷完全不敏感）。
- 台账改动必须在 `begin` **之前**完成（TASK-043 记录过、TASK-049 又犯过：
  `.workflow-kit/tasks/**` 属任务模板自带的 `protected_paths`）。

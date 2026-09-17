# TASK-049 基线（2026-09-17）：store.ts 拆分前

采集于 TASK-048 验收之后（提交 `f7c6bcd`），工作区干净。

## 拆分前的真实结构（这是本任务的关键约束）

| 行范围 | 内容 |
| --- | --- |
| 1–20 | imports |
| 21–83 | 第一段章节注释与模块级常量 |
| 84–146 | `export async function bootstrapGithubAuth()`（模块级函数，不在 store 内） |
| 147–170 | 第二段章节注释 |
| **171–1650** | **`export const useAppStore = create<AppState>((set, get) => ({ … }))`** ——整套状态与 action 都在**一个对象字面量**里 |
| 1655 | `export * from './store/selectors'` |
| 1657 | `export type { AppState, PodcastPlayerState, SettingsState, ToastMessage } from './store/…'` |

即：**不是「一堆独立函数分散在文件里」，而是一个巨型对象字面量**。
因此拆分手段是 **Zustand slice 模式**（把 `StateCreator` 切片再展开组合），
而不是像 Rust 侧 TASK-044/045 那样「把顶层 fn 搬到同 crate 的子模块」。

**这也意味着**：`get()`/`set()` 的交叉调用、slice 展开顺序、同名 key 覆盖，
都是本任务真正的风险点——不是「搬错文件」，而是「组合出行为差异」。

## 相关既有模块

| 文件 | 行数（PS 口径） | 说明 |
| --- | --- | --- |
| `src/store.ts` | 1474 | 真实 1657（可信口径） |
| `src/store/types.ts` | 262 | 已有：`AppState` / `SettingsState` 等类型 |
| `src/store/selectors.ts` | 151 | 已有：派生选择器，经 `export *` 再导出 |
| `src/types.ts` | 70 | 更上层的共享类型 |

## 基线结果（本会话实跑）

```
npm run lint          => 0 warnings and 0 errors
npm run build         => ✓ built（含 tsc -b）
npm run test:frontend => 141/141 通过（既有 26 + TASK-048 新增 115），退出码 0
                         连跑 4 次断言序列逐行一致
cargo test            => 120 passed / 0 failed / 23 ignored（本任务不改 Rust）
```

**该套件的可信度**：TASK-048 的独立审查者在 tsc 产物上做了 **9 组变异测试**
（去 seek 夹取、toast 上限改 5、分页恒 offset 0、去 anchor 代际守卫、folderId 置 null、
starredOnly 恒 false、translatingIds 改常量键、去游标竞态守卫、去视图守卫），
**9/9 全被杀死且失败项精确落在对应领域**。因此它不是「摆设式断言」。

## 必须**保持不变**的已知缺陷（D1–D5，属独立修复任务）

TASK-048 写测试时发现、**未修改实现**。本任务必须**原样保留**其现状行为
（把它们一起改掉会让「搬错了」与「改对了」无法区分）：

| # | 位置 | 现状行为 |
| --- | --- | --- |
| D1 | `store.ts:762-764` / `:515` | 半截文本存在时，AI toast「重试」会早退、不重发 |
| D2 | `store.ts:1030-1032` | 分页失败被静默吞掉（无 toast） |
| D3 | `store.ts:1012-1013` | 竞态丢弃分支不复位 `articlesLoading`（入口守卫 `:1007`） |
| D4 | `store.ts:1581` | 「启动时打开」白名单与 UI 选项不一致 |
| D5 | `store.ts:1551-1557` | `updateSettings` 无运行时校验 |

> **特别提醒**：D3 今天「自愈」只因为所有写 `articlesLimit` 的路径都顺手把
> `articlesLoading` 置了 false。切片迁移时若改变了这一耦合，D3 会从「隐性」变成「显性卡死」——
> 但那属于**行为变化**，应停下报告，而不是顺手修掉。

## 口径与行尾纪律（沿用前几次教训）

- 行数用可信口径（LF 字节数 / `splitlines()` / `ReadAllLines()`）；
  **不用** PowerShell `Measure-Object -Line`（会少算，TASK-044 已记录）。
- 文本文件必须 **LF**；用 Python 写文件须显式指定行尾或走二进制模式
  （TASK-045 因文本模式写出 CRLF 被判 FAIL，且三门禁对该缺陷完全不敏感）。
- 验收「单文件 ≤400 行」时，必须用上面口径复算，不能引用编辑器显示的软换行数。

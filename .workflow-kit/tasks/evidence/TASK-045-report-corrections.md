# TASK-045 报告更正说明（审查后补充，不改动已绑定证据）

独立审查（`TASK-045-review-report.json`）在 **PASS** 结论中附带指出
`TASK-045-split-report.md` 有 3 处不准确。经复核**全部成立**，本文件更正。
**刻意不改动原报告**——它已被该次审查的 evidence 绑定，事后编辑会触发
`review evidence changed after approval` 自锁（本项目已记录该缺陷，TASK-043/044 都踩过）。

## 更正 1：可见性放大是 **24 处声明**，不是「19 项」

原报告 §3 标题与小结写「19 项」，但同表内容已含 `PUSH_LOCK` / `PushPlan` / `PushStatus` 与字段，**自相矛盾**。

实测（去注释后按声明统计）：

| 文件 | `pub(super) fn` | 类型/静态 | 字段 |
| --- | --- | --- | --- |
| `credentials.rs` | 9 | 0 | 0 |
| `push.rs` | 3 | 3 | 2 |
| `subscriptions.rs` | 2 | 0 | 0 |
| `entries.rs` | 3 | 0 | 0 |
| `greader_pull.rs` | 1 | 0 | 0 |
| `fever_pull.rs` | 1 | 0 | 0 |
| **合计** | **19** | **3** | **2** |

**总计 24 处声明**（19 + 3 + 2）。

**成因**：我的证明脚本只按 `fn` 切分比对，因此只数到 19 个函数，
**漏掉了 3 个类型/静态（`PUSH_LOCK`、`PushPlan`、`PushStatus`）与 2 个字段（`status`、`stars`）**。
报告作者用了脚本的输出作为总数，而表格又手工补了类型与字段——两边口径不一致。

**审查者的独立判断（我认同）**：24 处**都属最小必要**。其中 `PushPlan` / `PushStatus`
虽不被别的文件点名，但被 Rust 规则强制——私有类型出现在 `pub(super)` 字段类型或
`pub(super)` fn 签名中会触发 `private_interfaces`，在 `-D warnings` 下升格为 error
（审查者用独立 rustc 实验复现）。故不构成过度放宽。

## 更正 2：`push_feeds` / `pull_feeds` 的使用者是 `phases`，**不是** `phases / entries`

原报告 §3 表把这两项的使用者写成「phases / entries」。实测引用点：

| 符号 | 定义处 | 真实引用处 |
| --- | --- | --- |
| `push_feeds` | `subscriptions.rs` | `phases.rs`（2 处） |
| `pull_feeds` | `subscriptions.rs` | `phases.rs`（2 处） |

`entries.rs` **未引用**二者（审查者独立核对一致，我亦复核）。
（`push.rs` / `mod.rs` 里各有 1 处出现，但均在**注释/文档**中，不是代码引用。）

**影响**：无。这两个符号**确实**需要放大（`phases.rs` 在用），只是使用者名单多写了一个文件。

## 更正 3：孤立章节横幅注释（已修，非报告文字问题）

原报告未提及：拆分后**原 4 个章节横幅注释成了孤立死注释**——它们描述的章节已搬到别的文件，
横幅却留在原处，其中 3 处成为文件**末尾**的孤立横幅。审查者列为非阻塞披露并建议顺手删。

| 位置 | 孤立横幅 |
| --- | --- |
| `sync/mod.rs` | 「凭据」（内容已搬到 `credentials.rs`） |
| `sync/credentials.rs` 尾部 | 「① Push：本地状态变更 → 后端（只推不拉）」 |
| `sync/push.rs` 尾部 | 「② Pull：远端 → 本地（订阅关系 + 状态 + 条目）」 |
| `sync/entries.rs` 尾部 | 「总入口」 |

**已删除**（纯注释，零功能/lint 影响）。删后清理候选 `32cae71d…`：
fmt exit 0、clippy exit 0、`cargo test` exit 0 且 120 passed / 0 failed / 23 ignored。

**为什么不留到「后续任务」**：独立审查建议「后续顺手删」，但把 4 行注释清理拆成独立任务
需要它自己的 verify + review 周期，成本与现在直接修完相同；且不留已知债。
另已核对 `commands/`（TASK-044 产物）**无**同类孤立横幅，故残留仅限 `sync/`。

## 教训

1. **数字必须标明统计口径**：我用「脚本输出的 fn 数」当成了「放大声明总数」，
   漏掉类型/静态/字段三类。这与 TASK-044 的行数口径错误是**同一类失误**——
   报数时不交代口径、也不交叉验算。
2. **表格与小结要一致**：§3 表格已列类型与字段，标题却写 19，属自相矛盾；
   审查者正是抓住这一不一致。
3. **引用关系要按代码验证**：写「谁在用」时应 grep 确认，不能凭模块职责推断
   （`entries` 处理条目、`push_feeds` 是订阅层，职责相邻但并无引用）。

## 更正 4：§1 行数表随横幅删除失准（未同步更新）

删掉 4 处孤立横幅后，原报告 §1 的行数表没有同步，正确值应为：

| 文件 | 报告原值 | 删横幅后 |
| --- | --- | --- |
| `sync/mod.rs` | 64 | **60** |
| `sync/credentials.rs` | 138 | **134** |
| `sync/push.rs` | 193 | **189** |
| `sync/entries.rs` | 289 | **285** |
| 其余 4 个 | 未变 | 未变 |

「最大单文件 289」应改为 **285**（验收 ≤400 仍成立）。
四个文件各删 4 行（横幅 3 行 + 1 空行），故差值均为 4，与实测一致。

## 更正 5：`mod.rs` 私有 glob 的理由应补强

原报告 §2 只说了「对无 pub 项的模块做 `pub use` 会被 rustc 判为
『glob 未重导出任何 pub 项』并告警」。审查者指出**更强的理由是验收约束**：

> 若改成显式 `pub use entries::{item_numeric_id, …}`，会让
> `crate::sync::item_numeric_id` 等**成为新增的对外路径**，直接违反验收第 2 条
> 「`crate::sync::<fn>` 的既有调用路径不变 / pub 面不扩张」。

即：**私有 glob 不是「掩盖告警」，而是唯一能同时满足「项可被兄弟模块使用」
与「不新增对外路径」的做法**。原报告把次要理由写成了全部理由。

## 更正 6：我在修复横幅时引入 CRLF 缺陷（审查判 FAIL，已修）

这是**我的实现缺陷**，不只是报告文字问题，如实记录：

**缺陷**：我用 Python 脚本清理 4 处孤立横幅时写了
`p.write_text("\n".join(out) + "\n", encoding="utf-8")`。
Python 文本模式写入在 Windows 上会把 `\n` 翻译成 `\r\n`，
于是这 4 个文件由 **LF 变成 CRLF**，与仓库 `.gitattributes` 的 `* text=auto eol=lf` 冲突。
实测：`mod.rs` CRLF=60、`credentials.rs` CRLF=134、`push.rs` CRLF=189、`entries.rs` CRLF=285，
各自等于其行数；而另外 4 个未被脚本重写的子模块仍为 LF。

**为什么危险且难发现**：`cargo fmt` 的 `newline_style=Auto` 会**保留** CRLF，
所以 `fmt --check` 照样绿灯；三门禁对行尾完全不敏感。
审查者是靠**候选清单 sha256 逐文件比对 + 原始字节核对**才发现的
（先用 hash oracle 证明文本差异恰为横幅，再由「多出的字节数恰等于行数」定位到 CRLF）。

**修复**：改用二进制写入（`read_bytes().replace(b"\r\n", b"\n")` + `write_bytes`），
避免再经文本模式。修后 `src-tauri/src/` 与 `src/` 全树 CRLF 计数为 **0**。
三门禁复跑：fmt exit 0、clippy exit 0、`cargo test` 120 passed / 0 failed / 23 ignored。

**教训**：
1. Windows 上写文件**必须显式指定行尾**（`newline="\n"` 或用二进制模式），
   否则 Python 的文本模式会静默改变行尾——这类差异三门禁**全部不敏感**。
2. 「我改的是注释」不等于「我只改了注释」：**整体重写文件**会顺带改掉行尾、
   编码、末尾换行等元信息。审查者正是按「原始字节」而非「文本内容」判定的。
3. 这也解释了为什么 TASK-044 的同类脚本没出问题——那批文件随后被 `cargo fmt`
   重排过，顺手归一成了 LF；本次这 4 个文件 fmt 认为无需改动，于是 CRLF 留了下来。

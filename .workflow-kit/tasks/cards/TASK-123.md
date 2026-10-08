<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-123 · 保位窗口扩展——深页阅读的后台刷新不再丢锚（审计 P2-5）

**状态**：done

**目标**：修复第三方审计 P2-5（tmp/audit-20261007/REVIEW.md 第 5 节，探针 P7 场景）：①刷新固定 offset=0/limit=500——加载 1000 篇读到第 750 篇后后台刷新，列表缩回前 500，anchorRestoreIndex 返回 null（非文章删除，是刷新策略主动丢阅读窗口）；②组合问题：缓存返回先定位、导航仍无条件后台重拉第一页，返回锚在第二页时重拉再次删除锚（只订阅 switchRestoreNonce 的 effect 不能在重拉后解决）；③锚只存 id 不存卡片内像素偏移；250ms 节流无尾沿补记。修法要求（在 TASK-111/115 机制上扩展，不推翻）：①**刷新围绕当前已加载窗口**：reloadFromBackend 的 keepReadingPosition 路径（内容刷新三入口）改为「保留窗口刷新」——携带当前窗口边界（cursor.loaded 或 keyset 锚），刷新拉取覆盖 [首屏+已加载页]（分页语义下按已加载页数拉取），或更优：窗口内文章按 keyset 重取全窗口；窗口重建后锚 id 必在（除非真被删除）；②**缓存返回+重拉的组合**：切换返回的恢复 effect 与导航重拉的协调——重拉若导致锚丢失，恢复 effect 需在重拉落地后再次尝试（或重拉本身走保位窗口路径使锚不丢）；③锚存档增加 intra-item 像素偏移（卡片内 scrollOffset，恢复时 scrollToIndex+像素偏移）；250ms 节流加尾沿补记（停滚后补记一次最终位置）；④缓存截断记录窗口边界（审计：不能只保留 loadedCount 假定已移除条目可达——TASK-111 的 budget 截断若发生，恢复后续拉衔接已有 cursor；核实并锁定）。⑤回归 t123-* ≥5 条（探针 P7 场景转真实行为回归）：深页（1000+）读到 750 后台刷新保位；切换返回+重拉组合锚不丢；intra-item 偏移恢复；节流尾沿；缓存截断窗口边界。既有断言零弱化（t111-*/t115-* 语义保持）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 深页（窗口跨页）读到 750 篇后后台刷新：阅读位置保持（t123-*，探针 P7 本体转回归）
- ② 切换返回+导航重拉组合：锚不因重拉丢失
- ③ intra-item 像素偏移恢复；节流尾沿补记
- ④ 缓存截断窗口边界处理正确（恢复后续拉衔接不跳行）
- ⑤ 门禁全绿：frontend（t123-* 新增）、lint/build/cargo_fmt 不回退；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@TASK-122 后（91059a3）：frontend 723/723、CI 全绿。本卡扩展保位窗口，属行为增强。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：t123-* 回归（深页刷新保位/组合锚/intra-item/尾沿/截断边界）；审计 P2-5 修复需操作序列场景锁定（DEC-gate-adjust ①）；验证：frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言（t111-*/t115-*/t122-* 语义保持）；不回退证据；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T11:44:00.209832Z
- 原截止时间：2026-10-07T15:44:00.209832Z
- 当前截止时间：2026-10-07T15:44:00.209832Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 145 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T14:34:01.467646Z：阻塞已处置（review_failure）：R0 审查 FAIL（1 低危）：reloadFromBackend 的 fetchWindowRows 续页失败被外层静默 catch 丢弃（重开 TASK-067 N10 已关闭的『后台刷新失败不可见』缺陷类，与 reloadFilteredEntries 不对称）。核心窗口机制经审查确认正确（终止性/锚必在/代际协同/游标一致全过，4 处变异独立复现）。R1：续页失败同样 toast+rethrow+1 条失败可见性回归。；下一步：begin 重新实现
- 2026-10-07T14:34:50.602515Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T14:40:17.155642Z：编码结果已记录，差异范围已核对：src/store/slices/bootstrap.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T14:40:51.382605Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T14:50:54.104050Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-10-07T23:03:44.787420Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T00:23:01.210469Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T00:36:54.218658Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-123.json)

- [RUN-a298604dda814413bcde36d132ef7506](../runs/RUN-a298604dda814413bcde36d132ef7506.json)
- [RUN-31017f403d864ab4bba2c718f971c7d0](../runs/RUN-31017f403d864ab4bba2c718f971c7d0.json)
- [RUN-6eca158554f546138839d2a966489f50](../runs/RUN-6eca158554f546138839d2a966489f50.json)
- [RUN-af185e424597457fa6aee58a8f4fea75](../runs/RUN-af185e424597457fa6aee58a8f4fea75.json)
- [RUN-68179ae51e59418c8a0881c88e4fa5e1](../runs/RUN-68179ae51e59418c8a0881c88e4fa5e1.json)
- [RUN-c82f1e9b7c634b51a1acc63e070e7a3f](../runs/RUN-c82f1e9b7c634b51a1acc63e070e7a3f.json)
- [RUN-a3ffb99e3ce54971b39c7729e60f5ccb](../runs/RUN-a3ffb99e3ce54971b39c7729e60f5ccb.json)
- [RUN-b82fc36ee851489cbc294de6eae944f3](../runs/RUN-b82fc36ee851489cbc294de6eae944f3.json)
- [RUN-bf718e8858cd47d5ae9fffb91f792498](../runs/RUN-bf718e8858cd47d5ae9fffb91f792498.json)
- [RUN-435771d3935a4dc3839bc8d6949baca3](../runs/RUN-435771d3935a4dc3839bc8d6949baca3.json)
- [RUN-36b10dd1e41a47ba9ac397d0fcce946d](../runs/RUN-36b10dd1e41a47ba9ac397d0fcce946d.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。

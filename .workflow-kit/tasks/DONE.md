<!-- project-workflow: generated view; edit task JSON instead -->
# DONE

| Task | Title | Status |
| --- | --- | --- |
| [TASK-029](items/TASK-029.json) | 全局排查空壳功能与隐藏 Bug，产出可确认清单（REQ-007） | done |
| [TASK-030](items/TASK-030.json) | 修复社交布局正文无限加载（REQ-001） | done |
| [TASK-031](items/TASK-031.json) | 定位双向同步缺口：订阅与文章状态回传（REQ-002/003） | done |
| [TASK-032](items/TASK-032.json) | 同步状态链路修复：对账防误判（C-1）+ 离线变更一律入队（A-5） | done |
| [TASK-033](items/TASK-033.json) | 生产错误回退 mock 修复（P0-2）+ 两处 [object Object] 错误文案（P1-10/P1-11） | done |
| [TASK-034](items/TASK-034.json) | 社交/通知卡片翻译按钮接线（P1-7） | done |
| [TASK-035](items/TASK-035.json) | 删除订阅接线：远端退订 + 删除墓碑防复活（A-1） | done |
| [TASK-036](items/TASK-036.json) | 订阅改名/移动目录接线：edit_subscription 推送远端（A-2） | done |
| [TASK-037](items/TASK-037.json) | 同步接线收尾：push 挂分类（A-3）+ 分类改名/删除防复活（A-4） | done |
| [TASK-038](items/TASK-038.json) | 同步队列卫生：老化清理（A-8）+ 吞错日志（C-2） | done |
| [TASK-039](items/TASK-039.json) | REQ-004 播客页 toast 位置 + REQ-008 设置页控件一致性 | done |
| [TASK-040](items/TASK-040.json) | 前端缺陷批一：按 id 摘要态（F4）+ 搜索打开标读（F7）+ 全部已读视图口径（F8）+ 搜索竞态（F20） | done |
| [TASK-041](items/TASK-041.json) | 前端体验收尾：文案统一精简（REQ-006）+ 控件一致性（REQ-008） | done |
| [TASK-042](items/TASK-042.json) | 交互动画打磨（REQ-005）：补全过渡与 prefers-reduced-motion | done |
| [TASK-043](items/TASK-043.json) | TASK-041 遗留收尾：焦点可达性 + 措辞口径 + 按钮收敛（REQ-006/008） | done |
| [TASK-044](items/TASK-044.json) | 后端单体拆分一：commands.rs 按领域拆分为子模块（可维护性） | done |
| [TASK-045](items/TASK-045.json) | 后端单体拆分二：sync.rs 按领域拆分为子模块（可维护性） | done |
| [TASK-046](items/TASK-046.json) | 流程工具修复：prepare 读取 scope.allowed_paths 并对静默回退报错（消除二次命中的死锁缺口） | cancelled |
| [TASK-047](items/TASK-047.json) | 可访问性修复：关闭态浮层不可聚焦（inert），消除 Tab 进入不可见控件 | done |
| [TASK-048](items/TASK-048.json) | 前端拆分前置：补 store.ts 行为测试（为拆分建立回归网） | done |
| [TASK-049](items/TASK-049.json) | 前端拆分一：store.ts 按领域拆为 Zustand slice（行为保持不变） | done |
| [TASK-050](items/TASK-050.json) | 前端拆分二：SettingsModal.tsx 按既有函数边界拆为设置页子模块（纯移动） | done |
| [TASK-051](items/TASK-051.json) | 修复全部已发现缺陷：D1–D5 + 低优先观察 + REQ-007 残余项（先盘点后逐项修） | done |
| [TASK-052](items/TASK-052.json) | P1-14 口径半边：分页带上当前范围筛选（per-scope 游标重设计） | done |
| [TASK-053](items/TASK-053.json) | P1-5 离线期间的「全部已读」不入队：改为无论是否已配置都入队待推 | done |
| [TASK-054](items/TASK-054.json) | 恢复被失效 #[ignore] 理由掩盖的 14 个测试（先补网） | done |
| [TASK-055](items/TASK-055.json) | 修 subscriptions.rs 墓碑误清除：已删订阅被 pull 复活（P1） | done |

verified 表示验证/审查完成、等待所属交付验收；不等于已经合并。

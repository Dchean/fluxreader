# 拍板清单（产品死规矩 + 架构边界）

每条 2~4 行大白话；关联源码路径可直接跳。源码顶部反向注释**本次未改业务代码，后续实际修改对应文件时再顺手补入路标**。

## A. 产品 / 界面红线（拍过板，不许被半成品代码反向抹掉）

1. **对比度不能降。** 普通文字 ≥4.5:1，图标/大字 ≥3:1；深色 accent 兼作前景与白字按钮底时单值无法双向达标，只按图形口径验前景，**不得降阈值关问题**。→ `src/styles/tokens.css`
2. **列表宽度硬上限 420px。** 三栏是 `260px / clamp(280px, min(--list-width,37vw), 420px) / 1fr`；放到 560 会把 980 视口阅读列压到 160。校验区间与 UI 夹取不一致是已登记行为项，未拍板前不擅自改。→ `src/styles/base.css`、`src/store/settingsValidation.ts`
3. **J/K 五布局统一"选中驱动焦点"。** 卡片加 `aria-current` + `.active-selected`；卡被筛掉时只在焦点落 BODY 时迁到同下标，不抢侧栏/筛选焦点。→ `src/components/Timeline.tsx`
4. **Space 让路优先级：原生控件 > 播放器 > 卡片。** 让路白名单刻意不含 `[role=button]`。→ `src/components/shortcutYield.ts`
5. **返回位置按「条目 ID + filterKey + 卡内偏移」记账。** 属性刷新 / 切回缓存 / 关阅读器是三条独立 nonce，不能合并成一条。→ `src/lib/readingPosition.ts`
6. **文案先答"现在能做什么"。** 技术边界折叠；墓碑/label/no-op 改写成后果句；成功态交给可见界面，低价值 toast 清零。→ `src/lib/copyPolicy.ts`
7. **带动作的 toast 必须同登记错误中心。** 上限 20 条、24h 惰性 TTL、不写盘；重试打的是失败时的目标，不是"当前对象"。→ `src/components/ErrorCenter.tsx`
8. **播放器 `applyMediaAction` 是目标态语义**，只有 toggle 才切换；社交折叠固定外高、画廊按列取可见窗口。→ `src/components/PlayerBar.tsx`、`src/components/GalleryLayout.ts`

## B. 数据安全与边界红线

1. **可渲染 HTML 只有 `sanitize()` 一个口径**，写入口与读边界共用；只做元素级替换与属性赋值，绝不把属性串 format 成标签。→ `src-tauri/src/sanitize.rs`
2. **DPAPI 加解密失败一律 Err，不写明文；读取错误 ≠ 未配置。** 账号保存/断开的 purge + 配置同一短事务。→ `src-tauri/src/credentials.rs`
3. **快照 `VACUUM INTO` → `.partial` → quick_check → 原子改名**，保留 10 份；不做应用内覆盖式恢复。→ `src-tauri/src/db/backup.rs`
4. **便携 v4：预览验全容器摘要，恢复单事务替换**并重生成 actor/主键高水位、清凭据与运行队列。→ `src-tauri/src/local_sync/backup.rs`
5. **分类墓碑按 `account_key` 写入/回收**，空 key 全局生效；读取失败就失败关闭、跳过导入。→ `src-tauri/src/db/folders.rs`
6. **外链只经 `openExternalWithFeedback`，仅 HTTP/HTTPS**；主页面只放行应用 origin。→ `src/lib/external.ts`、`src-tauri/src/navigation.rs`
7. **预算不放大：** 图片抓取/IPC/在途预留各 25 MiB、抓取路径 16 MiB；WebDAV XML 与批 256 KiB、单次 get ≤4 MiB；本地正文 32 MiB。→ `docs/contracts.md` 的预算行
8. **AI 只用 OpenAI 兼容协议。** Base URL / Key / 模型由用户填；**不预填默认模型、不探测端点**，模型为空则零请求。→ `src-tauri/src/ai.rs`

## C. 架构契约单点（改一处必须同步的面）

1. **阅读中成员与顺序不变。** `ReadingSession` 捕获一次查询的完整有序 ID，后台同步不插入新成员、不重排。→ `src/store/readingSession.ts`
2. **乐观写入按条目 × 字段版本**（`${id}|${field}`）；回滚要凭据匹配且不推进版本。→ `src/store/internals.ts`
3. **同步冲突方向单点定义**，并守住"失败不是空集合"。→ `src-tauri/src/sync/conflict_policy.rs`
4. **订阅写回 outbox**：合并同账号同 URL、未发送链保留链起点、账号屏障、`local_only` 终态。→ `src-tauri/src/db/outbox.rs`
5. **AI 三层策略唯一 resolver `selectFeedConfig`**（布局/组/源）。→ `src/store/aiPolicy.ts`
6. **写事务唯一入口 `db::write_transaction`（IMMEDIATE）+ `db/writer.rs` FIFO**，锁序固定 业务锁 → writer。→ `src-tauri/src/db/writer.rs`
7. **同步三通道 Content → State → Push**，每类一个运行操作；会话缓存单槽 `account_key + generation`。→ `src-tauri/src/sync/runtime.rs`
8. **全刷 single-flight + 90s footprint + `fetch_generation` CAS**，抓取名与自定义名分域。→ `src-tauri/src/scheduler.rs`
9. **正文/AI 实体缓存 = 内容身份 + 2000 项 LRU + contentRevision 戳。** → `src/store/bodyCache.ts`

## D. 仍有效的流程铁律

- 证据五要素：基线 SHA、命令、输入、结果、性质（运行 / 静态 / 未验证）。缺一只是线索。
- 独立审计者只读、只交报告；登记由实现者或其指定登记者做。
- 同类方法连败两次，带证据换方法，不反复试错。
- **不得为变绿删测试、加 `#[ignore]` 或放宽断言。**
- 发布说明只取 tag 附注，且只接受 tag ref；不改历史 tag 附注。
- 文档不写行号（用"文件 + 符号名"或 `<a id>` 锚）。

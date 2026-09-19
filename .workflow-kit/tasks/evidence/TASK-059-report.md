# TASK-059 实施报告：Endpoint 自动适配（只填域名即可连接）

- 运行：RUN-61037fec83504a9c87b2da94c03bc088
- 任务：TASK-059（批次 BATCH-6108ba756ba5490f924879b7f22b2f18）
- 授权：DEC-endpoint-autodetect-20260918（自动适配）、DEC-fever-version-tolerant-20260918（Fever 版本放宽）
- 接手说明：本任务前一位执行者**编辑到一半停止**（`tests/mock_greader.rs` 留下重复片段、
  无法编译，后端探测已写但解析结果未被采用、缓存与前端文案均未做）。本报告覆盖**完整交付**，
  接手时的状态与后续补充写在 §2。
- 日期：2026-09-19（本地）

---

## 1. 需求与根因

需求（owner 明确纠正）：**不管后端是 FreshRSS 还是 Miniflux，用户只需填域名**，
不需要知道 `/api/greader.php` 这类 API 后缀。

此前 TASK-057 交付的是「教用户填完整路径」——那是**方向反了**：它要求用户去适配
应用内部对路径形态的假设。根因是两处**路径形态被写死**：

| 协议 | 写死的行为 | 真实布局 |
| --- | --- | --- |
| GReader | `{用户输入}/accounts/ClientLogin` | Miniflux 在根；FreshRSS 在 `{域名}/api/greader.php` 之下 |
| Fever | `{base}/fever/?api`（`fever.rs:126`） | Miniflux 是 `/fever/`；FreshRSS 是 `/api/fever.php` |

**Fever + FreshRSS 此前根本连不上**（实测 `{域名}/fever/?api → 404`）。

实证依据（2026-09-18，demo.freshrss.org）：

```text
POST {域名}/accounts/ClientLogin                → 404   路径不存在
POST {域名}/api/greader.php/accounts/ClientLogin → 401   端点存在，凭据被拒
GET  {域名}/api/fever.php?api                    → 200   {"api_version":4,"auth":0}
GET  {域名}/fever/?api                           → 404
```

即 **404 与 401/403/400 可可靠区分「路径不存在」与「路径正确但凭据错」**——
这是自动探测能成立、且**不会把凭据错报成地址错**的判据。

**为什么既有测试全漏掉**：所有 Rust e2e 与前端回归都用「已知正确的 endpoint」
（mock 的 `server.url()` 直接就是 API 根），**「用户填的是纯域名」这一输入形态从未被测过**。
本任务必须补上该形态的测试。

## 2. 接手时的状态（如实记录）

前一位执行者留下的改动：新增 `endpoint_resolve.rs`（候选表 + `path_exists`）、
`greader.rs` 的 `login` 探测循环、`fever.rs` 的 `call_probe`/版本放宽、mock 的布局模拟。
**这些是有价值的**，但存在以下**未完成/缺陷**，本轮全部处理：

| # | 问题 | 性质 | 处置 |
| --- | --- | --- | --- |
| 1 | `tests/mock_greader.rs:325` 保留重复片段（`}d_prefix(&srv, path) {`） | **无法编译** | 删除重复片段 |
| 2 | Fever 探测成功后**丢掉解析结果**（`verify()` 只 `map(|_| ())`），同步侧仍拿纯域名拼 `/fever/?api` ⇒ FreshRSS 依旧 404 | **逻辑缺陷（本任务核心目标未达成）** | 新增 `resolve()` 返回「base 已确定为 API 根」的客户端，并加测试锁定「解析结果被真正使用」 |
| 3 | 解析结果**未缓存**：`build_client` 在每轮 feeds/states/调度同步都会重复探测 | 契约硬性要求未做 | 新增缓存（落库 + 按输入自动失效） |
| 4 | mock 的前缀过滤对 Miniflux 形态（`prefix=""`）判定过宽（任何路径都放行），探测测试会失去意义 | 测试保真度缺陷 | 改为「恰好等于 前缀+端点」 |
| 5 | 前端文案**完全未改**（仍在教用户填 `/api/greader.php`） | 契约要求未做 | 已更正（§4） |
| 6 | 无任何实机（UI）证据 | 契约要求未做 | 已补（§6） |
| 7 | 新测试缺失、门禁未跑 | 契约要求未做 | 已补（§5） |

## 3. 后端改动

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/endpoint_resolve.rs` | **新增（197 行）**：GReader/Fever 候选表（固定且有限）、`path_exists`（**只有 404 = 路径不存在**）、解析缓存 `cached_base` / `remember_base`；模块文档写明边界与实测依据 |
| `src-tauri/src/greader.rs` | `login` 改为遍历候选；抽出 `login_at`（`Ok(None)` = 404 该路径无端点，`Err` = 路径对但请求被拒）；新增 `login_resolved`（用已解析地址直接登录，**不再探测**）、`resolved_base()` |
| `src-tauri/src/fever.rs` | 新增 `resolve()`（探测**并采用**解析结果）、`at_resolved()`、`resolved_base()`；`api_entry()` 按形态拼入口（`.php` → `?api`，否则 `/fever/?api`）；版本校验 `!= 3` → `< 3`（**`auth` 校验不放松**） |
| `src-tauri/src/sync/credentials.rs` | `build_client` 改为「命中缓存 ⇒ 候选收敛为唯一地址，这一次请求只做认证；未命中 ⇒ 客户端自己探测，成功即已认证」；探测成功才落库 |
| `src-tauri/src/sync/phases.rs` | `test_connection` 两个协议都先 `resolve()` 再用解析出的地址拉订阅；返回值增加「解析出的 API 根」 |
| `src-tauri/src/commands/sync.rs` | `sync_save` 把设置页刚验证出的解析结果写入缓存（后续同步不必重探）；`greader_endpoint` 存的**仍是用户原始输入** |
| `src-tauri/src/lib.rs` | 注册 `endpoint_resolve` 模块 |

**关键设计（与契约逐条对应）**

- **有界且有序**：候选表是常量、最多 2 个；顺序**先试用户原样输入**——
  已填完整路径的老用法**首个候选即命中**，行为与耗时不变；
- **凭据错立即停止**：任一候选返回非 404 即判定「路径已找对」，立刻按真实状态码报错，
  **不再试其余候选**（否则密码填错会被说成「找不到 API」，比改动前更糟）；
- **缓存按输入自动失效**：缓存项同时存 `protocol` 与**用户原始输入**，命中条件是
  「协议 + 输入都一致」，故改地址/换协议自动失效，不会出现「改了地址却仍按旧地址同步」；
- **回显原样**：`greader_endpoint` 始终存用户输入；解析结果另存 `endpoint_resolved`，
  不回显到界面。

## 4. 前端文案更正

`src/components/settings/endpointHint.ts`：

| | 修前（TASK-057） | 修后 |
| --- | --- | --- |
| desc | `Miniflux 填站点根（如 https://reader.example.com）；FreshRSS 填 API 路径（如 https://demo.freshrss.org/api/greader.php）` | `只填域名即可：FreshRSS / Miniflux 自动适配` |
| placeholder | `如 https://主机/api/greader.php` | `如 https://demo.freshrss.org` |
| 404 提示 | `…请确认 Endpoint 是否需指向 API 路径。Miniflux 填站点根，FreshRSS 需填 https://主机/api/greader.php` | `…已自动尝试该域名下的常见 API 路径：请确认域名是否正确，以及后端是否已启用 Google Reader / Fever API` |

**顺带修掉一个既有缺陷**：在运行中的应用里量到该说明区
（`.setting-card-text p`）**宽 234px、`-webkit-line-clamp: 2`**，即**只有两行**。
实测旧文案需要 **4 行**（`scrollHeight 69px` vs `clientHeight 35px`），
**一直在被截断**——用户根本读不到「FreshRSS 填 API 路径」这后半句。
新文案实测 **单行**（`17px == 17px`），完整可读。

TASK-057 遗留的前端断言**已相应适配而非删除**（`(e1)`–`(e5)` 全部重写为「只填域名」方向，
并保留其防误伤边界：401/403 一律原样透出）。

## 5. 测试与门禁

### 5.1 四门禁（实测）

| 门禁 | 命令 | 基线 | 实测（审查修复前） | 实测（审查修复后，终版） |
| --- | --- | --- | --- | --- |
| cargo | `cargo test`（src-tauri） | 141 passed / 0 failed / 9 ignored | 160 / 0 / 9 | **161 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors | 0 / 0 | **0 warnings / 0 errors** |
| build | `npm run build` | exit 0 | exit 0 | **exit 0** |
| frontend | `npm run test:frontend` | 280/280 | 282/282 | **283/283** |

cargo 的 +20 = `endpoint_autodetect_e2e` 14 条 + `endpoint_resolve` 单测 6 条；
frontend 的 +3 = Endpoint 文案组重写后的净增（既有 26 条原样保留）。
ignored 始终为 9，未增加。

### 5.2 新增 Rust 测试（`src-tauri/tests/endpoint_autodetect_e2e.rs`，14 条）

| 测试 | 覆盖的契约点 |
| --- | --- |
| `bare_domain_resolves_freshrss_layout` | 纯域名 + FreshRSS（GReader）⇒ 成功，且解析到 `/api/greader.php` |
| `bare_domain_resolves_miniflux_layout` | 纯域名 + Miniflux ⇒ 首个候选命中、base 保持站点根 |
| `full_path_still_works_first_try` | 旧用法一次成功（向后兼容） |
| `missing_path_vs_bad_credentials_are_distinguished` | **核心边界**：路径全错 ⇒ 「找不到 API」；凭据错 ⇒ **不得**报成「找不到 API」 |
| `probing_is_bounded_and_full_path_does_not_extra_probe` | 探测有界（纯域名恰好 2 次）；完整路径恰好 1 次（无多余探测） |
| `fever_bare_domain_resolves_freshrss_endpoint` | Fever 纯域名 + FreshRSS 形态 ⇒ 成功 |
| `fever_resolved_client_actually_uses_resolved_endpoint` | **解析结果被真正采用**：解析后的客户端拉数据时，所有请求都打在解析出的端点上 |
| `fever_write_path_uses_resolved_endpoint_too` | **写路径同样走解析出的端点**（独立审查 F1 的回归网）：标记已读/收藏都必须打在 `/api/fever.php`，且不得出现两种形态的叠加拼接 |
| `fever_accepts_api_version_4` / `fever_rejects_api_version_below_3` | 版本放宽到 `>= 3`，但低于 3 仍拒绝 |
| `fever_still_rejects_bad_auth` | **`auth` 校验未放松** |
| `cache_hits_on_same_input_and_invalidates_on_change` | 缓存命中；**改地址/换协议即失效** |
| `second_sync_reuses_cached_endpoint_without_probing` | **缓存生效实证**：首轮探测 2 次并落库；次轮**恰好 1 次 ClientLogin、0 次探测** |
| `candidates_are_bounded_and_shaped_correctly` | 候选表有界、形态正确；只有 404 代表路径缺失 |

mock（`tests/mock_greader.rs`）新增能力：可配置 GReader 前缀（`""`/`/api/greader.php`）
与 Fever 端点（`/fever/`/`/api/fever.php`）、故障注入（ClientLogin 401、`auth=0`、
`api_version`），以及**请求记录**——`probing_is_bounded_*` 与
`second_sync_reuses_cached_*` 需要「试了几次」的直接证据，只看状态码区分不出来。

### 5.3 前端断言（修前失败 / 修后通过）

**修前失败证据**（先把新断言写进套件、文案尚未改时运行）：

```text
=== 前端逻辑回归合计 274/281 通过 ===
新增失败项: (e1) Endpoint 说明要求「只填域名」并说明应用自动适配;
            (e1) Endpoint 说明不得再教用户填 /api/greader.php 后缀（那是被纠正掉的方向）;
            (e1) placeholder 给出纯域名示例（不含 /api 后缀）;
            (e1) placeholder 不得用实测超宽（219.46px > 219px）的完整路径写法;
            (e3) 404 提示不再要求用户填 API 后缀（自动适配后该说法会反向误导）;
            (e3) 404 提示仍可操作：说明已自动尝试并指向域名/后端核对;
            (e3) 405/410 同属「路径不存在」，同样附加该指引
```

**修后通过**：`=== 前端逻辑回归合计 282/282 通过 ===`
（既有 26 条未改一行；新增 store 行为断言 256/256）。

新增 `(e1)` 里还有一条**长度预算断言**，用实测字体度量（CJK 11.5px / ASCII 5.9px）
限制 desc 的估算宽度，防止再写出会被 `line-clamp` 截断的说明。

### 5.4 端到端与实机

- **Rust e2e**：见 5.2；
- **实机（真实运行的应用 + 本地 FreshRSS 形态服务端）**：见
  [TASK-059-interaction-report.md](TASK-059-interaction-report.md)，
  7 个场景全通过，含「只填域名连通」「旧路径兼容」「凭据错如实报 401」
  「Fever 纯域名连通」与双主题截图。

## 6. 边界遵守情况

| 边界 | 状态 |
| --- | --- |
| 不引入新依赖；`Cargo.toml`/`Cargo.lock`/`package.json` 零改动 | **遵守**（`git diff --stat` 未见这三个文件） |
| 不改认证流程与 ClientLogin 双格式解析 | **遵守**（`login_at` 内仍是原 JSON→文本回退逻辑） |
| 不放松 Fever `auth` 校验 | **遵守**（有专门测试 `fever_still_rejects_bad_auth`） |
| 不改同步协议语义（推送顺序/对账口径/入队条件） | **遵守**（未触碰 `push.rs`/`entries.rs`/`subscriptions.rs` 的逻辑） |
| 不改 TASK-058 的失败可见性实现 | **遵守**（`syncErrors.ts` 与两处调用点未动；其断言全部仍通过） |
| 不改工作流脚本 | **遵守**（未动 `.workflow-kit/scripts/**`、`binding.json`） |
| 文本文件 LF 行尾 | **遵守**（新增文件按 LF 写入） |
| **用户真实数据库只读 / 改后逐字节还原** | **一度违反，已还原并加装防护**——经过与证据见 [TASK-059-incident-real-db-write.md](TASK-059-incident-real-db-write.md) |

## 7. 如实记录的问题（含我自己的失误）

1. **首轮深色截图实为浅色**：文件名写 dark、实测渲染为 light。
   修正为「截图前硬校验 `data-theme` 与背景亮度，不符即中止且不落盘」。
2. **Endpoint 说明被 `line-clamp` 截断**（且旧文案一直在被截断）：见 §4，已修正并加断言。
3. **误写用户真实库**：`APPDATA=...` 对 Tauri 无效（`app_data_dir` 走 Win32 已知文件夹 API），
   我的「隔离运行」实际写了真实库；`sync_save` 的换账号分支还触发了 `purge_remote_data`。
   已按 WAL 帧重放**逐字节还原**（哈希一致）并加装强制备份/还原校验。
   完整经过见事故记录。
4. **驱动自身的三个坑**（均已修，写进注释避免重犯）：
   - 按卡片文案找「密码」输入框，会命中**用户名**卡片（其说明含「账号密码」）；
     改为按字段身份定位；
   - 「保存并同步」成功后应用会清空密码框，后续步骤须重填，否则得到的是应用自己的
     「请填写…」；驱动改为「出现非预期 toast 立即报错」，而不是傻等超时；
   - 切到「外观」再切回「同步」会**重新挂载** SyncTab、表单 state 被重置，同理须重填。

## 8. 未覆盖 / 限制

- **Miniflux 形态未做实机截图**：同一 mock 端口无法同时呈现两种布局；
  实机验证的是 FreshRSS 形态（用户实测出问题的那一种），Miniflux 形态由 Rust e2e 覆盖。
- **Fever 的 `auth=0` 拒绝路径未走实机**：由 Rust e2e 覆盖。
- **探测缓存未使用系统的设置同步机制**：缓存键 `endpoint_resolved` **不会**随配置同步走——
  `config_sync.rs` 是白名单同步（仅 `sync_protocol`/`greader_endpoint`/`greader_username` +
  `app_settings`），`endpoint_resolved` 不在其中。因此换设备后首次同步会重新探测一次
  （属可接受行为：探测本身是有界的），但**不应**把它当作「已跨设备同步」。
  （初版报告此处写成「会随既有设置同步走」，与 `config_sync.rs` 不符，已订正。）
- 本轮实机验证的后端是**本地 mock**（`tools/mock_greader_ui_server.py`），
  未再次对 demo.freshrss.org 发起真实网络请求（任务范围 `network: none`）。

## 9. 独立审查（第 1 轮 FAIL → 修复 → 第 2 轮 PASS）

第 1 轮**独立审查判定 FAIL**，指出 6 项问题。**其中第 1 项是真实的用户可见缺陷，我的测试网漏掉了**。
修复后第 2 轮**独立审查判定 PASS**（findings 为空，五个核对域全 PASS，performance 为
NOT_APPLICABLE 并给出理由）。两轮结论分别保存在
`TASK-059-review-r1-independent-FAIL.json` 与 `TASK-059-review-r2-independent-PASS.json`。

**两轮均来自没有参与实现的新上下文**（审查者独立复算了全部候选文件哈希、
自行重跑四门禁，并按任务卡要求**未启动应用、未运行会写真实库的 e2e 驱动**）。

### 必须修（已修，第 2 轮确认）

**F1 · HIGH/MED — `fever.rs` 的 `mark_items` 绕过 `api_entry()`**：
它仍写死 `format!("{}/fever/?api&mark=item…", self.base)`。FreshRSS 形态下解析出的
base 是 `…/api/fever.php`，于是拼成 **`…/api/fever.php/fever/?api` → 404**。
该路径由 `push.rs` 的已读/收藏推送调用，后果是：
**「Fever + FreshRSS」能拉、能看，但状态推不上去**，`SyncReport.errors` 会一直累积
「Fever mark … → 404」——正是本任务（首次让该组合可用）新引入的组合上的缺陷。

- **修法**：`mark_items` 改用 `self.api_entry()`；
- **测试**：新增 `fever_write_path_uses_resolved_endpoint_too`，
  断言写路径请求都打在解析出的端点上、且**不出现** `/api/fever.php/fever/` 这种叠加；
- **修前失败的证据（实测）**：把 `mark_items` 改回写死版本后运行该测试：

```text
test fever_write_path_uses_resolved_endpoint_too ... FAILED
panicked at endpoint_autodetect_e2e.rs:249:
标记已读应成功: AppError { code: "network", message: "Fever mark read 1001 → 404 Not Found" }
```

**F2 · MED/LOW — mock 的 Fever 前缀判定用 `starts_with`，抓不到 F1**：
`…/api/fever.php/fever/?api` 会被误判为合法（因为以 `/api/fever.php` 打头），
所以 mock 返回 200 信封、F1 在测试里静默通过。已改为**恰好相等**（与 GReader 分支一致）。

**F6 · LOW — 新错误消息不含状态码，前端指引分支永不触发**：
后端「找不到 API」的消息是「在该地址下找不到 GReader API（已尝试：…）」，
不含 404/405/410 字样，因此 `endpointHint` 的「路径类失败」分支**对其主场景失效**，
用户看到的是一条没有指引的裸错误。已在消息中补上 `HTTP 404`，并新增前端断言
覆盖该**真实消息形状**（不只是合成串 `'ClientLogin → 404'`）。

### 其余三项与报告/证据有关，已订正

- **F3 · LOW — 过期缓存不自愈**：命中缓存时不再探测，若缓存的地址已失效，
  会一直 `notConnected` 直到用户重存配置。**本轮不改**——契约只要求「用户改地址后缓存须失效」
  （已做到并有测试），而加自愈会引入新的失败模式：探测可能把**真实的凭据问题**
  掩盖成「地址换了」。第 2 轮审查同意该判断（明确表示不因此阻断），本项已记为已知限制。
- **F4 · LOW — 报告措辞与 `config_sync.rs` 不符**：见 §8 已订正（初版称缓存「会随设置同步走」）。
- **F5 · LOW — UI 证据未放在 run 级目录**：截图直接放在 `tasks/evidence/` 下，
  而审查包声明的 `ui_evidence_directory` 是 `TASK-059-ui/<verification_run>/`。
  证据文件本身真实有效（两轮审查均核对过主题、文案与 toast 一致性），
  此处仅路径约定未对齐；第 2 轮审查判定为「可发现性/约定差异，不是证据真实性问题」，不阻断。

### 第 2 轮审查独立复算并确认的事实

- 全部 **144/144** 候选文件哈希与审查包一致，0 处不符；候选清单自身的 `digest`
  与 RUN 记录里的 `candidate_digest` 均为 `e57efa82…`；改动文件仍是任务卡里的 19 个（无越界）；
- 自行重跑：`cargo test` **161 / 0 / 9**、`npm run test:frontend` **283/283**、
  lint 0/0、build exit 0——与作者日志逐项吻合；
- 核对 F1 的修法是否彻底：`fever.rs` 中 `self.base` 仅出现在 `api_entry()` 自身、
  `resolve()` 的候选来源与 `resolved_base()`，**再无任何站点绕过** `api_entry()`；
- 判定新增的 `fever_write_path_uses_resolved_endpoint_too` 是**有意义的守卫而非同义反复**
  （它先真实 `resolve()`，再断言请求日志只打 `/api/fever.php` 且不出现形态叠加），
  并指出其效力依赖 F2 的 mock 收紧——两者构成一个整体守卫；
- 真实库只读核对：`419b118e…`、`-wal`/`-shm` 不存在，与事故报告的还原声明一致；
- 3 张截图主题与文件名相符、desc 未截断、toast 与 `TASK-059-ui-e2e-result.json` 逐字一致。
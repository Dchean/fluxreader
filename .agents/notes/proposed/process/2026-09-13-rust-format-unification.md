# Agent Note: 以独立任务统一 Rust 格式

Status: proposed

## Problem

`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` 返回 1，涉及 **38 个文件**（`src-tauri/build.rs`、20 个 `src/` 模块、17 个 `tests/` 目标）。本管理 agent 接手后复核确认该结果与文件清单，仅 diff 分节计数由 532 变为 528（同一文件集合，分节合并方式所致，见 `docs/runs/BATCH-001-takeover.md` §2.2）。

这是当前唯一已知的**失败**检查。同时存在两个特殊之处：

1. 该检查**不在现有 CI 中**（`ci.yml` 只跑 clippy 与 test），因此它是一次"新增测量检查的失败"，而不是"CI 已经红了"。
2. 它覆盖 `src/**` 与 `tests/**`，即**整个 Rust 侧**。若与任何业务改动混在同一次变更里，审查者将无法从 diff 中区分"格式重排"与"逻辑改动"。

因此格式统一不能搭车任何功能任务，必须独立成任务、独立留痕、独立验证。

## Proposal

使用项目既有的 rustfmt（stable 工具链已安装 `rustfmt-x86_64-pc-windows-msvc`），对允许清单内的 38 个文件执行一次格式化：

```text
cargo fmt --manifest-path src-tauri/Cargo.toml --all
```

该命令作为**管理 agent 执行的机械操作** `ACTION-RUSTFMT-ALL` 预定义在任务 JSON 中，Claude 只请求该 ID，不生成任意命令。

约束：

- 只改变格式，不改变任何业务语义、测试预期、`#[ignore]` 标记、迁移 SQL 内容、`Cargo.toml`/`Cargo.lock`、配置或 CI。
- 38 个文件是**允许清单**，不是示例；任何清单外变化都视为越界。
- 格式化前后的完整 diff 必须保存，并按"是否只含空白/换行/流式重排"审查。

## Alternatives considered

### 不做：保留现状，不把 rustfmt 纳入门禁

最强理由是 rustfmt 检查**当前不在 CI 中**，失败不影响任何交付；而 `cargo fmt --all` 会产生一个跨 38 文件的巨大 diff，污染 git blame、让后续 review 变重，收益却只是"风格统一"。

不采用的理由：38 文件 / 500+ 处的漂移意味着此后再想统一，代价只会更大（每个新改动都增加漂移面）。且本批恰好存在一个天然的独立时机。**但该理由成立的前提是"独立成一次的纯格式变更"** —— 因此本提案明确反对把它并入任何业务任务。

### 在 CI 中新增 `cargo fmt --check`，与格式化同时进行

最强理由是一次做完"统一格式 + 防止再漂移"，避免刚格式化完又开始漂移。

不采用的理由：本任务范围明确不含 `.github/**`。且若同时新增门禁，一旦格式化的 38 文件中有任何一处是 rustfmt 对某个宏或属性列表的**语义敏感**重排（例如影响 `cfg` 属性顺序），门禁会立刻把问题放大成"CI 红"。应当先格式化并确认测试全绿，再由独立任务决定是否加门禁。

### 只格式化 `src/`，不动 `tests/`

最强理由是测试文件对格式最不敏感，且改动面减半。

不采用的理由：`cargo fmt --all` 是工具的单一边界；手工拆分会产生"部分格式化"状态，下一次 `--check` 仍然失败，等于制造了更混乱的中间态。

## Acceptance criteria

- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` 返回 **0**。
- 变更仅由 `ACTION-RUSTFMT-ALL` 产生，无允许清单外文件。
- 未改变任何行为、断言、`#[ignore]`、迁移 SQL、依赖或配置。
- `cargo clippy --locked --all-targets -- -D warnings` 通过。
- `cargo test --locked` 与指定的 6 个 target `--ignored` 套件通过（与 TASK-002 基线口径一致）。

## Risks

- **最大风险是掩盖语义变化。** rustfmt 在极少数情况下会重排 `#[cfg(...)]` 属性或宏 token 顺序。因此不能只看"测试通过"，必须审查 diff 是否只含格式化差异，并单独确认测试文件中的 `#[ignore]` 数量（忽略数从 23 变为其他值即为越界信号）。
- 该 diff 体量大，天然难以逐行人工审查。缓解方式是：只对 `git diff` 中非空白差异做抽样核对，并确认格式化后测试计数与基线一致。
- 格式化使此前所有绑定旧哈希的 Rust 证据失效；本批其他任务（TASK-004/005）不涉及 Rust，不受影响。

## Verification

管理 agent 独立执行：

```text
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --test account_lifecycle_e2e --test ai_e2e --test dual_client_e2e --test sync_content_e2e --test sync_e2e --test sync_phases_e2e -- --ignored
```

并以 `git status --porcelain` 与允许清单逐项比对。

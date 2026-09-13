# Agent Note: Rust 全仓格式统一与 fmt CI 门禁

Status: implemented

## Problem

TASK-002 基线测量时 `cargo fmt --check` 失败：38 个 Rust 文件共 532 处格式差异，且该检查不在 CI 中——格式漂移没有任何门禁拦截，后续改动会持续累积差异。

## Decision

- BATCH-001 的 TASK-006 以预定义机械操作 `ACTION-RUSTFMT-ALL`（`cargo fmt --manifest-path src-tauri/Cargo.toml --all`）统一全部 38 个文件；独立审查以确定性格式化复现（39/39 逐字节一致）确认无语义夹带（见该审查 R-05 更正）。
- BATCH-004 的 TASK-014 在 ci.yml 的 rust 作业中 clippy 之前新增 `cargo fmt --check` 步骤——格式漂移从此在 CI 被拦截。
- 后续 Rust 代码必须先过 rustfmt：BATCH-004 的 TASK-013 worker 产出曾因未格式化被该要求拦下，由管理 agent 机械格式化后通过。

## Consequences

- 收益：格式成为零争议事项——CI 强制、机械可修，代码审查不再讨论风格。
- 代价：任何未过 rustfmt 的 Rust 改动会直接 CI 失败（预期内的暴露；修复方式就是运行 `cargo fmt --all`，不接受放宽检查）。
- 边界：rustfmt 只保证格式；语义不变性依赖 TASK-006 确立的"确定性格式化复现"验证方法，不使用标识符多重集等弱证据。

## Alternatives considered

### 不做：格式靠代码审查自觉

最强理由是零 CI 成本、不挡任何 PR。

不采用的理由：TASK-002 基线已证明会漂移到 532 处差异；审查者不该花时间讨论风格。

### CI 中只警告不失败（不设 -D 类硬门禁）

最强理由是避免格式问题阻塞功能合并。

不采用的理由：软警告必然被忽略（6 条 lint 警告存活数月即为佐证）；fmt 是机械可修的，失败的成本是运行一条命令。

### 自定义 rustfmt.toml 放宽行宽等规则

最强理由是减少存量代码的改动量。

不采用的理由：默认规则是社区共识，放宽是为了绕过一次性迁移成本；TASK-006 已一次性完成迁移，放宽只剩长期阅读成本。


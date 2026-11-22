# 垂直切片 1：收口 D8/X25 在飞改动

**目标**：把当前未提交的 WebDAV 传输层清退改动收口到"可演示、可验收"，不扩范围。

## 演示口径

打开本地资料库同步 → 触发一次 WebDAV 同步 → 传输层只走不可变 library GET / 条件 PUT / readback，不再出现 probe / LOCK / checkpoint / MOVE 的旧调用路径；内部会话过期被正确归类为可重试错误，而非静默吞掉。

## 验收口径（草案，非已通过）

1. `cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --no-fail-fast` 在 `src-tauri/` 隔离环境绿。
2. 保留的 ignored WsgiDAV 测试按约定读写边界单独跑通，不因"看起来没消费者"就删。
3. 新增 `webdav_library_e2e.rs` 覆盖：PUT 丢响应的 OutcomeUnknown、精确字节重试不二次 PUT、异字节 Conflict 不覆盖；`session_tests.rs` 覆盖内部错误归属。
4. 全新只读上下文的独立审计确认：外部 `code` / `message` 形状未变，`stop` / `generation` 语义未变。
5. 共同门禁：`lint` / `build` / `test:frontend` / `test:components` / `test:ui-layout` / `docs:check` / `cargo fmt --check`。

## 边界

不改 schema、持久格式、真实数据、远端 DELETE，不扩远端容量上限。证据文件路径与原件哈希见 `.agents/state.md` 与归档交接包。

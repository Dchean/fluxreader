//! 集成测试共享 helper（TASK-096 测试基建收口）。
//!
//! 背景（TASK-095 基线证据，逐字固化于
//! .workflow-kit/tasks/evidence/TASK-095-baseline-flaky.md）：CI cargo test
//! 偶发失败（CI #101、2026-09-28 本地 verify 均复现）——cargo test 把同一
//! 测试二进制的多个 #[test] 作为线程并发跑在同一进程内，此前各测试文件
//! 自拼的临时库名唯一性只靠时钟纳秒，而 Windows 时钟在密集调用下精度不足
//! （实测 1000 次紧邻 `as_nanos()` 仅产生 350 个不同值），库名碰撞后两个
//! 测试互相 remove_file / 争用同一 SQLite 文件，`db::open` 建库期报
//! "table folders already exists"。实测碰撞率：subsec_nanos 2.95%、
//! as_nanos 3.05%（同等危险）；as_nanos + 进程内 AtomicU64 计数器 = 0/2000。
//!
//! 粒度说明：每个集成测试二进制会各自编译一份本模块——AtomicU64 是
//! 进程内计数，恰好是正确粒度（碰撞只发生在同进程线程间；跨二进制
//! std::process::id() 不同，天然不撞）。
//!
//! 本模块仅用 std 实现（项目 Cargo.toml 无 dev-dependencies，不新增依赖）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程内递增序号：同进程内每次调用必然不同，是唯一性的主保证；
/// pid 区分不同测试二进制，时钟纳秒只作可读性前缀。
static UNIQUE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 返回进程内唯一的临时 SQLite 库路径：`fluxreader_{base}_{pid}_{nanos}_{seq}.db`。
///
/// 保持既有 `fluxreader_<base>_…db` 命名风格；不触碰文件系统，
/// 既有调用方的 remove_file 语义原样保留（由调用方决定）。
#[allow(dead_code)] // 跨 test target 共享，未用到的 target 会报 dead_code，显式豁免（同 mock_greader 惯例）
pub fn unique_db_path(base: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = UNIQUE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("fluxreader_{base}_{pid}_{nanos}_{seq}.db"))
}

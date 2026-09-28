//! TASK-096：锁住共享 helper `common::unique_db_path` 的唯一性不变量。
//!
//! 修前实现（库名唯一性只靠时钟纳秒、无进程内计数器）在 Windows 时钟
//! 粒度不足下实测碰撞率 ~3%（TASK-095 基线：subsec_nanos 2.95%、
//! as_nanos 3.05%），表现为两个测试争用同一 SQLite 文件、db::open 报
//! "table folders already exists"——即 CI #101 与 2026-09-28 本地 verify
//! 的偶发失败根因。本测试复刻基线对照实验：Barrier 强制 4 线程同刻
//! 采样 × 2000 轮，把时钟粒度碰撞暴露为确定性失败（成对回归锚：
//! 红证 tmp/task-096/red-pure-as_nanos.log，绿证 tmp/task-096/green-helper.log）。

mod common;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

/// 4 线程 × 2000 轮 = 8000 次并发取样（与 TASK-095 基线实验同量纲）
const THREADS: usize = 4;
const ROUNDS: usize = 2000;

#[test]
fn concurrent_unique_db_path_is_pairwise_distinct() {
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut names = Vec::with_capacity(ROUNDS);
                for _ in 0..ROUNDS {
                    // 每轮把 4 线程同步到同一时刻再取样，最大化时钟粒度碰撞暴露
                    //（同 TASK-095 基线实验；断言在 join 后统一做，线程内
                    // 无 panic 路径，barrier 不会死锁）
                    barrier.wait();
                    names.push(common::unique_db_path("pressure"));
                }
                names
            })
        })
        .collect();

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut total = 0usize;
    for handle in handles {
        for name in handle.join().expect("取样线程 panic") {
            seen.insert(name);
            total += 1;
        }
    }
    assert_eq!(
        seen.len(),
        total,
        "并发取名出现重复：{total} 次调用只得到 {} 个不同路径",
        seen.len()
    );
}

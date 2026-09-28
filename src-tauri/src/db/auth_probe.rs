//! 测试专用：SQLite authorizer 计数探针（raw ffi）。
//!
//! rusqlite 0.32 的 `Connection::authorizer` 封装在 `hooks` feature 之后，本项目
//! 未启用且 Cargo.toml 不在本卡允许路径内——这里经 `rusqlite::ffi` 直接挂
//! `sqlite3_set_authorizer` 回调，不引入依赖与 feature 变更。
//! 回调在 prepare 阶段按（语句 × 访问动作 × 表/列）触发，计数稳定可复现，
//! 用于断言：
//!   · REQ-108 M-9：全部已读集合化后语句数不随条目数增长；
//!   · REQ-108 M-14：完成标记生效后重复回填零 UPDATE（零重复工作）。
//! 仅 `#[cfg(test)]` 编译，不进入产品代码。

use rusqlite::{ffi, Connection};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 计数过滤：(动作码, 表名)。None = 计全部动作。
static FILTER: Mutex<Option<(c_int, Option<String>)>> = Mutex::new(None);
static COUNT: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_auth(
    _p: *mut c_void,
    action: c_int,
    arg1: *const c_char,
    _arg2: *const c_char,
    _arg3: *const c_char,
    _arg4: *const c_char,
) -> c_int {
    let keep = match &*FILTER.lock().unwrap() {
        None => true,
        Some((code, table)) => {
            action == *code
                && table.as_deref().map_or(true, |t| {
                    !arg1.is_null() && unsafe { CStr::from_ptr(arg1) }.to_str().ok() == Some(t)
                })
        }
    };
    if keep {
        COUNT.fetch_add(1, Ordering::SeqCst);
    }
    ffi::SQLITE_OK
}

/// 在 authorizer 计数下执行 `f`，返回（结果, 回调命中数）。
/// `filter = None` 计全部动作；`Some((动作码, 表名))` 只计该表上的该动作。
pub(crate) fn with_count<T>(
    conn: &Connection,
    filter: Option<(c_int, &str)>,
    f: impl FnOnce() -> T,
) -> (T, usize) {
    *FILTER.lock().unwrap() = filter.map(|(a, t)| (a, Some(t.to_string())));
    COUNT.store(0, Ordering::SeqCst);
    let rc =
        unsafe { ffi::sqlite3_set_authorizer(conn.handle(), Some(on_auth), std::ptr::null_mut()) };
    assert_eq!(rc, ffi::SQLITE_OK, "安装 authorizer 失败");
    let out = f();
    let n = COUNT.load(Ordering::SeqCst);
    let rc = unsafe { ffi::sqlite3_set_authorizer(conn.handle(), None, std::ptr::null_mut()) };
    assert_eq!(rc, ffi::SQLITE_OK, "卸载 authorizer 失败");
    *FILTER.lock().unwrap() = None;
    (out, n)
}

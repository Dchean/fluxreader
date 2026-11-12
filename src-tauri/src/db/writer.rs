//! Process-local FIFO admission for every write to the same SQLite main file.
//! SQLite still owns atomicity, WAL visibility and inter-process locking.
//! Note: .agents/notes/implemented/architecture/2026-11-09-多连接SQLite写预留与原子设置.md

use crate::error::{AppError, AppResult};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    marker::PhantomData,
    ops::Deref,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, LazyLock, Mutex, MutexGuard, Weak,
    },
    thread::ThreadId,
    time::Duration,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Identity {
    #[cfg(windows)]
    File {
        volume: u64,
        id: [u8; 16],
    },
    #[cfg(unix)]
    File {
        device: u64,
        inode: u64,
    },
    Private(usize),
}

fn connection_id(conn: &Connection) -> usize {
    // Only used as an identity while the caller owns this connection. Never
    // dereferenced or used after close, and never compared across processes.
    unsafe { conn.handle() as usize }
}

fn identity(conn: &Connection) -> AppResult<Identity> {
    if conn.path().map_or(true, str::is_empty) {
        // Production uses private memory/temp connections, never shared-cache
        // memory URIs. They do not own a shared on-disk main file.
        return Ok(Identity::Private(connection_id(conn)));
    }
    file_identity(conn)
}

#[cfg(windows)]
fn file_identity(conn: &Connection) -> AppResult<Identity> {
    use rusqlite::ffi;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO},
    };
    let mut handle = HANDLE::default();
    // The bundled Windows VFS writes its *already open* HANDLE into this slot.
    // Do not reopen a path: case, symlinks, hardlinks and replacement files must
    // be identified by the file actually held by SQLite. SQLite owns the handle.
    let code = unsafe {
        ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
            (&mut handle as *mut HANDLE).cast(),
        )
    };
    if code != ffi::SQLITE_OK {
        return Err(AppError::new(
            "dbWriterIdentity",
            format!("SQLite 文件身份不可用（{code}），未开始写入"),
        ));
    }
    let mut info = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .map_err(|error| {
        AppError::new(
            "dbWriterIdentity",
            format!("无法读取数据库文件身份：{error}"),
        )
    })?;
    Ok(Identity::File {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
    })
}

#[cfg(unix)]
fn file_identity(conn: &Connection) -> AppResult<Identity> {
    // Development/test fallback only; the Windows product uses its live HANDLE.
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(conn.path().unwrap_or_default())
        .map_err(|error| AppError::new("dbWriterIdentity", error.to_string()))?;
    Ok(Identity::File {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(any(windows, unix)))]
fn file_identity(_conn: &Connection) -> AppResult<Identity> {
    Err(AppError::new(
        "dbWriterIdentity",
        "此平台不支持数据库文件身份",
    ))
}

#[derive(Default)]
struct Registry {
    gates: HashMap<Identity, Weak<Gate>>,
    // Only an actual failed rollback may enter this set. Keeping that file
    // failed closed until process restart is deliberate: &Connection cannot
    // safely close a still-borrowed connection or prove another owner closed it.
    failed: HashSet<Identity>,
}
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Gate {
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Default)]
struct State {
    owner: Option<Owner>,
    queue: VecDeque<u64>,
    next: u64,
    failed: bool,
}
struct Owner {
    thread: ThreadId,
    connection: usize,
    depth: usize,
}

// No user/SQLite code runs with these mutexes held. An unwind cannot partially
// commit database state here; preserve the queue instead of resetting tickets.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn failed() -> AppError {
    AppError::new("dbWriterRecovery", "数据库写入恢复失败，请重启应用后重试")
}
fn failed_after_rollback(error: impl std::fmt::Display) -> AppError {
    AppError::new(
        "dbWriterRecovery",
        format!("数据库事务恢复失败（回滚失败：{error}），请重启应用后重试"),
    )
}
fn gate(key: &Identity) -> AppResult<Arc<Gate>> {
    let mut registry = lock(&REGISTRY);
    registry.gates.retain(|_, value| value.strong_count() != 0);
    if registry.failed.contains(key) {
        return Err(failed());
    }
    if let Some(gate) = registry.gates.get(key).and_then(Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(Gate::default());
    registry.gates.insert(key.clone(), Arc::downgrade(&gate));
    Ok(gate)
}

fn fail_closed(identity: &Identity, gate: &Gate) {
    lock(&REGISTRY).failed.insert(identity.clone());
    lock(&gate.state).failed = true;
    gate.changed.notify_all();
    log::error!("SQLite writer recovery failed; this file stays closed until restart");
}

thread_local! {
    static CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}
struct CancellationScope(Option<Arc<AtomicBool>>);
impl Drop for CancellationScope {
    fn drop(&mut self) {
        CANCELLATION.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

/// Scope the existing recovery stop flag to its synchronous blocking worker.
/// It is checked before each admission, not after partially applying a write.
pub(crate) fn with_write_cancellation<T>(stop: Arc<AtomicBool>, work: impl FnOnce() -> T) -> T {
    let previous = CANCELLATION.with(|slot| slot.replace(Some(stop)));
    let _scope = CancellationScope(previous);
    work()
}

fn cancelled(stop: &Option<Arc<AtomicBool>>) -> bool {
    stop.as_ref()
        .is_some_and(|stop| stop.load(Ordering::Acquire))
}
fn cancel_error() -> AppError {
    AppError::new("cancelled", "同步已停止")
}
fn busy() -> AppError {
    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None)
        .into()
}

/// A synchronous SQL scope, including implicit writes. Reentrant only on the
/// same connection and thread. It never opens a transaction or changes timeout.
#[must_use]
pub(crate) struct WriterGuard {
    identity: Identity,
    gate: Arc<Gate>,
    // SQLite transactions/connection borrows cannot migrate between threads.
    _thread: PhantomData<Rc<()>>,
}

/// Stable pointer to the already-admitted file gate. Savepoint recovery uses
/// this instead of re-identifying the file after a failed recovery command.
#[derive(Clone)]
pub(super) struct RecoveryToken {
    identity: Identity,
    gate: Arc<Gate>,
}

impl RecoveryToken {
    pub(super) fn fail_closed(&self) {
        fail_closed(&self.identity, &self.gate);
    }
}

impl WriterGuard {
    pub(super) fn recovery_token(&self) -> RecoveryToken {
        RecoveryToken {
            identity: self.identity.clone(),
            gate: self.gate.clone(),
        }
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        if let Some(owner) = state.owner.as_mut() {
            owner.depth -= 1;
            if owner.depth == 0 {
                state.owner = None;
                self.gate.changed.notify_all();
            }
        }
    }
}

struct Waiting {
    gate: Arc<Gate>,
    ticket: u64,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        state.queue.retain(|ticket| *ticket != self.ticket);
        self.gate.changed.notify_all();
    }
}

fn acquire(conn: &Connection, key: &Identity, wait: bool) -> AppResult<WriterGuard> {
    let gate = gate(key)?;
    let stop = CANCELLATION.with(|slot| slot.borrow().clone());
    let thread = std::thread::current().id();
    let connection = connection_id(conn);
    let ticket = {
        let mut state = lock(&gate.state);
        if state.failed {
            return Err(failed());
        }
        if let Some(owner) = state.owner.as_mut() {
            if owner.thread == thread {
                if owner.connection != connection {
                    if !wait {
                        return Err(busy());
                    }
                    return Err(AppError::new(
                        "dbWriterReentry",
                        "同一线程不能在持有数据库写入时通过另一连接再次写入",
                    ));
                }
                // Existing outer atomic work must finish even if stop arrived.
                owner.depth += 1;
                return Ok(WriterGuard {
                    identity: key.clone(),
                    gate: gate.clone(),
                    _thread: PhantomData,
                });
            }
        }
        if cancelled(&stop) {
            return Err(cancel_error());
        }
        if state.owner.is_none() && state.queue.is_empty() {
            state.owner = Some(Owner {
                thread,
                connection,
                depth: 1,
            });
            return Ok(WriterGuard {
                identity: key.clone(),
                gate: gate.clone(),
                _thread: PhantomData,
            });
        }
        if !wait {
            return Err(busy());
        }
        let ticket = state.next;
        state.next = state
            .next
            .checked_add(1)
            .ok_or_else(|| AppError::internal("数据库写入排队序号已耗尽"))?;
        state.queue.push_back(ticket);
        ticket
    };
    // The waiter owns its queue entry. Cancellation/unwind withdraws only that
    // entry; the next caller can never strand a missing head ticket.
    let _waiting = Waiting {
        gate: gate.clone(),
        ticket,
    };
    let mut state = lock(&gate.state);
    loop {
        if state.failed {
            return Err(failed());
        }
        if cancelled(&stop) {
            return Err(cancel_error());
        }
        if state.owner.is_none() && state.queue.front() == Some(&ticket) {
            state.queue.pop_front();
            state.owner = Some(Owner {
                thread,
                connection,
                depth: 1,
            });
            return Ok(WriterGuard {
                identity: key.clone(),
                gate: gate.clone(),
                _thread: PhantomData,
            });
        }
        state = if stop.is_some() {
            // Cancellation polling, not a recovery cadence or a fairness sleep.
            gate.changed
                .wait_timeout(state, Duration::from_millis(25))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0
        } else {
            gate.changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
    }
}

pub(crate) fn writer_guard(conn: &Connection) -> AppResult<WriterGuard> {
    acquire(conn, &identity(conn)?, true)
}

/// The field/drop order is intentional: finish SQLite rollback before releasing
/// admission. Deref preserves helpers that explicitly accept &Transaction.
#[must_use]
pub(crate) struct WriteTransaction<'conn> {
    transaction: Option<Transaction<'conn>>,
    permit: WriterGuard,
    conn: &'conn Connection,
    identity: Identity,
}
impl<'conn> Deref for WriteTransaction<'conn> {
    type Target = Transaction<'conn>;
    fn deref(&self) -> &Self::Target {
        self.transaction.as_ref().expect("live write transaction")
    }
}
impl WriteTransaction<'_> {
    fn require_closed(&self) {
        if !self.conn.is_autocommit() {
            // No silent permit release after a rollback failure. Every waiting
            // or future writer gets a recovery error instead of a hung ticket.
            fail_closed(&self.identity, &self.permit.gate);
        }
    }
    pub(crate) fn commit(mut self) -> AppResult<()> {
        let transaction = self.transaction.take().expect("live write transaction");
        if lock(&self.permit.gate.state).failed {
            return match transaction.rollback() {
                Ok(()) => Err(failed()),
                Err(error) => {
                    log::error!(
                        "SQLite outer rollback failed after SAVEPOINT recovery failure: {error}"
                    );
                    fail_closed(&self.identity, &self.permit.gate);
                    Err(failed_after_rollback(error))
                }
            };
        }
        let result = transaction.commit().map_err(AppError::from);
        // rusqlite's consumed Transaction also attempts rollback on COMMIT error.
        self.require_closed();
        result
    }
}
impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        if let Some(transaction) = self.transaction.take() {
            if self.conn.is_autocommit() {
                // SQLite can already have rolled back a transaction after an
                // error. Its default Drop recognizes that terminal state.
                drop(transaction);
            } else if let Err(error) = transaction.rollback() {
                log::error!("SQLite writer rollback failed: {error}");
            }
            self.require_closed();
        }
        // `permit` drops only after this method and SQLite rollback have ended.
    }
}

fn transaction(conn: &Connection, wait: bool) -> AppResult<WriteTransaction<'_>> {
    if !conn.is_autocommit() {
        return Err(AppError::new(
            "db",
            "cannot start a transaction within a transaction",
        ));
    }
    let identity = identity(conn)?;
    let permit = acquire(conn, &identity, wait)?;
    let transaction = match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
        Ok(transaction) => transaction,
        Err(error) => {
            // A failed BEGIN normally remains autocommit. If SQLite reports an
            // active transaction despite the error, close it before admission
            // is released, or keep this actual file failed closed.
            if !conn.is_autocommit() {
                if let Err(rollback) = conn.execute_batch("ROLLBACK") {
                    log::error!("SQLite failed-BEGIN rollback failed: {rollback}");
                }
                if !conn.is_autocommit() {
                    fail_closed(&identity, &permit.gate);
                }
            }
            return Err(error.into());
        }
    };
    Ok(WriteTransaction {
        transaction: Some(transaction),
        permit,
        conn,
        identity,
    })
}

pub(crate) fn write_transaction(conn: &Connection) -> AppResult<WriteTransaction<'_>> {
    transaction(conn, true)
}

/// Maintenance preserves its existing busy=skip policy and cannot jump a waiter.
pub(crate) fn try_write_transaction(conn: &Connection) -> AppResult<WriteTransaction<'_>> {
    transaction(conn, false)
}

#[cfg(test)]
pub(super) fn clear_recovery_for_test(conn: &Connection) {
    let Ok(identity) = identity(conn) else {
        return;
    };
    lock(&REGISTRY).failed.remove(&identity);
    if let Some(gate) = lock(&REGISTRY).gates.get(&identity).and_then(Weak::upgrade) {
        let mut state = lock(&gate.state);
        state.failed = false;
        gate.changed.notify_all();
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;

//! Production immutable intent objects; never pretend that a sparse patch is a
//! complete checkpoint. Exact storage ACK and causal application are distinct.
use super::{
    database, logical,
    model::*,
    storage::{self, ROOT},
    transport_error,
};
use crate::{
    db,
    error::{AppError, AppResult},
    webdav_transport::{Batch, Intent, PreparedBatch},
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::VecDeque, sync::Arc, time::Instant};
use tokio::sync::Mutex;

pub struct Outgoing {
    pub path: String,
    pub bytes: Vec<u8>,
    pub first: u64,
    pub last: u64,
}
pub fn prepare(conn: &Connection, generation: i64) -> AppResult<Option<Outgoing>> {
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let current:Option<(i64,String,Vec<u8>,u64,u64)>=tx.query_row("SELECT generation,path,bytes,first_seq,last_seq FROM local_sync_fast_pending WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if let Some((epoch, path, bytes, first, last)) = current {
        if epoch != generation {
            tx.execute("DELETE FROM local_sync_fast_pending WHERE id=1", [])?;
        } else {
            tx.commit()?;
            return Ok(Some(Outgoing {
                path,
                bytes,
                first,
                last,
            }));
        }
    }
    let (actor,library,after):(String,String,u64)=tx.query_row("SELECT actor,library_id,MAX(fast_stored_seq,published_seq,COALESCE(CAST((SELECT value FROM settings WHERE key='local_sync_seed_seq') AS INTEGER),0)) FROM local_sync_control WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let mut intents = Vec::new();
    {
        let mut stmt=tx.prepare("SELECT seq,lamport,observed,patch FROM local_sync_journal WHERE seq>?1 ORDER BY seq LIMIT 256")?;
        for row in stmt.query_map([after], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (seq, lamport, observed, patch) = row?;
            if seq != after + intents.len() as u64 + 1 {
                return Err(invalid("本机快批序号存在缺口"));
            }
            intents.push(Intent {
                seq,
                lamport,
                observed: serde_json::from_str(&observed)?,
                patch: serde_json::from_str(&patch)?,
            });
            if let Err(error) = PreparedBatch::new(Batch {
                format: 2,
                library_id: library.clone(),
                actor: actor.clone(),
                intents: intents.clone(),
            }) {
                intents.pop();
                if intents.is_empty() {
                    return Err(transport_error(error));
                }
                break;
            }
        }
    }
    if intents.is_empty() {
        tx.commit()?;
        return Ok(None);
    }
    let first = intents[0].seq;
    let last = intents.last().ok_or_else(|| invalid("空快批"))?.seq;
    let prepared = PreparedBatch::new(Batch {
        format: 2,
        library_id: library,
        actor: actor.clone(),
        intents,
    })
    .map_err(transport_error)?;
    let path = format!(
        "{ROOT}actors/{actor}/intents/{}-{}.json",
        prepared.id(),
        storage::random_id(&tx)?
    );
    let bytes = prepared.bytes().to_vec();
    tx.execute("INSERT INTO local_sync_fast_pending(id,generation,actor,first_seq,last_seq,path,bytes) VALUES(1,?1,?2,?3,?4,?5,?6)",params![generation,actor,first,last,path,bytes])?;
    tx.execute("INSERT INTO local_sync_fast_objects(path,actor,first_seq,last_seq,bytes,generation) VALUES(?1,?2,?3,?4,?5,?6)",params![path,actor,first,last,bytes,generation])?;
    tx.commit()?;
    Ok(Some(Outgoing {
        path,
        bytes,
        first,
        last,
    }))
}
pub fn acknowledge(conn: &Connection, generation: i64, out: &Outgoing) -> AppResult<()> {
    let tx = db::write_transaction(conn)?;
    database::require_generation(&tx, generation)?;
    let matched:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_fast_pending WHERE id=1 AND generation=?1 AND path=?2 AND bytes=?3 AND first_seq=?4 AND last_seq=?5)",params![generation,out.path,out.bytes,out.first,out.last],|r|r.get(0))?;
    if !matched {
        return Err(AppError::new("staleLocalSync", "快批回执与当前字节不符"));
    }
    tx.execute(
        "UPDATE local_sync_control SET fast_stored_seq=MAX(fast_stored_seq,?1) WHERE id=1",
        [out.last],
    )?;
    tx.execute("DELETE FROM local_sync_fast_pending WHERE id=1", [])?;
    // This is NOT a checkpoint ACK. Journal and body obligations remain.
    tx.commit()?;
    Ok(())
}
pub fn has_outgoing(conn: &Connection) -> AppResult<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_fast_pending) OR EXISTS(SELECT 1 FROM local_sync_journal WHERE seq>(SELECT MAX(fast_stored_seq,published_seq,COALESCE(CAST((SELECT value FROM settings WHERE key='local_sync_seed_seq') AS INTEGER),0)) FROM local_sync_control WHERE id=1))",[],|r|r.get(0))?)
}

/// Only the send gate owns this function. Peer directory/GET work never holds it.
pub(super) async fn send(
    database: &Arc<Mutex<Connection>>,
    session: &super::runtime::Session,
    budget: &mut storage::Budget,
    directories_ready: bool,
) -> AppResult<bool> {
    super::runtime::ensure_session(database, session).await?;
    let generation = session.generation;
    let guard = session.clone();
    let outgoing = super::runtime::blocking(database.clone(), move |conn| {
        let _writer = db::writer_guard(conn)?;
        super::runtime::check_session(conn, &guard)?;
        prepare(conn, generation)
    })
    .await?;
    if let Some(out) = outgoing {
        if !directories_ready {
            for directory in [
                format!("{ROOT}actors/{}/", session.actor),
                format!("{ROOT}actors/{}/intents/", session.actor),
            ] {
                budget.take(1)?;
                super::runtime::ensure_session(database, session).await?;
                session
                    .transport
                    .library_mkdir(&directory)
                    .await
                    .map_err(transport_error)?;
            }
        }
        budget.take(3)?;
        session
            .transport
            .library_store_guarded(&out.path, &out.bytes, || {
                super::runtime::transport_guard(database, session)
            })
            .await
            .map_err(transport_error)?;
        super::runtime::ensure_session(database, session).await?;
        let guard = session.clone();
        super::runtime::blocking(database.clone(), move |conn| {
            let _writer = db::writer_guard(conn)?;
            super::runtime::check_session(conn, &guard)?;
            acknowledge(conn, generation, &out)
        })
        .await?;
        return Ok(true);
    }
    Ok(false)
}

/// A listed page is at most 256 names (Transport's parser limit). Retain the
/// unconsumed names across scheduling slices so a slow PROPFIND cannot starve
/// its GETs. This cursor belongs to one runtime session and is not an ACK.
#[derive(Default)]
pub(super) struct PeerCursor {
    names: Option<VecDeque<String>>,
}
impl PeerCursor {
    pub(super) fn is_pending(&self) -> bool {
        self.names.is_some()
    }
}

fn batch_digest(name: &str) -> AppResult<&str> {
    let (hash, attempt) = name
        .strip_suffix(".json")
        .and_then(|s| s.split_once('-'))
        .ok_or_else(|| invalid("快批对象名无效"))?;
    if !valid_hex(hash, 64) || !valid_hex(attempt, 32) {
        return Err(invalid("快批对象身份无效"));
    }
    Ok(hash)
}

async fn admit_request(
    database: &Arc<Mutex<Connection>>,
    session: &super::runtime::Session,
    budget: &mut storage::Budget,
    deadline: Instant,
) -> AppResult<bool> {
    super::runtime::ensure_session(database, session).await?;
    if budget.remaining() == 0 || Instant::now() >= deadline {
        return Ok(false);
    }
    budget.take(1)?;
    Ok(true)
}

/// At most one list and four new batch GETs per selected peer. Deadline and
/// request count admit each request separately; an admitted request completes
/// within Transport's timeout, then its result is guarded and saved before any
/// later request starts. The runtime rotates peers even with a pending cursor.
pub(super) async fn receive_peer(
    database: &Arc<Mutex<Connection>>,
    session: &super::runtime::Session,
    peer: &str,
    budget: &mut storage::Budget,
    deadline: Instant,
    cursor: &mut PeerCursor,
) -> AppResult<bool> {
    let directory = format!("{ROOT}actors/{peer}/intents/");
    if cursor.names.is_none() {
        if !admit_request(database, session, budget, deadline).await? {
            return Ok(false);
        }
        let listing = session.transport.library_list(&directory).await;
        super::runtime::ensure_session(database, session).await?;
        let mut names = match listing {
            Err(e) if e.kind == crate::webdav_transport::ErrorKind::Http(404) => return Ok(false),
            other => other.map_err(transport_error)?,
        };
        names.sort();
        // A malformed listing must be refreshed on retry, rather than pinning
        // an invalid name in an in-memory continuation indefinitely.
        for name in &names {
            batch_digest(name)?;
        }
        cursor.names = Some(names.into());
    }
    let names = cursor
        .names
        .as_mut()
        .ok_or_else(|| invalid("快批目录游标缺失"))?;
    let mut received = 0;
    while let Some(name) = names.front() {
        let hash = batch_digest(name)?;
        let checked_peer = peer.to_owned();
        let checked_hash = hash.to_owned();
        let guard = session.clone();
        let seen = super::runtime::blocking(database.clone(), move |conn| {
            super::runtime::check_session(conn, &guard)?;
            database::require_generation(conn, guard.generation)?;
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM local_sync_seen_batches WHERE actor=?1 AND digest=?2)",
                params![checked_peer, checked_hash],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .await?;
        if seen {
            names.pop_front();
            continue;
        }
        if received >= 4 || !admit_request(database, session, budget, deadline).await? {
            break;
        }
        let fetched = session
            .transport
            .library_get(&format!("{directory}{name}"), 256 * 1024)
            .await;
        super::runtime::ensure_session(database, session).await?;
        let Some(bytes) = fetched.map_err(transport_error)? else {
            // A stale list is not evidence of no work. Ask the state lane
            // for an authoritative replacement checkpoint; never fake ACK.
            names.pop_front();
            if names.is_empty() {
                cursor.names = None;
            }
            return Ok(true);
        };
        if digest(&bytes) != hash {
            return Err(invalid("快批摘要不符"));
        }
        let batch: Batch = serde_json::from_slice(&bytes)?;
        if batch.actor != peer {
            return Err(invalid("快批actor与路径不符"));
        }
        super::runtime::ensure_session(database, session).await?;
        let guard = session.clone();
        let hash = hash.to_owned();
        super::runtime::blocking(database.clone(), move |conn| {
            {
                let _writer = db::writer_guard(conn)?;
                super::runtime::check_session(conn, &guard)?;
                // ingest registers every pending actor/dependency and revision in
                // its same transaction before any GC can use the actor set.
                logical::receive_batch(conn, guard.generation, &batch, Some(&hash))?;
            }
            for _ in 0..64 {
                // Each original transaction has its own FIFO turn. A waiting
                // foreground intent may run before the next local application.
                let _writer = db::writer_guard(conn)?;
                super::runtime::check_session(conn, &guard)?;
                if !logical::apply_ready_intent(conn)? {
                    break;
                }
            }
            Ok(())
        })
        .await?;
        names.pop_front();
        received += 1;
        tokio::task::yield_now().await;
    }
    if names.is_empty() {
        cursor.names = None;
    }
    Ok(false)
}

pub(super) fn missing_dependencies(
    conn: &Connection,
) -> AppResult<std::collections::BTreeSet<String>> {
    let current = logical::frontier(conn)?;
    let mut out = std::collections::BTreeSet::new();
    let mut stmt = conn.prepare("SELECT intent FROM local_sync_intents ORDER BY actor,seq")?;
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let intent: Intent = serde_json::from_str(&row?)?;
        for (actor, seq) in intent.observed {
            if current.get(&actor).copied().unwrap_or(0) < seq {
                out.insert(actor);
            }
        }
    }
    Ok(out)
}

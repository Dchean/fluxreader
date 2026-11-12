//! Pure local WebDAV synchronization. The module intentionally has no dependency
//! on the legacy Gist configuration snapshot or RSS service sync account.

pub mod backup;
pub mod body_archive;
mod capture;
pub mod database;
pub mod fast;
pub mod logical;
pub mod model;
pub mod runtime;
pub mod storage;

use crate::error::AppError;
use crate::webdav_transport::{ErrorKind, TransportError};

pub(crate) fn transport_error(error: TransportError) -> AppError {
    let code = match error.kind {
        ErrorKind::Network => "localSyncNetwork",
        ErrorKind::OutcomeUnknown => "localSyncUnknown",
        ErrorKind::TooLarge => "localSyncCapacity",
        ErrorKind::Conflict => "localSyncConflict",
        ErrorKind::InvalidXml | ErrorKind::InvalidState => "localSyncFormat",
        ErrorKind::InvalidInput => "validate",
        ErrorKind::LeaseExpired => "localSyncLease",
        ErrorKind::Http(404) => "localSyncNotFound",
        ErrorKind::Http(_) => "localSyncHttp",
    };
    AppError::new(code, error.message)
}

#[cfg(test)]
mod tests {
    use crate::{
        db,
        local_sync::{database, model, storage},
    };
    use rusqlite::Connection;

    fn conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("memory db");
        db::MIGRATIONS.to_latest(&mut conn).expect("schema");
        database::register_functions(&conn).expect("functions");
        conn
    }

    #[test]
    fn initializes_from_stable_source_identity_and_captures_field_changes() {
        let conn = conn();
        let folder =
            db::create_folder_with_sync_id(&conn, "Reading", "article", Some(&"a".repeat(32)))
                .unwrap();
        let feed = db::insert_feed(
            &conn,
            "https://example.test/feed.xml",
            None,
            "Example",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let source: String = conn
            .query_row("SELECT sync_id FROM feeds WHERE id=?1", [feed], |r| {
                r.get(0)
            })
            .unwrap();
        database::initialize(&conn, &"b".repeat(32)).unwrap();
        let initial = database::checkpoint(&conn).unwrap();
        assert!(initial.fields.contains_key(&format!("s/{source}/feed_url")));
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
            .unwrap();
        conn.execute("UPDATE feeds SET title='Renamed' WHERE id=?1", [feed])
            .unwrap();
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after,
            before + 1,
            "one business transaction emits one local operation"
        );
        let state = database::checkpoint(&conn).unwrap();
        assert_eq!(
            model::value(&state.fields, &format!("s/{source}"), "title").and_then(|v| v.as_str()),
            Some("Renamed")
        );
    }

    #[test]
    fn remote_apply_suppresses_capture_and_stale_source_does_not_reactivate() {
        let conn = conn();
        let folder =
            db::create_folder_with_sync_id(&conn, "Reading", "article", Some(&"a".repeat(32)))
                .unwrap();
        let feed = db::insert_feed(
            &conn,
            "https://example.test/feed.xml",
            None,
            "Example",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        let source: String = conn
            .query_row("SELECT sync_id FROM feeds WHERE id=?1", [feed], |r| {
                r.get(0)
            })
            .unwrap();
        database::initialize(&conn, &"b".repeat(32)).unwrap();
        conn.execute("UPDATE feeds SET active=0 WHERE id=?1", [feed])
            .unwrap();
        let state = database::checkpoint(&conn).unwrap();
        // A lower-stamp ordinary source field never creates an activation record;
        // the deletion tombstone remains part of the joined state.
        assert!(!model::active_source(&state.fields, &format!("s/{source}")));
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
            .unwrap();
        database::merge_apply(&conn, database::generation(&conn).unwrap(), &state).unwrap();
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM local_sync_journal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, before,
            "remote join is applying=true and must not echo local intents"
        );
    }

    #[test]
    fn sealed_state_has_valid_checkpoint_and_reuses_immutable_object_bytes() {
        let conn = conn();
        let folder =
            db::create_folder_with_sync_id(&conn, "Reading", "article", Some(&"a".repeat(32)))
                .unwrap();
        db::insert_feed(
            &conn,
            "https://example.test/feed.xml",
            None,
            "Example",
            None,
            folder,
            "inherit",
            false,
            false,
        )
        .unwrap();
        database::initialize(&conn, &"c".repeat(32)).unwrap();
        let generation = database::generation(&conn).unwrap();
        let pending = storage::seal_state(&conn, generation).unwrap();
        assert!(pending.is_some());
        let pending = pending.unwrap();
        pending.manifest.validate().unwrap();
        let first = pending
            .manifest
            .shards
            .iter()
            .map(|s| s.digest.clone())
            .collect::<Vec<_>>();
        storage::acknowledge(&conn, generation, &pending.manifest).unwrap();
        assert!(
            storage::seal_state(&conn, generation).unwrap().is_none(),
            "unchanged state does not create a full re-upload"
        );
        assert!(!first.is_empty());
    }
}

#[cfg(test)]
mod database_tests;

//! 状态写回 ACK ≠ 已应用：GReader 写后读回确认回归。
//!
//! 判别力：旧实现只要 edit-tag 返回 200/OK 就按队列 id 出队；本文件的未知
//! target、读回失败与批量部分缺失用例都会在旧路径下错误显示成功。
//!
//! 运行：cargo test --test sync_ack_verification_e2e

mod common;
mod mock_greader;

use app_lib::db;
use app_lib::sync;
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

fn setup(
    name: &str,
    server: &MockGReader,
) -> (
    common::TempDbGuard,
    Arc<Mutex<rusqlite::Connection>>,
    reqwest::Client,
) {
    let tmp = common::unique_db_path(&format!("ack_verify_{name}"));
    let guard = common::TempDbGuard::new(tmp.clone());
    let conn = db::open(&tmp).expect("open db");
    db::set_setting(&conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(&conn, "greader_username", "test").unwrap();
    db::set_setting(&conn, "greader_password", "test-token").unwrap();
    if let Some(account) = sync::current_account(&conn).unwrap() {
        // states_phase 用例只关心推送段；把订阅发现节流在测试窗口内压住，
        // 避免本地夹具被远端默认订阅列表改写。
        db::mark_zero_binding_discovery_attempt(
            &conn,
            &account.key,
            chrono::Utc::now().timestamp(),
        )
        .unwrap();
    }
    (
        guard,
        Arc::new(Mutex::new(conn)),
        app_lib::ingestion::build_client(10),
    )
}

fn seed_article(conn: &rusqlite::Connection, remote_id: i64) -> i64 {
    seed_article_in(
        conn,
        "https://ack.example/rss",
        format!("ack-{remote_id}"),
        format!("https://ack.example/{remote_id}"),
        remote_id,
    )
}

fn seed_article_in(
    conn: &rusqlite::Connection,
    feed_url: &str,
    guid: impl AsRef<str>,
    url: impl AsRef<str>,
    remote_id: i64,
) -> i64 {
    let guid = guid.as_ref();
    let url = url.as_ref();
    let url_norm = db::normalize_url(url);
    conn.execute(
        "INSERT OR IGNORE INTO feeds (feed_url, title)
         VALUES (?1, 'ACK')",
        [feed_url],
    )
    .unwrap();
    let feed_id: i64 = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            [feed_url],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO articles (feed_id, guid, title, url, url_norm)
         VALUES (?1, ?2, 'ACK entry', ?3, ?4)",
        rusqlite::params![feed_id, guid, url, url_norm],
    )
    .unwrap();
    let article_id = conn.last_insert_rowid();
    db::set_article_remote_id(conn, article_id, remote_id).unwrap();
    article_id
}

fn seed_unbound_article(
    conn: &rusqlite::Connection,
    feed_url: &str,
    article_url: &str,
    remote_feed_id: i64,
) -> i64 {
    conn.execute(
        "INSERT OR IGNORE INTO feeds (feed_url, title)
         VALUES (?1, 'Unbound')",
        [feed_url],
    )
    .unwrap();
    let feed_id: i64 = conn
        .query_row(
            "SELECT id FROM feeds WHERE feed_url = ?1",
            [feed_url],
            |r| r.get(0),
        )
        .unwrap();
    db::set_feed_remote_id(conn, feed_id, remote_feed_id).unwrap();
    conn.execute(
        "INSERT INTO articles (feed_id, guid, title, url, url_norm)
         VALUES (?1, ?2, 'Unbound entry', ?3, ?4)",
        rusqlite::params![
            feed_id,
            format!("unbound-{article_url}"),
            article_url,
            db::normalize_url(article_url)
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn queue_row(conn: &rusqlite::Connection) -> Option<(String, Option<String>, Option<String>)> {
    conn.query_row(
        "SELECT status, last_error_class, last_error
           FROM sync_queue ORDER BY id LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

#[tokio::test]
async fn normal_read_write_readback_confirms_and_prunes() {
    let server = MockGReader::start().await.expect("start mock");
    let remote_id = server.add_entry_ret(10, "https://ack.example/normal", "normal", false, false);
    let (_guard, db, http) = setup("normal", &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, remote_id);
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    {
        let conn = db.lock().await;
        assert!(
            db::take_sync_queue(&conn).unwrap().is_empty(),
            "读回一致后才允许出队"
        );
    }
    let entries = server.entries.lock().unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.id == remote_id && entry.read));
}

#[tokio::test]
async fn unknown_target_with_ok_ack_stays_retry_wait_inconclusive() {
    let server = MockGReader::start().await.expect("start mock");
    let (_guard, db, http) = setup("missing", &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, 9001);
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "目标缺失不得被 ACK 出队");
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error.as_deref().unwrap_or("").contains("inconclusive"),
        "空响应必须报告 inconclusive：{last_error:?}"
    );
    let read: i64 = conn
        .query_row("SELECT is_read FROM articles WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(read, 1, "本地读状态必须保留");
}

#[tokio::test]
async fn readback_network_failure_keeps_retry_wait() {
    let server = MockGReader::start().await.expect("start mock");
    let remote_id = server.add_entry_ret(
        10,
        "https://ack.example/readback-fail",
        "readback",
        false,
        false,
    );
    server.set_fail_item_contents(true);
    let (_guard, db, http) = setup("readback_fail", &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, remote_id);
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "读回失败不得出队");
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error.as_deref().unwrap_or("").contains("读回失败"),
        "网络/协议错误必须保留可见原因：{last_error:?}"
    );
    drop(conn);
    let entries = server.entries.lock().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == remote_id && entry.read),
        "写请求已应用；失败只属于读回确认"
    );
}

#[tokio::test]
async fn short_readback_keeps_only_the_missing_target_in_retry_wait() {
    let server = MockGReader::start().await.expect("start mock");
    let present = server.add_entry_ret(
        10,
        "https://ack.example/batch-present",
        "present",
        false,
        false,
    );
    let missing = present + 900;
    let (_guard, db, http) = setup("partial", &server);
    {
        let conn = db.lock().await;
        let present_aid = seed_article(&conn, present);
        db::set_read_with_enqueue(&conn, present_aid, true).unwrap();
        let missing_aid = seed_article(&conn, missing);
        db::set_read_with_enqueue(&conn, missing_aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "只保留目标缺失的队项");
    assert_eq!(queued[0].article_id, Some(2));
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error.as_deref().unwrap_or("").contains("inconclusive"),
        "短响应必须报告 inconclusive：{last_error:?}"
    );
    drop(conn);
    let entries = server.entries.lock().unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.id == present && entry.read));
}

async fn assert_contents_response_keeps_verification_failure(
    name: &str,
    status: u16,
    body: &str,
    expected_in_error: &str,
) {
    let server = MockGReader::start().await.expect("start mock");
    let remote_id = server.add_entry_ret(
        10,
        &format!("https://ack.example/{name}"),
        name,
        false,
        false,
    );
    server.set_contents_response(status, body);
    let (_guard, db, http) = setup(name, &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, remote_id);
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "读回不可确认时不得出队");
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error
            .as_deref()
            .unwrap_or("")
            .contains(expected_in_error),
        "错误摘要应包含 {expected_in_error:?}：{last_error:?}"
    );
    drop(conn);
    let entries = server.entries.lock().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == remote_id && entry.read),
        "写请求已经发送并应用；失败只属于读回确认"
    );
}

#[tokio::test]
async fn empty_items_response_is_inconclusive() {
    assert_contents_response_keeps_verification_failure(
        "empty_items",
        200,
        r#"{"items":[]}"#,
        "inconclusive",
    )
    .await;
}

#[tokio::test]
async fn unrelated_item_does_not_make_short_response_complete() {
    assert_contents_response_keeps_verification_failure(
        "unrelated_item",
        200,
        r#"{"items":[{"id":"991","title":"unrelated","categories":[]}]}"#,
        "短于请求",
    )
    .await;
}

#[tokio::test]
async fn duplicate_items_do_not_make_short_response_complete() {
    let server = MockGReader::start().await.expect("start mock");
    let present = server.add_entry_ret(
        10,
        "https://ack.example/duplicate-present",
        "present",
        false,
        false,
    );
    let missing = present + 900;
    let duplicate_body = format!(
        r#"{{"items":[
            {{"id":"{present}","title":"present","categories":["user/-/state/com.google/read"]}},
            {{"id":"{present}","title":"present","categories":["user/-/state/com.google/read"]}}
        ]}}"#
    );
    server.set_contents_response(200, duplicate_body);
    let (_guard, db, http) = setup("duplicate_items", &server);
    {
        let conn = db.lock().await;
        let present_aid = seed_article(&conn, present);
        db::set_read_with_enqueue(&conn, present_aid, true).unwrap();
        let missing_aid = seed_article(&conn, missing);
        db::set_read_with_enqueue(&conn, missing_aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "重复条目只能确认自身，不能冒充另一个目标");
    assert_eq!(queued[0].article_id, Some(2));
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error.as_deref().unwrap_or("").contains("短于请求"),
        "重复条目凑出的长度不能算完整响应：{last_error:?}"
    );
}

#[tokio::test]
async fn missing_items_field_is_verification_failure() {
    assert_contents_response_keeps_verification_failure(
        "missing_items",
        200,
        r#"{"total":0}"#,
        "读回失败",
    )
    .await;
}

#[tokio::test]
async fn malformed_contents_body_is_verification_failure() {
    assert_contents_response_keeps_verification_failure(
        "malformed_contents",
        200,
        "<html>not json</html>",
        "读回失败",
    )
    .await;
}

#[tokio::test]
async fn broadcast_primary_confirmed_sibling_missing_settles_with_partial_warning() {
    let server = MockGReader::start().await.expect("start mock");
    let shared_url = "https://ack.example/shared";
    let primary = server.add_entry_ret(10, shared_url, "primary", false, false);
    let sibling = primary + 100;
    let (_guard, db, http) = setup("broadcast_partial", &server);
    {
        let conn = db.lock().await;
        let primary_aid = seed_article_in(
            &conn,
            "https://ack.example/feed-a",
            "broadcast-primary",
            shared_url,
            primary,
        );
        let sibling_aid = seed_article_in(
            &conn,
            "https://ack.example/feed-b",
            "broadcast-sibling",
            shared_url,
            sibling,
        );
        // 副本先标读（不再产生本地意图/队列行）；主行标读时仍会把它纳入
        // read 广播目标。这样本用例只检验「唯一队列行的主目标 + 副本」聚合。
        db::set_read(&conn, sibling_aid, true).unwrap();
        db::set_read_with_enqueue(&conn, primary_aid, true).unwrap();
        let sibling_read: i64 = conn
            .query_row(
                "SELECT is_read FROM articles WHERE id = ?1",
                [sibling_aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sibling_read, 1, "副本预置为已读，不产生额外队列行");
    }

    let report = sync::states_phase(&db, &http, false)
        .await
        .expect("states phase");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("partial") && e.contains("广播副本")),
        "主目标确认后，缺失副本应作为 partial warning 可见：{:?}",
        report.errors
    );

    let conn = db.lock().await;
    assert!(
        db::take_sync_queue(&conn).unwrap().is_empty(),
        "主目标确认即可结算队列行"
    );
    drop(conn);
    assert!(server
        .entries
        .lock()
        .unwrap()
        .iter()
        .any(|entry| entry.id == primary && entry.read));
}

#[tokio::test]
async fn unbound_queue_item_is_pushed_after_pull_binding_in_same_states_round() {
    let server = MockGReader::start().await.expect("start mock");
    let article_url = "https://ack.example/pull-bind";
    let remote_id = server.add_entry_ret(10, article_url, "pull-bind", false, false);
    let (_guard, db, http) = setup("pull_bind", &server);
    {
        let conn = db.lock().await;
        let aid =
            seed_unbound_article(&conn, "https://ack.example/pull-bind-feed", article_url, 10);
        db::set_read_with_enqueue(&conn, aid, true).unwrap();
        let (status, _class, _last_error) = queue_row(&conn).expect("queued before pull");
        assert_eq!(status, "bindable_retry", "首轮推送前身份应未绑定");
    }

    sync::states_phase(&db, &http, true)
        .await
        .expect("states phase with pull binding");

    {
        let conn = db.lock().await;
        let remote_id_bound: Option<i64> = conn
            .query_row(
                "SELECT remote_id FROM articles WHERE url = ?1",
                [article_url],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remote_id_bound,
            Some(remote_id),
            "pull 应在同一 states 轮把文章绑定到远端条目"
        );
        assert!(
            db::take_sync_queue(&conn).unwrap().is_empty(),
            "pull 后补推成功并出队"
        );
    }
    let updates = server.status_updates.lock().unwrap();
    assert_eq!(
        updates
            .iter()
            .filter(|(id, action)| *id == remote_id && action.as_str() == "read")
            .count(),
        1,
        "同一 states 轮应在 pull 绑定后补推一次 read：{updates:?}"
    );
}

async fn assert_star_readback_failure(name: &str, body_template: &str) {
    let server = MockGReader::start().await.expect("start mock");
    let remote_id = server.add_entry_ret(
        10,
        &format!("https://ack.example/{name}"),
        name,
        false,
        false,
    );
    server.set_contents_response(200, body_template.replace("{id}", &remote_id.to_string()));
    let (_guard, db, http) = setup(name, &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, remote_id);
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    let conn = db.lock().await;
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 1, "star ACK 不得直接出队");
    assert_eq!(queued[0].status, db::STATUS_RETRY_WAIT);
    let (status, class, last_error) = queue_row(&conn).expect("queue row");
    assert_eq!(status, "retry_wait");
    assert_eq!(class.as_deref(), Some("verification"));
    assert!(
        last_error.as_deref().unwrap_or("").contains("inconclusive"),
        "star 读回未确认必须报告 inconclusive：{last_error:?}"
    );
    drop(conn);
    let entries = server.entries.lock().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == remote_id && entry.starred),
        "mark 已成功应用；失败只属于读回确认"
    );
}

#[tokio::test]
async fn star_readback_empty_contents_keeps_retry_wait() {
    assert_star_readback_failure("star_empty", r#"{"items":[]}"#).await;
}

#[tokio::test]
async fn star_readback_missing_categories_is_inconclusive() {
    assert_star_readback_failure(
        "star_missing_categories",
        r#"{"items":[{"id":"{id}","title":"target"}]}"#,
    )
    .await;
}

#[tokio::test]
async fn star_readback_opposite_state_is_inconclusive() {
    assert_star_readback_failure(
        "star_opposite",
        r#"{"items":[{"id":"{id}","title":"target","categories":[]}]}"#,
    )
    .await;
}

#[tokio::test]
async fn star_write_also_requires_readback_confirmation() {
    let server = MockGReader::start().await.expect("start mock");
    let remote_id = server.add_entry_ret(10, "https://ack.example/star", "star", false, false);
    let (_guard, db, http) = setup("star", &server);
    {
        let conn = db.lock().await;
        let aid = seed_article(&conn, remote_id);
        db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    }

    sync::push_states_now(&db, &http).await;

    {
        let conn = db.lock().await;
        assert!(db::take_sync_queue(&conn).unwrap().is_empty());
    }
    let entries = server.entries.lock().unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.id == remote_id && entry.starred));
}

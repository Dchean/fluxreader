//! 2026-11-09 已确认产品契约：重构前先固定外部可观察结果。
//! 全部调用生产入口；红灯表示尚未达到新契约，不使用 ignore/xfail。
//! 旧 read-wins / 跨源广播政策的替换范围见本轮后端门禁交付说明。

mod common;
mod mock_greader;

use app_lib::{commands, db, ingestion, sync};
use mock_greader::MockGReader;
use std::sync::Arc;
use tokio::sync::Mutex;

fn article(guid: &str, url: &str, body: &str) -> db::NewArticle {
    db::NewArticle {
        guid: guid.into(),
        url: Some(url.into()),
        title: "验收文章".into(),
        author: None,
        summary: None,
        content_html: Some(format!("<p>{body}</p>")),
        body_text: body.into(),
        image_url: None,
        enclosure_url: None,
        enclosure_mime: None,
        duration_sec: None,
        published_at: Some("2020-01-01T00:00:00Z".into()),
        source: "direct".into(),
    }
}

fn feed(conn: &rusqlite::Connection, suffix: &str) -> i64 {
    let folder = db::create_folder(conn, &format!("分类{suffix}"), "article").unwrap();
    db::insert_feed(
        conn,
        &format!("https://acceptance.invalid/{suffix}.xml"),
        None,
        suffix,
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap()
}

fn open(
    name: &str,
) -> (
    common::TempDbGuard,
    rusqlite::Connection,
    std::path::PathBuf,
) {
    let path = common::unique_db_path(&format!("refactor_{name}"));
    let guard = common::TempDbGuard::new(path.clone());
    let conn = db::open(&path).unwrap();
    (guard, conn, path)
}

fn query(feed_id: Option<i64>) -> db::ArticleQuery {
    db::ArticleQuery {
        feed_id,
        folder_id: None,
        only_unread: false,
        only_starred: false,
        only_today: false,
        newest_first: true,
        limit: 100,
        offset: 0,
        with_content: true,
        layout: None,
        last_published: None,
        last_id: None,
    }
}

fn state(conn: &rusqlite::Connection, id: i64) -> (bool, bool) {
    let row = db::get_article(conn, id).unwrap().expect("article exists");
    (row.is_read, row.is_starred)
}

#[test]
fn same_feed_same_link_different_content_remains_two_articles() {
    let (_guard, conn, _) = open("different_content");
    let fid = feed(&conn, "a");
    let (first, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("guid-a", "https://acceptance.invalid/shared", "通知 A"),
        true,
    )
    .unwrap();
    db::set_starred(&conn, first, true).unwrap();
    let (second, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("guid-b", "https://acceptance.invalid/shared", "通知 B"),
        true,
    )
    .unwrap();
    assert_ne!(first, second, "同链接不能覆盖同源不同正文的独立条目");
    let rows = db::list_articles(&conn, &query(Some(fid))).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        db::get_article(&conn, first)
            .unwrap()
            .unwrap()
            .content_html
            .as_deref(),
        Some("<p>通知 A</p>")
    );
    assert_eq!(state(&conn, first), (false, true));
    assert_eq!(state(&conn, second), (false, false));
}

#[test]
fn same_feed_equal_content_is_shown_once_despite_distinct_links() {
    let (_guard, conn, _) = open("equal_content");
    let fid = feed(&conn, "a");
    for suffix in ["a", "b"] {
        db::upsert_article_with_feed(
            &conn,
            fid,
            &article(
                suffix,
                &format!("https://acceptance.invalid/{suffix}"),
                "相同正文",
            ),
            true,
        )
        .unwrap();
    }
    let rows = db::list_articles(&conn, &query(Some(fid))).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "同源内容相同应去重；链接不同不能阻止内容去重"
    );
}

#[test]
fn source_content_group_applies_all_four_actions_and_splits_on_stable_guid_revision() {
    let (_guard, conn, _) = open("group_actions");
    let fid = feed(&conn, "a");
    let first = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("a", "https://acceptance.invalid/a", "同文"),
        true,
    )
    .unwrap()
    .0;
    let second = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("b", "https://acceptance.invalid/b", "同文"),
        true,
    )
    .unwrap()
    .0;
    assert_ne!(first, second, "真实GUID身份保留，不物理吞并");
    for desired in [true, false] {
        db::set_read_with_enqueue(&conn, first, desired).unwrap();
        db::set_starred_with_enqueue(&conn, first, desired).unwrap();
        assert_eq!(state(&conn, first), (desired, desired));
        assert_eq!(state(&conn, second), (desired, desired));
        assert_eq!(
            db::take_sync_queue(&conn).unwrap().len(),
            4,
            "每身份每字段都有可确认的队项"
        );
    }
    let revised = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("b", "https://acceptance.invalid/b", "修订后不同正文"),
        true,
    )
    .unwrap()
    .0;
    assert_eq!(revised, second, "同稳定GUID修订原行");
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        2
    );
    db::set_read_with_enqueue(&conn, first, true).unwrap();
    db::set_starred_with_enqueue(&conn, first, true).unwrap();
    assert_eq!(
        state(&conn, second),
        (false, false),
        "内容分叉之后不再闭包状态"
    );
}

#[test]
fn different_enclosures_and_titles_never_share_a_display_group() {
    let (_guard, conn, _) = open("group_attachments");
    let fid = feed(&conn, "podcast");
    for suffix in ["a", "b"] {
        let mut item = article(
            suffix,
            &format!("https://acceptance.invalid/{suffix}"),
            "相同节目说明",
        );
        item.enclosure_url = Some(format!("https://acceptance.invalid/{suffix}.mp3"));
        item.enclosure_mime = Some("audio/mpeg".into());
        db::upsert_article_with_feed(&conn, fid, &item, true).unwrap();
    }
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        2
    );
    let mut title_a = article("c", "https://acceptance.invalid/c", "通知正文");
    title_a.title = "release A".into();
    let mut title_b = article("d", "https://acceptance.invalid/d", "通知正文");
    title_b.title = "release B".into();
    db::upsert_article_with_feed(&conn, fid, &title_a, true).unwrap();
    db::upsert_article_with_feed(&conn, fid, &title_b, true).unwrap();
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        4
    );
}

#[test]
fn media_and_light_member_updates_share_version_without_returning_bodies() {
    let (_guard, conn, _) = open("media_updates");
    let fid = feed(&conn, "a");
    let mut item = article("a", "https://acceptance.invalid/a", "正文");
    item.content_html=Some("<p>正文</p><img src=\"https://acceptance.invalid/1.jpg\"><img src=\"https://acceptance.invalid/2.jpg\"><img src=\"https://acceptance.invalid/1.jpg\">".into());
    let aid = db::upsert_article_with_feed(&conn, fid, &item, true)
        .unwrap()
        .0;
    let detail = db::get_article(&conn, aid).unwrap().unwrap();
    assert_eq!(
        detail.images,
        vec![
            "https://acceptance.invalid/1.jpg",
            "https://acceptance.invalid/2.jpg"
        ]
    );
    assert_eq!(detail.image_count, 2);
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    let updates = db::get_article_updates(&conn, &[aid, 999_999]).unwrap();
    assert_eq!(updates.missing_ids, vec![999_999]);
    assert_eq!(updates.items.len(), 1);
    let row = &updates.items[0];
    assert!(row.is_read);
    assert!(row.content_html.is_none());
    assert_eq!(row.image_count, 2);
    assert_eq!(row.media_revision, detail.media_revision);
    assert_eq!(row.content_id, detail.content_id);
    assert!(!row.body_missing);
}

#[tokio::test]
async fn same_link_different_remote_content_cannot_claim_early_local_intent() {
    let (_guard, conn, _) = open("binding_content_guard");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let aid = db::upsert_article_with_feed(
        &conn,
        fid,
        &article(
            "early",
            "https://acceptance.invalid/shared",
            "完全不同的本机正文",
        ),
        true,
    )
    .unwrap()
    .0;
    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://acceptance.invalid/shared",
        "验收文章",
        false,
        false,
    );
    let dbm = Arc::new(Mutex::new(conn));
    let report = sync::states_phase(&dbm, &ingestion::build_client(10), true)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = dbm.lock().await;
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        2
    );
    let remote: Option<i64> = conn
        .query_row("SELECT remote_id FROM articles WHERE id=?1", [aid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(remote, None, "URL候选不等于内容确认");
    assert_eq!(
        db::take_sync_queue(&conn).unwrap().len(),
        1,
        "未绑定不能伪确认本机收藏"
    );
    assert!(
        !server
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == rid)
            .unwrap()
            .starred
    );
}

fn two_sources(conn: &rusqlite::Connection) -> (i64, i64, i64) {
    let first_feed = feed(conn, "a");
    let second_feed = feed(conn, "b");
    let a = article(
        "same-guid",
        "https://acceptance.invalid/shared",
        "完全相同正文",
    );
    let (first, _) = db::upsert_article_with_feed(conn, first_feed, &a, true).unwrap();
    let (second, _) = db::upsert_article_with_feed(conn, second_feed, &a, true).unwrap();
    (first_feed, first, second)
}

#[test]
fn cross_feed_equal_content_is_visible_as_two_independent_articles() {
    let (_guard, conn, _) = open("cross_feed_display");
    db::set_setting(&conn, "app_settings", r#"{"smartDedup":true}"#).unwrap();
    let (_, a, b) = two_sources(&conn);
    let rows = db::list_articles(&conn, &query(None)).unwrap();
    let ids: std::collections::HashSet<_> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        [a, b].into_iter().collect(),
        "跨源同文不得在全部视图隐藏"
    );
}

#[test]
fn single_read_never_changes_or_enqueues_another_feed() {
    let (_guard, conn, _) = open("cross_feed_read");
    let (_, a, b) = two_sources(&conn);
    db::set_read_with_enqueue(&conn, a, true).unwrap();
    assert_eq!(state(&conn, a), (true, false));
    assert_eq!(state(&conn, b), (false, false), "标读不能传播到另一源");
    assert!(db::take_sync_queue(&conn)
        .unwrap()
        .iter()
        .all(|q| q.article_id == Some(a)));
}

#[test]
fn mark_feed_read_keeps_same_content_outside_its_scope_unread() {
    let (_guard, conn, _) = open("scoped_read");
    let (fid, a, b) = two_sources(&conn);
    db::mark_all_read_with_enqueue(&conn, Some(fid), None, false, None, None).unwrap();
    assert_eq!(state(&conn, a), (true, false));
    assert_eq!(
        state(&conn, b),
        (false, false),
        "源内全部已读不能越过源边界"
    );
}

#[test]
fn local_state_and_latest_intents_survive_reopen_and_enqueue_failure_is_atomic() {
    let (_guard, conn, path) = open("transaction");
    let fid = feed(&conn, "a");
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("a", "https://acceptance.invalid/a", "a"),
        true,
    )
    .unwrap();
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    db::set_read_with_enqueue(&conn, aid, false).unwrap();
    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    drop(conn);
    let conn = db::open(&path).unwrap();
    assert_eq!(state(&conn, aid), (false, true));
    let actions: std::collections::HashSet<_> = db::take_sync_queue(&conn)
        .unwrap()
        .into_iter()
        .map(|q| q.action)
        .collect();
    assert_eq!(
        actions,
        ["unread".to_owned(), "star".to_owned()]
            .into_iter()
            .collect()
    );
    conn.execute_batch("CREATE TEMP TRIGGER reject_intent BEFORE INSERT ON sync_queue BEGIN SELECT RAISE(ABORT, 'test queue write failure'); END;").unwrap();
    assert!(db::set_starred_with_enqueue(&conn, aid, false).is_err());
    assert_eq!(state(&conn, aid), (false, true), "入队失败必须回滚本地值");
    assert_eq!(
        db::take_sync_queue(&conn).unwrap().len(),
        2,
        "原队项不能被失败事务删除"
    );
}

#[test]
fn cache_expiry_protects_unread_starred_and_read_identity_without_retaining_expired_body() {
    let (_guard, mut conn, _) = open("retention");
    let fid = feed(&conn, "a");
    let mut ids = Vec::new();
    for suffix in ["unread", "starred", "read"] {
        ids.push(
            db::upsert_article_with_feed(
                &conn,
                fid,
                &article(
                    suffix,
                    &format!("https://acceptance.invalid/{suffix}"),
                    suffix,
                ),
                true,
            )
            .unwrap()
            .0,
        );
    }
    db::set_read(&conn, ids[1], true).unwrap();
    db::set_starred(&conn, ids[1], true).unwrap();
    db::set_read(&conn, ids[2], true).unwrap();
    assert_eq!(db::cleanup_cache(&mut conn, 1, "articles").unwrap().0, 1);
    let unread = db::get_article(&conn, ids[0]).unwrap().unwrap();
    assert!(!unread.is_read);
    assert_eq!(unread.content_html.as_deref(), Some("<p>unread</p>"));
    let starred = db::get_article(&conn, ids[1]).unwrap().unwrap();
    assert!(starred.is_starred);
    assert_eq!(starred.content_html.as_deref(), Some("<p>starred</p>"));
    // 允许轻量stub或独立状态账本；不能锁死为“必须删除整行”。无论采用哪种
    // 存储形态，详情不能继续返回过期正文，同一稳定身份重新出现后仍必须已读。
    if let Some(expired) = db::get_article(&conn, ids[2]).unwrap() {
        assert!(expired.content_html.as_deref().unwrap_or("").is_empty());
        assert!(expired.is_read);
    }
    let replay = article("read", "https://acceptance.invalid/read", "read");
    let (replayed, _) = db::upsert_article_with_feed(&conn, fid, &replay, true).unwrap();
    assert_eq!(
        state(&conn, replayed),
        (true, false),
        "正文过期后，同一条目再次被抓取不能忘记已读而复活未读"
    );
}

#[test]
fn unsubscribe_keeps_only_starred_articles_readable() {
    let (_guard, conn, _) = open("unsubscribe");
    let fid = feed(&conn, "a");
    let (saved, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("saved", "https://acceptance.invalid/saved", "收藏正文"),
        true,
    )
    .unwrap();
    let (unread, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("unread", "https://acceptance.invalid/unread", "未读正文"),
        true,
    )
    .unwrap();
    db::set_starred(&conn, saved, true).unwrap();
    commands::record_feed_deletion(&conn, fid).unwrap();
    assert!(
        db::get_article(&conn, unread).unwrap().is_none(),
        "退订不保留普通未读"
    );
    let kept = db::get_article(&conn, saved)
        .unwrap()
        .expect("退订必须保留收藏");
    assert!(kept.is_starred);
    assert_eq!(kept.content_html.as_deref(), Some("<p>收藏正文</p>"));
}

#[test]
fn folder_rename_to_existing_name_is_rejected_without_partial_changes() {
    let (_guard, conn, _) = open("folder_rename");
    db::create_folder(&conn, "A", "article").unwrap();
    let b = db::create_folder(&conn, "B", "article").unwrap();
    assert!(
        commands::record_folder_rename(&conn, b, "A").is_err(),
        "普通改名应遵守创建时的重名约束"
    );
    assert_eq!(db::folder_name(&conn, b).unwrap().as_deref(), Some("B"));
    assert!(db::folder_tombstones(&conn).unwrap().is_empty());
}

fn configure(conn: &rusqlite::Connection, server: &MockGReader) {
    db::set_setting(conn, "greader_endpoint", &server.url()).unwrap();
    db::set_setting(conn, "greader_username", "test").unwrap();
    db::set_setting(conn, "greader_password", "test-token").unwrap();
}

#[tokio::test]
async fn cache_expiry_keeps_unconfirmed_read_intent_deliverable() {
    let (_guard, mut conn, _) = open("retention_pending");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://acceptance.invalid/pending",
        "验收文章",
        false,
        false,
    );
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("pending", "https://acceptance.invalid/pending", "待推正文"),
        true,
    )
    .unwrap();
    db::set_article_remote_id(&conn, aid, rid).unwrap();
    // This delivery/retry fixture represents an earlier Content feed/10 response.
    assert!(db::confirm_article_remote_origin(&conn, aid, rid, 10).unwrap());
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    let before = db::take_sync_queue(&conn).unwrap();
    assert_eq!(before.len(), 1);
    db::cleanup_cache(&mut conn, 1, "articles").unwrap();
    let after = db::take_sync_queue(&conn).unwrap();
    assert_eq!(after.len(), 1, "清理不能级联丢失尚未确认的意图");
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[0].article_id, Some(aid));
    let dbm = Arc::new(Mutex::new(conn));
    sync::push_states_now(&dbm, &ingestion::build_client(10)).await;
    assert!(db::take_sync_queue(&*dbm.lock().await).unwrap().is_empty());
    let entries = server.entries.lock().unwrap();
    assert!(entries.iter().find(|e| e.id == rid).unwrap().read);
}

#[tokio::test]
async fn greader_light_follows_remote_unread_and_both_star_directions_outside_content_window() {
    let (_guard, conn, _) = open("gr_bidirectional");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let rid = server.add_entry_with_published(
        10,
        "https://acceptance.invalid/a",
        "验收文章",
        false,
        true,
        1_600_000_000,
    );
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("a", "https://acceptance.invalid/a", "a"),
        true,
    )
    .unwrap();
    db::set_article_remote_id(&conn, aid, rid).unwrap();
    db::set_read(&conn, aid, true).unwrap();
    db::set_last_sync_ts(&conn, 1_700_000_000).unwrap();
    let dbm = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(10);
    let report = sync::states_phase(&dbm, &http, false).await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        state(&*dbm.lock().await, aid),
        (false, true),
        "手机重新未读必须返回桌面，即使正文不在增量窗口"
    );
    {
        let mut entries = server.entries.lock().unwrap();
        entries[0].read = true;
        entries[0].starred = false;
    }
    let report = sync::states_phase(&dbm, &http, false).await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(state(&*dbm.lock().await, aid), (true, false));
}

#[tokio::test]
async fn greader_failed_push_retains_latest_intents_then_retries_all_four_assignments() {
    let (_guard, conn, _) = open("gr_retry");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let rid = server.add_entry_ret(10, "https://acceptance.invalid/a", "验收文章", false, false);
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("a", "https://acceptance.invalid/a", "a"),
        true,
    )
    .unwrap();
    db::set_article_remote_id(&conn, aid, rid).unwrap();
    // This delivery/retry fixture represents an earlier Content feed/10 response.
    assert!(db::confirm_article_remote_origin(&conn, aid, rid, 10).unwrap());
    let dbm = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(10);
    for desired in [true, false] {
        {
            let conn = dbm.lock().await;
            db::set_read_with_enqueue(&conn, aid, desired).unwrap();
            db::set_starred_with_enqueue(&conn, aid, desired).unwrap();
        }
        server.set_fail_edit_tag(true);
        sync::push_states_now(&dbm, &http).await;
        {
            let conn = dbm.lock().await;
            assert_eq!(state(&conn, aid), (desired, desired));
            assert_eq!(db::take_sync_queue(&conn).unwrap().len(), 2);
        }
        server.set_fail_edit_tag(false);
        sync::push_states_now(&dbm, &http).await;
        assert!(db::take_sync_queue(&*dbm.lock().await).unwrap().is_empty());
        let entries = server.entries.lock().unwrap();
        let remote = entries.iter().find(|e| e.id == rid).unwrap();
        assert_eq!((remote.read, remote.starred), (desired, desired));
    }
}

#[tokio::test]
async fn direct_early_article_stays_unconfirmed_until_binding_then_pushes_saved_intent() {
    let (_guard, conn, _) = open("early_binding");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let (aid, _) = db::upsert_article_with_feed(
        &conn,
        fid,
        &article(
            "early-guid",
            "https://acceptance.invalid/early",
            "Mock fetched content",
        ),
        true,
    )
    .unwrap();
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    let dbm = Arc::new(Mutex::new(conn));
    let http = ingestion::build_client(10);
    sync::push_states_now(&dbm, &http).await;
    {
        let conn = dbm.lock().await;
        let pending = db::take_sync_queue(&conn).unwrap();
        assert_eq!(pending.len(), 2, "没有远端身份时不能伪确认");
        assert!(pending
            .iter()
            .all(|q| q.status == db::STATUS_BINDABLE_RETRY));
        assert_eq!(state(&conn, aid), (true, true));
    }
    let rid = server.add_entry_ret(
        10,
        "https://acceptance.invalid/early",
        "验收文章",
        false,
        false,
    );
    let report = sync::states_phase(&dbm, &http, true).await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = dbm.lock().await;
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        1,
        "远端迟到副本应绑定本机早到正文"
    );
    assert_eq!(state(&conn, aid), (true, true));
    assert!(
        db::take_sync_queue(&conn).unwrap().is_empty(),
        "绑定后补推并读回确认才清队"
    );
    let entries = server.entries.lock().unwrap();
    let remote = entries.iter().find(|e| e.id == rid).unwrap();
    assert_eq!((remote.read, remote.starred), (true, true));
}

#[tokio::test]
async fn retained_favorite_ignores_remote_absence_and_resubscription_restores_sync() {
    let (_guard, conn, _) = open("retained_sync");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let rid = server.add_entry_ret(10, "https://acceptance.invalid/a", "验收文章", true, false);
    let aid = db::upsert_article_with_feed(
        &conn,
        fid,
        &article("a", "https://acceptance.invalid/a", "Mock fetched content"),
        true,
    )
    .unwrap()
    .0;
    db::set_article_remote_id(&conn, aid, rid).unwrap();
    db::set_starred(&conn, aid, true).unwrap();
    let source = db::list_article_sources(&conn).unwrap().remove(0).sync_id;
    commands::record_feed_deletion(&conn, fid).unwrap();
    let dbm = Arc::new(Mutex::new(conn));
    let report = sync::states_phase(&dbm, &ingestion::build_client(10), false)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = dbm.lock().await;
    let saved = db::get_article(&conn, aid).unwrap().unwrap();
    assert!(
        saved.is_starred && saved.retained,
        "远端取消收藏/移除源不取消本地退订保留收藏"
    );
    let folder = db::create_folder(&conn, "重新订阅", "social").unwrap();
    let reused = db::insert_feed(
        &conn,
        "https://acceptance.invalid/a.xml",
        None,
        "a",
        None,
        folder,
        "inherit",
        false,
        false,
    )
    .unwrap();
    assert_eq!(reused, fid);
    assert_eq!(db::list_feeds(&conn).unwrap()[0].sync_id, source);
    assert!(db::sync_match_maps(&conn)
        .unwrap()
        .mf_id_to_article
        .contains_key(&rid));
    drop(conn);
    sync::states_phase(&dbm, &ingestion::build_client(10), false)
        .await
        .unwrap();
    assert!(
        !db::get_article(&*dbm.lock().await, aid)
            .unwrap()
            .unwrap()
            .is_starred,
        "重新订阅后重新接受服务端状态"
    );
}

#[test]
fn reading_members_and_explicit_day_window_survive_later_filter_changes() {
    let (_guard, conn, _) = open("reading_members");
    let fid = feed(&conn, "a");
    let mut ids = Vec::new();
    for (guid, date) in [
        ("day-a", "2026-11-08T23:59:00+08:00"),
        ("day-b", "2026-11-09T00:01:00+08:00"),
    ] {
        let mut item = article(guid, &format!("https://acceptance.invalid/{guid}"), guid);
        item.published_at = Some(date.into());
        ids.push(
            db::upsert_article_with_feed(&conn, fid, &item, true)
                .unwrap()
                .0,
        );
    }
    let start = chrono::DateTime::parse_from_rfc3339("2026-11-08T00:00:00+08:00")
        .unwrap()
        .timestamp_millis();
    let end = start + 86_400_000;
    let mut q = query(Some(fid));
    q.only_today = true;
    q.only_unread = true;
    let members = db::list_article_member_ids(&conn, &q, Some((start, end))).unwrap();
    assert_eq!(
        members,
        vec![ids[0]],
        "today窗口按会话显式日界，不用当前墙钟"
    );
    db::set_read_with_enqueue(&conn, ids[0], true).unwrap();
    assert!(db::list_article_member_ids(&conn, &q, Some((start, end)))
        .unwrap()
        .is_empty());
    let updates = db::get_article_updates(&conn, &members).unwrap();
    assert_eq!(
        updates.items.len(),
        1,
        "已捕获成员仍可按ID更新属性，不受新未读筛选移除"
    );
    assert!(updates.items[0].is_read);
    q.only_unread = false;
    assert_eq!(
        db::article_index_in_window(&conn, &q, ids[0], Some((start, end))).unwrap(),
        Some(0)
    );
    assert_eq!(
        db::article_index_in_window(&conn, &q, ids[1], Some((start, end))).unwrap(),
        None
    );
}

#[tokio::test]
async fn source_local_group_pending_survives_partial_ack_and_stale_pull() {
    let (_guard, conn, _) = open("group_partial_ack");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "a");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let mut ids = Vec::new();
    for suffix in ["a", "b"] {
        let url = format!("https://acceptance.invalid/{suffix}");
        let rid = server.add_entry_ret(10, &url, "验收文章", false, false);
        let aid = db::upsert_article_with_feed(
            &conn,
            fid,
            &article(suffix, &url, "Mock fetched content"),
            true,
        )
        .unwrap()
        .0;
        db::set_article_remote_id(&conn, aid, rid).unwrap();
        ids.push(aid);
    }
    db::set_read_with_enqueue(&conn, ids[0], true).unwrap();
    let queued = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queued.len(), 2, "两个真实目标各有持久意图");
    db::prune_sync(&conn, &[queued[0].id]).unwrap();
    server.set_fail_edit_tag(true);
    let database = Arc::new(Mutex::new(conn));
    let report = sync::states_phase(&database, &ingestion::build_client(10), true)
        .await
        .unwrap();
    assert!(!report.errors.is_empty(), "第二个目标失败必须可见");
    let conn = database.lock().await;
    for id in ids {
        assert!(
            db::get_article(&conn, id).unwrap().unwrap().is_read,
            "组内仍有pending时，已ACK成员也不得被同轮旧未读回滚"
        );
    }
    assert_eq!(
        db::take_sync_queue(&conn).unwrap().len(),
        1,
        "ACK只删除自己的精确队项，失败成员继续保留"
    );
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn scheduled_worker_retries_without_click_and_accepts_next_idle_notification() {
    use std::time::Duration;
    let (_guard, conn, _) = open("worker_retry");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "worker");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let rid = server.add_entry_ret(
        10,
        "https://acceptance.invalid/worker",
        "验收文章",
        false,
        false,
    );
    let aid = db::upsert_article_with_feed(
        &conn,
        fid,
        &article(
            "worker",
            "https://acceptance.invalid/worker",
            "Mock fetched content",
        ),
        true,
    )
    .unwrap()
    .0;
    db::set_article_remote_id(&conn, aid, rid).unwrap();
    // This delivery/retry fixture represents an earlier Content feed/10 response.
    assert!(db::confirm_article_remote_origin(&conn, aid, rid, 10).unwrap());
    db::set_read_with_enqueue(&conn, aid, true).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let client = ingestion::build_client(3);
    server.set_fail_edit_tag(true);
    commands::schedule_state_push_for_db(&database, &client);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if db::sync_queue_stats(&*database.lock().await)
                .unwrap()
                .retry_wait
                == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("初次推送应失败并留下持久重试状态");
    server.set_fail_edit_tag(false);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if db::take_sync_queue(&*database.lock().await)
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("没有第二次点击也应自动重试并读回确认");
    assert!(
        server
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == rid)
            .unwrap()
            .read
    );
    db::set_starred_with_enqueue(&*database.lock().await, aid, true).unwrap();
    commands::schedule_state_push_for_db(&database, &client);
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if db::take_sync_queue(&*database.lock().await)
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("空闲接收器必须继续处理后一次通知");
    assert!(
        server
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == rid)
            .unwrap()
            .starred
    );
    let runtime = sync::runtime::for_db(&database);
    assert!(runtime.drain(Duration::from_secs(2)).await);
    db::set_read_with_enqueue(&*database.lock().await, aid, false).unwrap();
    commands::schedule_state_push_for_db(&database, &client);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(
        db::take_sync_queue(&*database.lock().await).unwrap().len(),
        1,
        "停止后的worker不得发出新请求或删除意图"
    );
    let weak = Arc::downgrade(&database);
    drop(database);
    assert!(weak.upgrade().is_none(), "worker空闲或停止都不得强持数据库");
}

#[test]
fn retained_unstar_pending_identity_is_hidden_from_lists_search_and_counts_until_ack() {
    let (_guard, conn, _) = open("retained_pending_visibility");
    let fid = feed(&conn, "retained");
    let aid = db::upsert_article_with_feed(
        &conn,
        fid,
        &article(
            "retained",
            "https://acceptance.invalid/retained",
            "retained body",
        ),
        true,
    )
    .unwrap()
    .0;
    db::set_starred(&conn, aid, true).unwrap();
    db::set_article_remote_id(&conn, aid, 42).unwrap();
    commands::record_feed_deletion(&conn, fid).unwrap();
    db::set_starred_with_enqueue(&conn, aid, false).unwrap();
    assert!(
        db::get_article(&conn, aid).unwrap().is_some(),
        "轻身份保留供精确未确认意图使用"
    );
    assert_eq!(db::take_sync_queue(&conn).unwrap().len(), 1);
    assert!(db::list_articles(&conn, &query(None)).unwrap().is_empty());
    assert!(db::search_articles(&conn, "验收文章", 100)
        .unwrap()
        .is_empty());
    assert_eq!(db::scope_counts(&conn, None).unwrap().all.total, 0);
    assert!(db::feed_counts(&conn).unwrap().is_empty());
    assert_eq!(
        db::mark_all_read_with_enqueue(&conn, None, None, false, None, None).unwrap(),
        0
    );
    let id = db::take_sync_queue(&conn).unwrap()[0].id;
    db::prune_sync(&conn, &[id]).unwrap();
    assert!(
        db::get_article(&conn, aid).unwrap().is_none(),
        "确认后才回收retained非收藏轻实体"
    );
    let source_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM source_tombstones", [], |r| r.get(0))
        .unwrap();
    assert_eq!(source_count, 1, "源排除墓碑独立于收藏生命周期持久化");
}

#[test]
fn source_local_group_queries_cover_twenty_and_fifty_thousand_mixed_rows() {
    use std::time::Instant;
    for size in [20_000_i64, 50_000] {
        let (_guard, mut conn, _) = open(&format!("group_scale_{size}"));
        let folder = db::create_folder(&conn, "Scale", "article").unwrap();
        for source in 0..20 {
            db::insert_feed(
                &conn,
                &format!("https://scale.invalid/{source}"),
                None,
                "Scale",
                None,
                folder,
                "inherit",
                false,
                false,
            )
            .unwrap();
        }
        let tx = conn.transaction().unwrap();
        tx.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000)
            INSERT INTO contents(identity,content_html,body_text) SELECT 'scale-'||x,'<p>scale '||x||'</p>','scale '||x FROM n;").unwrap();
        tx.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<?1)
            INSERT INTO articles(feed_id,guid,title,content_id,rss_content_id,published_at,is_read,is_starred)
            SELECT ((x-1)%20)+1,'scale-guid-'||x,'Article '||((x-1)/2),((x-1)%1000)+1,
                ((x-1)%1000)+1,'2026-11-01T00:00:00Z',x%3=0,x%7=0 FROM n",[size]).unwrap();
        // Exact source-local duplicate pairs, plus same RSS content in different sources.
        tx.execute(
            "UPDATE articles SET title='Pair '||((id-1)/40),rss_content_id=((id-1)/40)%1000+1
            WHERE id%4=0",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let started = Instant::now();
        let rows = db::list_articles(&conn, &query(None)).unwrap();
        let list_time = started.elapsed();
        assert_eq!(rows.len(), 100);
        let started = Instant::now();
        let counts = db::scope_counts(&conn, None).unwrap();
        let count_time = started.elapsed();
        assert!(counts.all.total > 0 && counts.all.total <= size);
        assert_eq!(
            counts.feeds.iter().map(|f| f.total).sum::<i64>(),
            counts.all.total,
            "不同源不聚合，因此全局计数等于各源和"
        );
        let started = Instant::now();
        let changed = db::mark_all_read_with_enqueue(&conn, None, None, false, None, None).unwrap();
        let write_time = started.elapsed();
        assert!(changed > 0);
        assert_eq!(db::scope_counts(&conn, None).unwrap().all.unread, 0);
        let queued = db::take_sync_queue(&conn).unwrap().len();
        let touched: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM articles WHERE read_version=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            queued, touched,
            "每个承接组意图并推进版本的实体都有自己的队项"
        );
        assert!(queued >= changed && queued <= size as usize);
        println!("GROUP_SCALE rows={size} list_ms={} counts_ms={} mark_all_ms={} displayed={} changed={changed}",
            list_time.as_millis(),count_time.as_millis(),write_time.as_millis(),counts.all.total);
    }
}

#[tokio::test]
async fn real_rss_guid_named_like_remote_id_cannot_bypass_content_proof() {
    let (_guard, conn, _) = open("remote_guid_namespace");
    let server = MockGReader::start().await.unwrap();
    configure(&conn, &server);
    let fid = feed(&conn, "collision");
    db::set_feed_remote_id(&conn, fid, 10).unwrap();
    let url = "https://acceptance.invalid/collision";
    let rid = server.add_entry_ret(10, url, "验收文章", false, false);
    let aid = db::upsert_article_with_feed(
        &conn,
        fid,
        &article(&format!("remote-{rid}"), url, "different local body"),
        true,
    )
    .unwrap()
    .0;
    db::set_starred_with_enqueue(&conn, aid, true).unwrap();
    let database = Arc::new(Mutex::new(conn));
    let report = sync::states_phase(&database, &ingestion::build_client(3), true)
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let conn = database.lock().await;
    assert_eq!(
        conn.query_row("SELECT remote_id FROM articles WHERE id=?1", [aid], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .unwrap(),
        None
    );
    assert_eq!(
        db::get_article(&conn, aid)
            .unwrap()
            .unwrap()
            .content_html
            .as_deref(),
        Some("<p>different local body</p>")
    );
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        2
    );
    assert_eq!(
        db::take_sync_queue(&conn).unwrap().len(),
        1,
        "不同正文的本机意图继续等待真实绑定"
    );
    assert!(!server.entries.lock().unwrap()[0].starred);
}

#[test]
fn detail_hydration_uses_group_flags_and_cleanup_keeps_protected_group_together() {
    let (_guard, mut conn, _) = open("group_detail_cleanup");
    let fid = feed(&conn, "group");
    let mut ids = Vec::new();
    for suffix in ["a", "b"] {
        ids.push(
            db::upsert_article_with_feed(
                &conn,
                fid,
                &article(
                    suffix,
                    &format!("https://acceptance.invalid/{suffix}"),
                    "group body",
                ),
                true,
            )
            .unwrap()
            .0,
        );
    }
    db::set_read(&conn, ids[0], true).unwrap();
    db::set_starred(&conn, ids[1], true).unwrap();
    let listed = db::list_articles(&conn, &query(Some(fid)))
        .unwrap()
        .remove(0);
    let detail = db::get_article(&conn, listed.id).unwrap().unwrap();
    let hydrated = db::get_articles(&conn, &[listed.id]).unwrap().remove(0);
    assert_eq!((listed.is_read, listed.is_starred), (false, true));
    assert_eq!((detail.is_read, detail.is_starred), (false, true));
    assert_eq!((hydrated.is_read, hydrated.is_starred), (false, true));
    assert_eq!(
        db::cleanup_cache(&mut conn, 30, "articles").unwrap().0,
        0,
        "可见未读/收藏组不能因清除其中已读成员而分裂"
    );
    assert_eq!(
        db::list_articles(&conn, &query(Some(fid))).unwrap().len(),
        1
    );
    assert!(
        !db::get_article(&conn, ids[0])
            .unwrap()
            .unwrap()
            .body_missing
    );
}

#[test]
fn mark_all_read_queues_already_read_members_of_an_unread_group_atomically() {
    let (_guard, conn, _) = open("mark_all_mixed_group");
    let fid = feed(&conn, "mixed");
    let mut ids = Vec::new();
    for suffix in ["a", "b"] {
        ids.push(
            db::upsert_article_with_feed(
                &conn,
                fid,
                &article(
                    suffix,
                    &format!("https://acceptance.invalid/{suffix}"),
                    "same group",
                ),
                true,
            )
            .unwrap()
            .0,
        );
    }
    db::set_read(&conn, ids[0], true).unwrap();
    conn.execute_batch(&format!("CREATE TRIGGER fail_second_group_intent BEFORE INSERT ON sync_queue WHEN NEW.article_id={} BEGIN SELECT RAISE(ABORT,'group queue failure'); END;",ids[1])).unwrap();
    assert!(db::mark_all_read_with_enqueue(&conn, Some(fid), None, false, None, None).is_err());
    assert!(
        db::take_sync_queue(&conn).unwrap().is_empty(),
        "队列写失败必须全组回滚"
    );
    let before: Vec<(i64, i64)> = conn
        .prepare("SELECT is_read,read_version FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(before, vec![(1, 1), (0, 0)]);
    conn.execute_batch("DROP TRIGGER fail_second_group_intent;")
        .unwrap();
    assert_eq!(
        db::mark_all_read_with_enqueue(&conn, Some(fid), None, false, None, None).unwrap(),
        1,
        "返回值是实际翻转行数，不是底层入队目标数"
    );
    let after: Vec<(i64, i64)> = conn
        .prepare("SELECT is_read,read_version FROM articles ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        after,
        vec![(1, 2), (1, 1)],
        "已读成员同样推进版本以保护本次组意图"
    );
    let queue = db::take_sync_queue(&conn).unwrap();
    assert_eq!(queue.len(), 2, "两真实远端身份都需要自己的确认");
    assert!(queue.iter().all(|item| item.action == "read"));
    assert_eq!(
        db::mark_all_read_with_enqueue(&conn, Some(fid), None, false, None, None).unwrap(),
        0
    );
    assert_eq!(
        db::take_sync_queue(&conn)
            .unwrap()
            .iter()
            .map(|q| q.id)
            .collect::<Vec<_>>(),
        queue.iter().map(|q| q.id).collect::<Vec<_>>(),
        "已经全读时不重建队项"
    );
    conn.execute("DELETE FROM sync_queue", []).unwrap();
    conn.execute(
        "UPDATE articles SET read_version=?1 WHERE id=?2",
        rusqlite::params![i64::MAX, ids[0]],
    )
    .unwrap();
    conn.execute("UPDATE articles SET is_read=0 WHERE id=?1", [ids[1]])
        .unwrap();
    assert!(
        db::mark_all_read_with_enqueue(&conn, Some(fid), None, false, None, None).is_err(),
        "已读成员版本溢出同样必须拒绝全组操作"
    );
    assert!(db::take_sync_queue(&conn).unwrap().is_empty());
    assert_eq!(
        conn.query_row("SELECT is_read FROM articles WHERE id=?1", [ids[1]], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

//! 同步正确性回归：真实生产入口、SQLite/DPAPI 与 loopback 协议响应。
//! 只使用合成凭据及内存数据库，不访问真实账号。由2026-11-05/06审计反例转正。
//! 本测试二进制注册进程级会话缓存，因此每个场景先取得同一串行锁。
#[cfg(test)]
mod tests {
    use app_lib::{db, ingestion, sync};
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex as StdMutex,
    };
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{Mutex, Notify, Semaphore};

    const FEED_URL: &str = "https://probe.invalid/rss";
    type Database = Arc<Mutex<Connection>>;

    async fn serialize() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::const_new(());
        LOCK.lock().await
    }

    #[derive(Clone)]
    struct Entry {
        id: i64,
        feed: Option<i64>,
        link: Option<String>,
        read: bool,
        star: bool,
        published: i64,
    }

    impl Entry {
        fn gr(&self) -> Value {
            let mut categories = vec!["user/-/state/com.google/reading-list"];
            if self.read {
                categories.push("user/-/state/com.google/read");
            }
            if self.star {
                categories.push("user/-/state/com.google/starred");
            }
            let mut value = json!({
                "id": format!("tag:google.com,2005:reader/item/{:016x}", self.id),
                "title": format!("Probe {}", self.id),
                "published": self.published,
                "updated": self.published,
                "categories": categories,
                "alternate": self.link.as_ref().map(|link| vec![json!({"href":link})]).unwrap_or_default(),
                "summary": {"content": format!("<p>actual production content {}</p>", self.id)}
            });
            if let Some(feed) = self.feed {
                value["origin"] = json!({"streamId": format!("feed/{feed}"), "title":"Probe"});
            }
            value
        }

        fn fever(&self) -> Value {
            json!({
                "id": self.id,
                "feed_id": self.feed.unwrap_or(1),
                "title": format!("Probe {}", self.id),
                "url": self.link,
                "html": format!("<p>actual production content {}</p>", self.id),
                "is_read": i32::from(self.read),
                "is_saved": i32::from(self.star),
                "created_on_time": self.published
            })
        }
    }

    struct Model {
        entries: Vec<Entry>,
        unread_401_remaining: usize,
        subscription_401_remaining: usize,
        login_401_remaining: usize,
        fever_version: i64,
        login_count: usize,
        requests: Vec<Value>,
        mark_ids: Vec<i64>,
        title: String,
        subscription_status: u16,
        gr_page_size: usize,
        ignore_state_writes: bool,
        subscription_url: String,
        second_subscription: bool,
        omit_subscription_field: bool,
    }

    impl Default for Model {
        fn default() -> Self {
            Self {
                entries: vec![],
                unread_401_remaining: 0,
                subscription_401_remaining: 0,
                login_401_remaining: 0,
                fever_version: 4,
                login_count: 0,
                requests: vec![],
                mark_ids: vec![],
                title: "Probe feed".into(),
                subscription_status: 200,
                gr_page_size: 10_000,
                ignore_state_writes: false,
                subscription_url: FEED_URL.into(),
                second_subscription: false,
                omit_subscription_field: false,
            }
        }
    }

    struct Shared {
        model: StdMutex<Model>,
        // 1 = first Fever mark; 2 = first Fever with_ids response.
        gate_kind: AtomicUsize,
        gate_used: AtomicBool,
        entered: Notify,
        release: Semaphore,
    }

    struct Server {
        base: String,
        shared: Arc<Shared>,
        accept: tokio::task::JoinHandle<()>,
    }

    impl Server {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let shared = Arc::new(Shared {
                model: StdMutex::new(Model::default()),
                gate_kind: AtomicUsize::new(0),
                gate_used: AtomicBool::new(false),
                entered: Notify::new(),
                release: Semaphore::new(0),
            });
            let shared_loop = shared.clone();
            let accept = tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let shared_conn = shared_loop.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(10),
                            serve(socket, shared_conn),
                        )
                        .await;
                    });
                }
            });
            Self {
                base,
                shared,
                accept,
            }
        }

        fn set_entries(&self, entries: Vec<Entry>) {
            self.shared.model.lock().unwrap().entries = entries;
        }

        fn arm(&self, kind: usize) {
            self.shared.gate_kind.store(kind, Ordering::SeqCst);
        }

        async fn wait_parked(&self) {
            tokio::time::timeout(Duration::from_secs(5), self.shared.entered.notified())
                .await
                .expect("probe server barrier was not reached");
        }

        fn release(&self) {
            self.shared.release.add_permits(1);
        }

        fn evidence(&self) -> Value {
            let model = self.shared.model.lock().unwrap();
            json!({
                "logins": model.login_count,
                "marks": model.mark_ids,
                "remote_title": model.title,
                "requests": model.requests
            })
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.accept.abort();
        }
    }

    async fn serve(mut socket: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
        let mut bytes = Vec::new();
        let end = loop {
            if let Some(pos) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
                break pos + 4;
            }
            let mut block = [0u8; 4096];
            let n = socket.read(&mut block).await?;
            if n == 0 || bytes.len() > 1024 * 1024 {
                return Ok(());
            }
            bytes.extend_from_slice(&block[..n]);
        };
        let header = String::from_utf8_lossy(&bytes[..end]).to_string();
        let content_len = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < end + content_len {
            let mut block = [0u8; 4096];
            let n = socket.read(&mut block).await?;
            if n == 0 {
                return Ok(());
            }
            bytes.extend_from_slice(&block[..n]);
        }
        let first = header.lines().next().unwrap_or("");
        let target = first.split_whitespace().nth(1).unwrap_or("/");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let q: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let form: Vec<(String, String)> =
            url::form_urlencoded::parse(&bytes[end..end + content_len])
                .into_owned()
                .collect();
        let gate_match = match shared.gate_kind.load(Ordering::SeqCst) {
            1 => q.get("mark").is_some_and(|v| v == "item"),
            2 => q.contains_key("with_ids"),
            _ => false,
        };
        let (status, body) = {
            let mut model = shared.model.lock().unwrap();
            // Do not record auth headers, passwords or api_key form fields.
            let operation_form: Vec<_> = form
                .iter()
                .filter(|(key, _)| matches!(key.as_str(), "i" | "a" | "r" | "ac" | "t"))
                .cloned()
                .collect();
            model
                .requests
                .push(json!({"path":path, "query":q, "operation_form":operation_form}));
            route(&mut model, path, &q, &form)
        };
        if gate_match
            && shared
                .gate_used
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            shared.entered.notify_one();
            shared.release.acquire().await.unwrap().forget();
        }
        let response = format!(
            "HTTP/1.1 {status} Probe\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await?;
        socket.shutdown().await
    }

    fn route(
        model: &mut Model,
        path: &str,
        q: &HashMap<String, String>,
        form: &[(String, String)],
    ) -> (u16, String) {
        let get = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if path.ends_with("/accounts/ClientLogin") {
            model.login_count += 1;
            if model.login_401_remaining > 0 {
                model.login_401_remaining -= 1;
                return (401, "auth failed".into());
            }
            return (
                200,
                format!("SID=dummy\nAuth=loopback-session-{}\n", model.login_count),
            );
        }
        if path.ends_with("/token") {
            return (200, "loopback-action-token".into());
        }
        if path.ends_with("/tag/list") {
            return (200, json!({"tags":[]}).to_string());
        }
        if path.ends_with("/subscription/list") {
            if model.subscription_401_remaining > 0 {
                model.subscription_401_remaining -= 1;
                return (401, "auth failed".into());
            }
            if model.omit_subscription_field {
                return (200, "{}".into());
            }
            if model.second_subscription {
                return (200, json!({"subscriptions":[
                    {"id":"feed/1", "title":"Secure feed", "url":"https://probe.invalid/rss", "categories":[]},
                    {"id":"feed/2", "title":"Plain feed", "url":"http://probe.invalid/rss", "categories":[]}
                ]}).to_string());
            }
            return (
                200,
                json!({"subscriptions":[{
                    "id":"feed/1", "title":model.title, "url":model.subscription_url,
                    "htmlUrl":"https://probe.invalid/", "categories":[]
                }]})
                .to_string(),
            );
        }
        if path.ends_with("/subscription/edit") {
            if model.subscription_status == 200 {
                if let Some(title) = get("t") {
                    model.title = title.to_string();
                }
            }
            return (model.subscription_status, "OK".into());
        }
        if path.ends_with("/stream/items/ids") {
            let stream = q.get("s").map(String::as_str).unwrap_or("");
            if q.contains_key("xt") && model.unread_401_remaining > 0 {
                model.unread_401_remaining -= 1;
                return (401, "{}".into());
            }
            let ot = q.get("ot").and_then(|v| v.parse::<i64>().ok());
            let all_refs: Vec<Value> = model
                .entries
                .iter()
                .filter(|entry| !q.contains_key("xt") || !entry.read)
                .filter(|entry| !stream.ends_with("/starred") || entry.star)
                .filter(|entry| ot.is_none_or(|ot| entry.published > ot))
                .map(|entry| json!({"id":entry.id.to_string()}))
                .collect();
            let start = q
                .get("c")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let end = (start + model.gr_page_size).min(all_refs.len());
            let refs = all_refs.get(start..end).unwrap_or_default();
            let mut response = json!({"itemRefs":refs});
            if end < all_refs.len() {
                response["continuation"] = json!(end.to_string());
            }
            return (200, response.to_string());
        }
        if path.ends_with("/stream/items/contents") {
            let ids: Vec<i64> = form
                .iter()
                .filter(|(key, _)| key == "i")
                .filter_map(|(_, value)| value.parse().ok())
                .collect();
            return (200, json!({"items":model.entries.iter().filter(|entry| ids.contains(&entry.id)).map(Entry::gr).collect::<Vec<_>>()} ).to_string());
        }
        if path.ends_with("/edit-tag") {
            let ids: Vec<i64> = form
                .iter()
                .filter(|(key, _)| key == "i")
                .filter_map(|(_, value)| value.parse().ok())
                .collect();
            let apply = !model.ignore_state_writes;
            for entry in model
                .entries
                .iter_mut()
                .filter(|entry| ids.contains(&entry.id) && apply)
            {
                if get("a").is_some_and(|v| v.ends_with("/read")) {
                    entry.read = true;
                }
                if get("r").is_some_and(|v| v.ends_with("/read")) {
                    entry.read = false;
                }
                if get("a").is_some_and(|v| v.ends_with("/starred")) {
                    entry.star = true;
                }
                if get("r").is_some_and(|v| v.ends_with("/starred")) {
                    entry.star = false;
                }
            }
            return (200, "OK".into());
        }
        if q.contains_key("api") {
            let mut response = json!({"auth":1,"api_version":model.fever_version});
            if q.contains_key("mark") {
                let id = q.get("id").and_then(|v| v.parse::<i64>().ok()).unwrap();
                model.mark_ids.push(id);
                let apply = !model.ignore_state_writes;
                for entry in model
                    .entries
                    .iter_mut()
                    .filter(|entry| entry.id == id && apply)
                {
                    match q.get("as").map(String::as_str) {
                        Some("read") => entry.read = true,
                        Some("unread") => entry.read = false,
                        Some("saved") => entry.star = true,
                        Some("unsaved") => entry.star = false,
                        _ => {}
                    }
                }
            } else if q.contains_key("unread_item_ids") {
                response["unread_item_ids"] = json!(model
                    .entries
                    .iter()
                    .filter(|entry| !entry.read)
                    .map(|entry| entry.id.to_string())
                    .collect::<Vec<_>>()
                    .join(","));
            } else if q.contains_key("saved_item_ids") {
                response["saved_item_ids"] = json!(model
                    .entries
                    .iter()
                    .filter(|entry| entry.star)
                    .map(|entry| entry.id.to_string())
                    .collect::<Vec<_>>()
                    .join(","));
            } else if q.contains_key("items") {
                let selected: Vec<&Entry> = if let Some(csv) = q.get("with_ids") {
                    let ids: Vec<i64> = csv.split(',').filter_map(|v| v.parse().ok()).collect();
                    model
                        .entries
                        .iter()
                        .filter(|entry| ids.contains(&entry.id))
                        .collect()
                } else if let Some(since) = q.get("since_id").and_then(|v| v.parse::<i64>().ok()) {
                    model
                        .entries
                        .iter()
                        .filter(|entry| entry.id > since)
                        .take(50)
                        .collect()
                } else if let Some(max) = q.get("max_id").and_then(|v| v.parse::<i64>().ok()) {
                    model
                        .entries
                        .iter()
                        .rev()
                        .filter(|entry| entry.id < max)
                        .take(50)
                        .collect()
                } else {
                    model.entries.iter().rev().take(50).collect()
                };
                response["items"] =
                    json!(selected.into_iter().map(Entry::fever).collect::<Vec<_>>());
            } else if q.contains_key("groups") || q.contains_key("feeds") {
                response["groups"] = json!([]);
                response["feeds_groups"] = json!([]);
                response["feeds"] =
                    json!([{"id":1,"title":"Probe feed","url":model.subscription_url}]);
            }
            return (200, response.to_string());
        }
        (404, "{}".into())
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn database(server: &Server, protocol: &str) -> Database {
        sync::init_session_cache();
        sync::clear_session_cache_for_tests();
        let conn = db::open(std::path::Path::new(":memory:")).unwrap();
        db::ensure_uncategorized_folder(&conn).unwrap();
        for (key, value) in [
            ("sync_protocol", protocol),
            ("greader_endpoint", server.base.as_str()),
            ("greader_username", "probe"),
            ("greader_password", "loopback-only-dummy"),
        ] {
            // Synthetic credentials only; production setter exercises current Windows DPAPI.
            // No real account or persistent user database is accessed.
            db::set_setting(&conn, key, value).unwrap();
        }
        conn.execute(
            "INSERT INTO feeds(id,feed_url,title,remote_id,origin,folder_id) VALUES(1,?1,'Probe feed',1,'remote',1)",
            [FEED_URL],
        ).unwrap();
        let key = sync::account_key(protocol, &server.base, "probe");
        db::mark_zero_binding_discovery_attempt(&conn, &key, now()).unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn seed_article(conn: &Connection, id: i64, read: bool, star: bool) {
        conn.execute(
            "INSERT INTO articles(id,feed_id,guid,title,url,remote_id,is_read,is_starred,source)
             VALUES(?1,1,?2,?3,?4,?1,?5,?6,'miniflux')",
            params![
                id,
                format!("remote-{id}"),
                format!("Probe {id}"),
                format!("https://probe.invalid/{id}"),
                i32::from(read),
                i32::from(star)
            ],
        )
        .unwrap();
    }

    fn entry(id: i64, read: bool, star: bool) -> Entry {
        Entry {
            id,
            feed: Some(1),
            link: Some(format!("https://probe.invalid/{id}")),
            read,
            star,
            published: 2_000_000_000,
        }
    }

    #[tokio::test]
    async fn reauth_preserves_sql_fault_and_marks_history_after_recovery() {
        let _serial = serialize().await;
        let (report, pending, flag, evidence) = sql_fault_reauth_case(1, false).await;
        assert_eq!(
            (pending, flag),
            (0, 0),
            "SQL trigger ran; retry confirmed the item"
        );
        assert_eq!(evidence["logins"], 2);
        assert!(!report.auth_failure, "authentication recovered");
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].contains("regression_sql_fault_once"));
        assert!(report.errors[0].contains("重认证前曾发生："));
        assert!(report.errors[0].contains("后续阶段重试已完成"));
        assert!(report.errors[0].contains("引号内为当时状态"));
        println!(
            "{}",
            json!({"case":"sql_fault_then_reauth", "report":report, "pending":pending,"flag":flag,"server":evidence})
        );
    }

    #[tokio::test]
    async fn sql_fault_without_reauth_remains_current_failure() {
        let _serial = serialize().await;
        let (report, pending, flag, evidence) = sql_fault_reauth_case(0, false).await;
        assert_eq!((pending, flag), (1, 0));
        assert_eq!(evidence["logins"], 1);
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert!(report.errors[0].contains("regression_sql_fault_once"));
        assert!(!report.errors[0].contains("重认证前曾发生："));
    }

    #[tokio::test]
    async fn failed_reauthentication_retains_sql_and_auth_failures() {
        let _serial = serialize().await;
        let (report, pending, flag, evidence) = sql_fault_reauth_case(1, true).await;
        assert_eq!((pending, flag), (1, 0));
        assert_eq!(evidence["logins"], 2);
        assert!(report.auth_failure);
        assert!(report
            .errors
            .iter()
            .any(|message| message.contains("regression_sql_fault_once")));
        assert!(!report
            .errors
            .iter()
            .any(|message| message.contains("后续阶段重试已完成")));
    }

    #[tokio::test]
    async fn unsuccessful_stage_retry_keeps_latest_auth_and_historical_sql() {
        let _serial = serialize().await;
        let (report, pending, flag, evidence) = sql_fault_reauth_case(2, false).await;
        assert_eq!((pending, flag), (0, 0));
        assert_eq!(evidence["logins"], 2);
        assert!(report.auth_failure);
        let historical = report
            .errors
            .iter()
            .find(|message| message.contains("regression_sql_fault_once"))
            .expect("SQL fact retained");
        assert!(historical.contains("后续已重试，仍有错误"));
        assert!(!historical.contains("后续阶段重试已完成"));
        assert_eq!(
            report.errors.len(),
            2,
            "one current auth error, one historical SQL error: {report:?}"
        );
    }

    #[tokio::test]
    async fn recovered_authentication_only_does_not_report_final_failure() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        let http = ingestion::build_client(5);
        assert!(sync::feeds_phase(&database, &http)
            .await
            .unwrap()
            .errors
            .is_empty());
        server
            .shared
            .model
            .lock()
            .unwrap()
            .subscription_401_remaining = 1;
        let report = sync::feeds_phase(&database, &http).await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(!report.auth_failure);
        assert_eq!(server.evidence()["logins"], 2);
    }

    async fn sql_fault_reauth_case(
        subscription_failures: usize,
        login_fails: bool,
    ) -> (sync::SyncReport, i64, i64, Value) {
        let server = Server::start().await;
        let database = database(&server, "greader");
        let http = ingestion::build_client(5);
        assert!(sync::feeds_phase(&database, &http)
            .await
            .unwrap()
            .errors
            .is_empty());
        {
            let conn = database.lock().await;
            let account = sync::current_account(&conn).unwrap().unwrap();
            db::enqueue_edit(
                &conn,
                account.generation,
                &account.key,
                FEED_URL,
                1,
                Some("Edited during SQL fault"),
                None,
                None,
            )
            .unwrap();
            conn.execute_batch("CREATE TABLE regression_once(flag INTEGER); INSERT INTO regression_once VALUES(1);
                CREATE TRIGGER regression_one_shot_delete BEFORE DELETE ON subscription_outbox WHEN EXISTS(SELECT 1 FROM regression_once)
                BEGIN DELETE FROM regression_once; SELECT RAISE(FAIL,'regression_sql_fault_once'); END;").unwrap();
        }
        {
            let mut model = server.shared.model.lock().unwrap();
            model.subscription_401_remaining = subscription_failures;
            model.login_401_remaining = usize::from(login_fails);
        }
        let report = sync::feeds_phase(&database, &http).await.unwrap();
        let conn = database.lock().await;
        let pending = conn
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        let flag = conn
            .query_row("SELECT COUNT(*) FROM regression_once", [], |r| r.get(0))
            .unwrap();
        (report, pending, flag, server.evidence())
    }

    #[tokio::test]
    async fn cached_reauth_preserves_committed_changes() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        let http = ingestion::build_client(5);
        let warm = sync::states_phase(&database, &http, false).await.unwrap();
        assert!(warm.errors.is_empty(), "{:?}", warm.errors);
        server.set_entries(vec![entry(101, true, false)]);
        server.shared.model.lock().unwrap().unread_401_remaining = 1;
        let report = sync::states_phase(&database, &http, false).await.unwrap();
        let count: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":"cached_reauth_committed_changes","expected_change":true,"actual_change":report.has_data_change(),"article_count":count,"report":report,"server":server.evidence()})
        );
        assert_eq!(
            count, 1,
            "control: the first attempt really committed a new article"
        );
        assert!(
            report.errors.is_empty(),
            "control: fresh authentication recovered"
        );
        assert!(report.has_data_change(), "a successful authentication retry must retain first-attempt committed data-change evidence");
    }

    #[tokio::test]
    async fn fever_read_batch_stops_after_real_disconnect() {
        let _serial = serialize().await;
        let server = Server::start().await;
        server.set_entries((101..=103).map(|id| entry(id, false, false)).collect());
        let database = database(&server, "fever");
        {
            let conn = database.lock().await;
            for id in 101..=103 {
                seed_article(&conn, id, true, false);
                // Earlier protocol content supplied feed/1. Reach the real
                // first mark, then disconnect before any subsequent ID.
                assert!(db::confirm_article_remote_origin(&conn, id, id, 1).unwrap());
                db::enqueue_sync(&conn, Some(id), None, "read", None).unwrap();
            }
        }
        server.arm(1);
        let db_task = database.clone();
        let push = tokio::spawn(async move {
            sync::push_states_now(&db_task, &ingestion::build_client(5)).await;
        });
        server.wait_parked().await;
        let disconnected = {
            let mut conn = database.lock().await;
            app_lib::commands::disconnect_for_test(&mut conn).unwrap()
        };
        server.release();
        push.await.unwrap();
        let evidence = server.evidence();
        println!(
            "{}",
            json!({"probe":"fever_read_after_disconnect","expected_total_marks":1,"actual_total_marks":evidence["marks"].as_array().unwrap().len(),"disconnect_removed":disconnected,"server":evidence})
        );
        assert_eq!(evidence["marks"].as_array().unwrap().len(), 1, "disconnect may not retract the first request, but must stop subsequent Fever item writes");
    }

    #[tokio::test]
    async fn fever_star_probe_does_not_write_after_real_disconnect() {
        let _serial = serialize().await;
        let server = Server::start().await;
        server.set_entries(vec![entry(101, false, false)]);
        server.shared.model.lock().unwrap().fever_version = 3;
        let database = database(&server, "fever");
        {
            let conn = database.lock().await;
            seed_article(&conn, 101, false, true);
            assert!(db::confirm_article_remote_origin(&conn, 101, 101, 1).unwrap());
            db::enqueue_sync(&conn, Some(101), None, "star", None).unwrap();
        }
        server.arm(2);
        let db_task = database.clone();
        let push = tokio::spawn(async move {
            sync::push_states_now(&db_task, &ingestion::build_client(5)).await;
        });
        server.wait_parked().await;
        {
            let mut conn = database.lock().await;
            app_lib::commands::disconnect_for_test(&mut conn).unwrap();
        }
        server.release();
        push.await.unwrap();
        let evidence = server.evidence();
        println!(
            "{}",
            json!({"probe":"fever_star_after_disconnect","expected_marks":0,"actual_marks":evidence["marks"].as_array().unwrap().len(),"server":evidence})
        );
        assert!(evidence["marks"].as_array().unwrap().is_empty(), "a read-only probe response arriving after disconnect must not start a new old-account mark");
    }

    async fn completion_write_case(protocol: &str) -> (sync::SyncReport, i64, Value) {
        let server = Server::start().await;
        let database = database(&server, protocol);
        {
            let conn = database.lock().await;
            db::set_last_sync_ts(&conn, 123).unwrap();
            db::set_last_sync_entry_id(&conn, 1).unwrap();
            db::set_fever_history_complete(&conn).unwrap();
            conn.execute_batch(
                "CREATE TRIGGER audit_deny_completion BEFORE INSERT ON settings
                 WHEN NEW.key = 'sync_last_sync'
                 BEGIN SELECT RAISE(FAIL,'audit_completion_write_denied'); END;",
            )
            .unwrap();
        }
        let report = sync::states_phase(&database, &ingestion::build_client(5), false)
            .await
            .unwrap();
        let ts = db::last_sync_ts(&*database.lock().await).unwrap();
        (report, ts, server.evidence())
    }

    #[tokio::test]
    async fn fever_completion_persistence_failure_is_reported() {
        let _serial = serialize().await;
        let (report, ts, evidence) = completion_write_case("fever").await;
        println!(
            "{}",
            json!({"probe":"fever_completion_failure","expected_error":true,"actual_error":!report.errors.is_empty(),"actual_timestamp":ts,"report":report,"server":evidence})
        );
        assert_eq!(ts, 123, "control: the injected DB write really failed");
        assert!(
            !report.errors.is_empty(),
            "Fever completion write failure must remain visible instead of returning clean success"
        );
    }

    #[tokio::test]
    async fn greader_completion_failure_control_is_visible() {
        let _serial = serialize().await;
        let (report, ts, evidence) = completion_write_case("greader").await;
        println!(
            "{}",
            json!({"probe":"greader_completion_failure_control","actual_error":!report.errors.is_empty(),"actual_timestamp":ts,"report":report,"server":evidence})
        );
        assert_eq!(ts, 123);
        assert!(report
            .errors
            .iter()
            .any(|e| e.contains("audit_completion_write_denied")));
    }

    #[tokio::test]
    async fn unknown_greader_entry_missing_origin_does_not_consume_window() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let mut remote = entry(101, true, false);
        remote.feed = None;
        remote.published = 1_700_000_000;
        server.set_entries(vec![remote.clone()]);
        let database = database(&server, "greader");
        let http = ingestion::build_client(5);
        {
            db::set_last_sync_ts(&*database.lock().await, 123).unwrap();
        }
        let first = sync::states_phase(&database, &http, true).await.unwrap();
        let after_bad_cursor = db::last_sync_ts(&*database.lock().await).unwrap();
        remote.feed = Some(1);
        server.set_entries(vec![remote]);
        let light = sync::states_phase(&database, &http, false).await.unwrap();
        let after_light: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        let repaired_full = sync::states_phase(&database, &http, true).await.unwrap();
        let after_full: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":"greader_missing_origin_window","expected_first_error":true,"first_report":first,"cursor_after_bad":after_bad_cursor,"light_report":light,"articles_after_repaired_light":after_light,"articles_after_repaired_full":after_full,"full_report":repaired_full,"server":server.evidence()})
        );
        assert_eq!(
            after_full, 1,
            "control: fixing only origin makes the same article importable"
        );
        assert!(!first.errors.is_empty(), "unbound new contents without feed identity must not silently consume the successful listing window");
        assert_eq!(after_bad_cursor, 123);
    }

    #[tokio::test]
    async fn outbox_prune_failure_is_visible_in_report() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        {
            let conn = database.lock().await;
            let account = sync::current_account(&conn).unwrap().unwrap();
            db::enqueue_edit(
                &conn,
                account.generation,
                &account.key,
                FEED_URL,
                1,
                Some("Changed title"),
                None,
                None,
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TRIGGER audit_deny_outbox_delete BEFORE DELETE ON subscription_outbox
                 BEGIN SELECT RAISE(FAIL,'audit_outbox_confirmation_denied'); END;",
            )
            .unwrap();
        }
        let report = sync::push_subscription_changes(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        let pending: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":"outbox_confirmation_failure","expected_error":true,"actual_error":!report.errors.is_empty(),"pending_count":pending,"report":report,"server":server.evidence()})
        );
        assert_eq!(pending, 1, "control: the outbox confirmation really failed");
        assert!(!report.errors.is_empty(), "database failure confirming an acknowledged write must be observable in the returned sync report");
    }

    #[tokio::test]
    async fn outbox_ack_control_updates_remote_and_clears_queue() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        {
            let conn = database.lock().await;
            let account = sync::current_account(&conn).unwrap().unwrap();
            db::enqueue_edit(
                &conn,
                account.generation,
                &account.key,
                FEED_URL,
                1,
                Some("Changed title"),
                None,
                None,
            )
            .unwrap();
        }
        let report = sync::push_subscription_changes(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        let pending: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM subscription_outbox", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":"outbox_success_control","pending_count":pending,"report":report,"server":server.evidence()})
        );
        assert_eq!(pending, 0);
        assert_eq!(server.evidence()["remote_title"], "Changed title");
        assert!(report.errors.is_empty());
    }

    async fn empty_links_case(protocol: &str) {
        let server = Server::start().await;
        let mut entries: Vec<Entry> = (101..=104).map(|id| entry(id, true, false)).collect();
        entries[0].link = None;
        entries[1].link = Some(String::new());
        entries[2].link = Some("   ".into());
        server.set_entries(entries);
        let database = database(&server, protocol);
        let report = sync::states_phase(&database, &ingestion::build_client(5), true)
            .await
            .unwrap();
        let (count, ids, since, history) = {
            let conn = database.lock().await;
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
                .unwrap();
            let ids: Vec<i64> = conn
                .prepare("SELECT remote_id FROM articles ORDER BY remote_id")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (
                count,
                ids,
                db::last_sync_entry_id(&conn).unwrap(),
                format!("{:?}", db::fever_history_state(&conn).unwrap()),
            )
        };
        println!(
            "{}",
            json!({"probe":format!("{protocol}_empty_links"),"expected_rows":4,"actual_rows":count,"imported_remote_ids":ids,"actual_since":since,"history":history,"report":report,"server":server.evidence()})
        );
        assert_eq!(count, 4, "remote entry ID and bound feed are sufficient identity; missing/empty/whitespace original links must not drop distinct entries");
    }

    #[tokio::test]
    async fn greader_empty_links_preserve_distinct_remote_identities() {
        let _serial = serialize().await;
        empty_links_case("greader").await;
    }

    #[tokio::test]
    async fn fever_empty_links_preserve_distinct_remote_identities() {
        let _serial = serialize().await;
        empty_links_case("fever").await;
    }

    #[tokio::test]
    async fn pagination_controls_import_125_and_replay_without_changes() {
        let _serial = serialize().await;
        for protocol in ["greader", "fever"] {
            let server = Server::start().await;
            server.shared.model.lock().unwrap().gr_page_size = 40;
            server.set_entries((1..=125).map(|id| entry(id, true, false)).collect());
            let database = database(&server, protocol);
            let http = ingestion::build_client(5);
            let first = sync::states_phase(&database, &http, true).await.unwrap();
            let count: i64 = database
                .lock()
                .await
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
                .unwrap();
            let replay = sync::states_phase(&database, &http, true).await.unwrap();
            let after: i64 = database
                .lock()
                .await
                .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
                .unwrap();
            println!(
                "{}",
                json!({"probe":format!("{protocol}_pagination_control"),"expected_rows":125,"actual_first_rows":count,"actual_replay_rows":after,"first_change":first.has_data_change(),"replay_change":replay.has_data_change(),"first_report":first,"replay_report":replay,"server":server.evidence()})
            );
            assert_eq!(count, 125);
            assert_eq!(after, 125);
            assert!(first.has_data_change());
            assert!(
                !replay.has_data_change(),
                "same-data full replay should not manufacture changes"
            );
            assert!(first.errors.is_empty(), "{:?}", first.errors);
            assert!(replay.errors.is_empty(), "{:?}", replay.errors);
        }
    }

    #[tokio::test]
    async fn state_roundtrip_controls_cover_read_unread_star_unstar() {
        let _serial = serialize().await;
        for (protocol, version) in [("greader", 4), ("fever", 4), ("fever", 3)] {
            let server = Server::start().await;
            server.shared.model.lock().unwrap().fever_version = version;
            server.set_entries(vec![entry(101, false, false)]);
            let database = database(&server, protocol);
            seed_article(&*database.lock().await, 101, false, false);
            assert!(
                db::confirm_article_remote_origin(&*database.lock().await, 101, 101, 1).unwrap()
            );
            let http = ingestion::build_client(5);
            let mut steps = vec![];
            for (field, desired) in [
                ("read", true),
                ("read", false),
                ("star", true),
                ("star", false),
            ] {
                {
                    let conn = database.lock().await;
                    if field == "read" {
                        db::set_read_with_enqueue(&conn, 101, desired).unwrap();
                    } else {
                        db::set_starred_with_enqueue(&conn, 101, desired).unwrap();
                    }
                }
                sync::push_states_now(&database, &http).await;
                let pending: i64 = database
                    .lock()
                    .await
                    .query_row("SELECT COUNT(*) FROM sync_queue", [], |r| r.get(0))
                    .unwrap();
                let remote_value = {
                    let model = server.shared.model.lock().unwrap();
                    if field == "read" {
                        model.entries[0].read
                    } else {
                        model.entries[0].star
                    }
                };
                steps.push(json!({"field":field,"desired":desired,"remote":remote_value,"queue_count":pending}));
                assert_eq!(
                    remote_value, desired,
                    "{protocol} api_version={version} {field}={desired}"
                );
                assert_eq!(
                    pending, 0,
                    "{protocol} api_version={version} {field}={desired}"
                );
            }
            println!(
                "{}",
                json!({"probe":format!("{protocol}_v{version}_state_roundtrip_control"),"steps":steps,"server":server.evidence()})
            );
        }
    }

    #[tokio::test]
    async fn acknowledged_noop_state_writes_keep_retryable_intent() {
        let _serial = serialize().await;
        for protocol in ["greader", "fever"] {
            let server = Server::start().await;
            server.shared.model.lock().unwrap().ignore_state_writes = true;
            server.set_entries(vec![entry(101, false, false)]);
            let database = database(&server, protocol);
            {
                let conn = database.lock().await;
                seed_article(&conn, 101, false, false);
                // This no-op ACK control needs a previously proven identity
                // so the injected write/readback path is actually exercised.
                assert!(db::confirm_article_remote_origin(&conn, 101, 101, 1).unwrap());
                db::set_read_with_enqueue(&conn, 101, true).unwrap();
            }
            sync::push_states_now(&database, &ingestion::build_client(5)).await;
            let queue: (i64, String, Option<String>) = database
                .lock()
                .await
                .query_row(
                    "SELECT COUNT(*),status,last_error_class FROM sync_queue",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            println!(
                "{}",
                json!({"probe":format!("{protocol}_noop_ack_control"),"queue":queue,"server":server.evidence()})
            );
            assert_eq!(queue.0, 1);
            assert_eq!(queue.1, "retry_wait");
            assert_eq!(queue.2.as_deref(), Some("verification"));
        }
    }

    async fn url_change_case(protocol: &str, change_url: bool) {
        let server = Server::start().await;
        server.set_entries(vec![entry(101, true, true)]);
        let database = database(&server, protocol);
        let http = ingestion::build_client(5);
        let initial = sync::states_phase(&database, &http, true).await.unwrap();
        assert!(initial.errors.is_empty(), "{:?}", initial.errors);
        let old_feed_id: i64 = database
            .lock()
            .await
            .query_row(
                "SELECT feed_id FROM articles WHERE remote_id=101",
                [],
                |r| r.get(0),
            )
            .unwrap();
        database
            .lock()
            .await
            .execute(
                "UPDATE feeds SET layout='podcast',site_url='https://probe.invalid/' WHERE id=?1",
                [old_feed_id],
            )
            .unwrap();
        let before: (i64, i64) = database
            .lock()
            .await
            .query_row("SELECT COUNT(*),SUM(is_starred) FROM articles", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        if change_url {
            server.shared.model.lock().unwrap().subscription_url =
                "https://probe.invalid/moved-rss".into();
        }
        let feed_report = sync::feeds_phase(&database, &http).await.unwrap();
        assert!(feed_report.errors.is_empty(), "{:?}", feed_report.errors);
        let after_articles: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        let feeds: Vec<(i64, String, String, i64)> = {
            let conn = database.lock().await;
            let mut stmt = conn
                .prepare("SELECT id,feed_url,layout,remote_id FROM feeds ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let states_report = sync::states_phase(&database, &http, true).await.unwrap();
        let restored_articles: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":format!("new_{protocol}_url_change_{change_url}"),"old_feed_id":old_feed_id,"before_article_count":before.0,"before_starred":before.1,"after_feeds_articles":after_articles,"feeds":feeds,"feed_report":feed_report,"after_full_articles":restored_articles,"states_report":states_report,"server":server.evidence()})
        );
        assert_eq!(
            after_articles, before.0,
            "same remote feed identity must preserve existing articles when its URL changes"
        );
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].0, old_feed_id);
        assert_eq!(feeds[0].2, "podcast");
        assert_eq!(
            feeds[0].1,
            if change_url {
                "https://probe.invalid/moved-rss"
            } else {
                FEED_URL
            }
        );
        assert_eq!(feed_report.removed_feeds, 0);
        assert_eq!(
            feed_report.pulled_feeds, 0,
            "URL update preserves an existing entity"
        );
        assert_eq!(feed_report.has_data_change(), change_url);
    }

    #[tokio::test]
    async fn new_greader_feed_url_change_preserves_identity() {
        let _serial = serialize().await;
        url_change_case("greader", true).await;
    }
    #[tokio::test]
    async fn new_fever_feed_url_change_preserves_identity() {
        let _serial = serialize().await;
        url_change_case("fever", true).await;
    }
    #[tokio::test]
    async fn new_unchanged_feed_url_preserves_identity_control() {
        let _serial = serialize().await;
        for protocol in ["greader", "fever"] {
            url_change_case(protocol, false).await;
        }
    }
    #[tokio::test]
    async fn new_greader_distinct_remote_feeds_do_not_overwrite_bindings() {
        let _serial = serialize().await;
        let server = Server::start().await;
        server.shared.model.lock().unwrap().second_subscription = true;
        let mut second = entry(102, true, false);
        second.feed = Some(2);
        server.set_entries(vec![entry(101, true, false), second]);
        let database = database(&server, "greader");
        let http = ingestion::build_client(5);
        let feeds_report = sync::feeds_phase(&database, &http).await.unwrap();
        let states_report = sync::states_phase(&database, &http, true).await.unwrap();
        let feeds: Vec<(i64, String, i64)> = {
            let conn = database.lock().await;
            let mut stmt = conn
                .prepare("SELECT id,feed_url,remote_id FROM feeds ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let articles: Vec<(i64, i64)> = {
            let conn = database.lock().await;
            let mut stmt = conn
                .prepare("SELECT remote_id,feed_id FROM articles ORDER BY remote_id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        println!(
            "{}",
            json!({"probe":"new_distinct_remote_feeds_normalized_collision","feeds":feeds,"articles":articles,"feeds_report":feeds_report,"states_report":states_report,"server":server.evidence()})
        );
        assert_eq!(
            feeds.len(),
            2,
            "known remote identities cannot be overwritten merely because URLs normalize together"
        );
        assert_eq!(articles.len(), 2);
    }
    #[tokio::test]
    async fn new_missing_subscription_collection_is_fail_closed_control() {
        let _serial = serialize().await;
        let server = Server::start().await;
        server.shared.model.lock().unwrap().omit_subscription_field = true;
        let database = database(&server, "greader");
        seed_article(&*database.lock().await, 101, true, true);
        let report = sync::feeds_phase(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        let rows: i64 = database
            .lock()
            .await
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        println!(
            "{}",
            json!({"probe":"new_missing_subscriptions_fail_closed","articles":rows,"report":report,"server":server.evidence()})
        );
        assert_eq!(rows, 1);
        assert!(!report.errors.is_empty());
    }

    #[tokio::test]
    async fn placeholder_feed_title_backfill_reports_change_once() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        {
            let conn = database.lock().await;
            conn.execute("UPDATE feeds SET title=feed_url,title_custom='',site_url='https://probe.invalid/' WHERE id=1", []).unwrap();
        }
        let first = sync::feeds_phase(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        let replay = sync::feeds_phase(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        assert!(first.errors.is_empty(), "{first:?}");
        assert!(replay.errors.is_empty(), "{replay:?}");
        assert!(first.has_data_change());
        assert!(!replay.has_data_change());
        assert_eq!(
            database
                .lock()
                .await
                .query_row("SELECT title FROM feeds WHERE id=1", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "Probe feed"
        );
    }

    #[tokio::test]
    async fn feed_url_unique_conflict_keeps_entities_and_cursor() {
        let _serial = serialize().await;
        for protocol in ["greader", "fever"] {
            let server = Server::start().await;
            let database = database(&server, protocol);
            seed_article(&*database.lock().await, 101, true, true);
            let collision = "https://probe.invalid/conflicting-feed";
            {
                let conn = database.lock().await;
                conn.execute("UPDATE feeds SET layout='podcast' WHERE id=1", [])
                    .unwrap();
                conn.execute("INSERT INTO feeds(feed_url,title,origin,folder_id) VALUES(?1,'local','local',1)", [collision]).unwrap();
                db::set_last_sync_ts(&conn, 777).unwrap();
            }
            server.shared.model.lock().unwrap().subscription_url = collision.into();
            let report = sync::feeds_phase(&database, &ingestion::build_client(5))
                .await
                .unwrap();
            assert!(
                !report.errors.is_empty(),
                "unique URL collision must be visible: {report:?}"
            );
            assert_eq!(report.removed_feeds, 0);
            let states = sync::states_phase(&database, &ingestion::build_client(5), false)
                .await
                .unwrap();
            assert!(
                !states.errors.is_empty(),
                "incomplete binding must prevent cursor advance"
            );
            let conn = database.lock().await;
            let row: (String, String, i64) = conn
                .query_row(
                    "SELECT feed_url,layout,remote_id FROM feeds WHERE id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(row, (FEED_URL.into(), "podcast".into(), 1));
            assert_eq!(db::last_sync_ts(&conn).unwrap(), 777);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
    }

    #[tokio::test]
    async fn feed_url_fallback_only_binds_unbound_rows() {
        let _serial = serialize().await;
        let server = Server::start().await;
        let database = database(&server, "greader");
        {
            let conn = database.lock().await;
            conn.execute("UPDATE feeds SET remote_id=NULL,origin='local',layout='podcast',feed_url='http://probe.invalid/rss' WHERE id=1", []).unwrap();
        }
        let report = sync::feeds_phase(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        let conn = database.lock().await;
        let row: (i64, String, String, i64) = conn
            .query_row("SELECT id,feed_url,layout,remote_id FROM feeds", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!(
            row,
            (1, "http://probe.invalid/rss".into(), "podcast".into(), 1)
        );
    }

    #[tokio::test]
    async fn unsubscribe_http400_keeps_existing_accepted_policy() {
        let _serial = serialize().await;
        let server = Server::start().await;
        server.shared.model.lock().unwrap().subscription_status = 400;
        let database = database(&server, "greader");
        {
            let conn = database.lock().await;
            let account = sync::current_account(&conn).unwrap().unwrap();
            db::enqueue_unsubscribe(
                &conn,
                account.generation,
                &account.key,
                FEED_URL,
                Some(1),
                None,
            )
            .unwrap();
        }
        let report = sync::push_subscription_changes(&database, &ingestion::build_client(5))
            .await
            .unwrap();
        let pending: i64 = database
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM subscription_outbox WHERE status='pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let client = app_lib::greader::GReaderClient::new(
            &server.base,
            "independent-readback",
            ingestion::build_client(5),
        );
        let subscriptions = client.subscriptions().await.unwrap();
        let target_still_present = subscriptions.iter().any(|s| s.id == "feed/1");
        println!(
            "{}",
            json!({"probe":"new_unsubscribe_400_target_present","pending":pending,"target_still_present":target_still_present,"report":report,"server":server.evidence()})
        );
        assert!(
            target_still_present,
            "control: the failed removal target remains on server"
        );
        assert_eq!(pending,0,"accepted policy: HTTP400/404 tolerates a failed removal; local tombstone still prevents resurrection");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
}

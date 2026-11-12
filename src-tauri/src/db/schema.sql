-- Current FluxReader data format; legacy databases are rejected before any write.
PRAGMA application_id = 1179796018;

CREATE TABLE folders (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            name          TEXT NOT NULL,
            position      INTEGER NOT NULL DEFAULT 0,
            layout        TEXT NOT NULL DEFAULT 'article',
            auto_summary  INTEGER NOT NULL DEFAULT 0,
            auto_translate INTEGER NOT NULL DEFAULT 0,
            collapsed     INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        , remote_id INTEGER, sync_id TEXT,
            origin TEXT NOT NULL DEFAULT 'local' CHECK(origin IN ('local','remote')),
            summary_mode TEXT NOT NULL DEFAULT 'inherit' CHECK(summary_mode IN ('inherit','on','off')),
            translate_mode TEXT NOT NULL DEFAULT 'inherit' CHECK(translate_mode IN ('inherit','on','off')));

CREATE TABLE feeds (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            feed_url        TEXT NOT NULL UNIQUE,
            site_url        TEXT,
            title           TEXT NOT NULL,
            favicon_url     TEXT,
            folder_id       INTEGER REFERENCES folders(id) ON DELETE CASCADE,
            layout          TEXT NOT NULL DEFAULT 'inherit',
            auto_summary    INTEGER NOT NULL DEFAULT 0,
            auto_translate  INTEGER NOT NULL DEFAULT 0,
            etag            TEXT,
            last_modified   TEXT,
            last_fetched_at TEXT,
            fetch_error     TEXT,
            fetch_failed    INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        , remote_id INTEGER, fail_count INTEGER NOT NULL DEFAULT 0, next_retry_at TEXT, origin TEXT NOT NULL DEFAULT 'local', origin_was_local INTEGER NOT NULL DEFAULT 0, summary_mode TEXT NOT NULL DEFAULT 'inherit'
            CHECK (summary_mode IN ('inherit', 'on', 'off')), translate_mode TEXT NOT NULL DEFAULT 'inherit'
            CHECK (translate_mode IN ('inherit', 'on', 'off')), fetched_title TEXT NOT NULL DEFAULT '', title_custom TEXT, fetch_generation INTEGER NOT NULL DEFAULT 0,
            active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
            sync_id TEXT NOT NULL DEFAULT (lower(hex(randomblob(16)))),
            lifecycle_version INTEGER NOT NULL DEFAULT 0);

CREATE TABLE source_tombstones (
    source_id TEXT PRIMARY KEY,
    lifecycle_version INTEGER NOT NULL,
    feed_url TEXT NOT NULL,
    deleted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE articles (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            feed_id       INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
            guid          TEXT NOT NULL,
            url           TEXT,
            title         TEXT NOT NULL,
            author        TEXT,
            summary       TEXT,
            image_url     TEXT,
            image_source  TEXT NOT NULL DEFAULT 'page' CHECK(image_source IN ('rss','page')),
            enclosure_url TEXT,
            enclosure_mime TEXT,
            duration_sec  INTEGER,
            ai_summary    TEXT,
            translated_content TEXT,
            source        TEXT NOT NULL DEFAULT 'direct',
            published_at  TEXT,
            fetched_at    TEXT NOT NULL DEFAULT (datetime('now')),
            is_read       INTEGER NOT NULL DEFAULT 0,
            body_retention_since INTEGER CHECK(body_retention_since IS NULL OR (typeof(body_retention_since)='integer' AND body_retention_since>=0)),
            is_starred    INTEGER NOT NULL DEFAULT 0, remote_id INTEGER, remote_origin_feed_id INTEGER, url_norm TEXT, read_version INTEGER NOT NULL DEFAULT 0, star_version INTEGER NOT NULL DEFAULT 0, content_id INTEGER REFERENCES contents(id), ai_epoch INTEGER NOT NULL DEFAULT 0, ai_summary_sig TEXT, translated_sig TEXT, first_seen_at TEXT, published_at_source TEXT NOT NULL DEFAULT 'legacy_unknown'
            CHECK (published_at_source IN ('feed', 'feed_clamped', 'inferred_fetch', 'legacy_unknown')),
            rss_content_id INTEGER REFERENCES contents(id),
            images_json TEXT NOT NULL DEFAULT '[]',
            image_count INTEGER NOT NULL DEFAULT 0,
            media_revision INTEGER NOT NULL DEFAULT 0,
            local_sync_source TEXT,
            UNIQUE(feed_id, guid)
        );

CREATE TABLE settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

CREATE TABLE deduped_urls (
            url     TEXT PRIMARY KEY,
            kept_aid INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
            kept_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

CREATE TABLE "sync_queue" (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            article_id  INTEGER REFERENCES articles(id) ON DELETE CASCADE,
            feed_url    TEXT,
            action      TEXT NOT NULL,
            payload     TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            attempts    INTEGER NOT NULL DEFAULT 0,
            last_error  TEXT
        , kind TEXT NOT NULL DEFAULT 'article_state', field TEXT, desired_state INTEGER, status TEXT NOT NULL DEFAULT 'bound_pending'
            CHECK (status IN ('local_only', 'bindable_retry', 'bound_pending',
                              'retry_wait', 'failed', 'expired_unconfirmed')), updated_at TEXT, unbound_reason TEXT, last_error_class TEXT);

CREATE TABLE contents (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            identity           TEXT NOT NULL UNIQUE,
            url_norm           TEXT,
            content_html       TEXT,
            body_text          TEXT NOT NULL DEFAULT '',
            extraction_source  TEXT NOT NULL DEFAULT 'rss',
            fulltext_extracted INTEGER NOT NULL DEFAULT 0,
            revision           INTEGER NOT NULL DEFAULT 1,
            created_at         TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
            images_json TEXT NOT NULL DEFAULT '[]', image_count INTEGER NOT NULL DEFAULT 0
        );

CREATE TABLE subscription_outbox (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            account_generation INTEGER NOT NULL,
            account_key        TEXT NOT NULL,
            action             TEXT NOT NULL CHECK (action IN ('subscribe','edit','unsubscribe')),
            feed_url           TEXT NOT NULL,
            remote_id          INTEGER,
            desired_title      TEXT,
            desired_folder     TEXT,
            prev_folder        TEXT,
            stage              TEXT NOT NULL DEFAULT 'pending' CHECK (stage IN ('pending','remote_created')),
            status             TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','local_only')),
            attempts           INTEGER NOT NULL DEFAULT 0,
            last_error         TEXT,
            created_at         TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
        , resolved_remote_id INTEGER);

CREATE TABLE ai_segments (
            article_id INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
            product TEXT NOT NULL,
            signature TEXT NOT NULL,
            segment_index INTEGER NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (article_id, product, signature, segment_index)
        );

CREATE VIRTUAL TABLE articles_trigram_idx USING fts5(
            doc,
            tokenize='trigram',
            detail=none,
            content='',
            contentless_delete=1
        );

CREATE INDEX idx_articles_feed      ON articles(feed_id);
CREATE INDEX idx_articles_sync_source_guid ON articles(local_sync_source,guid);

CREATE INDEX idx_articles_published ON articles(published_at DESC);

CREATE INDEX idx_articles_unread    ON articles(is_read) WHERE is_read = 0;

CREATE UNIQUE INDEX idx_articles_miniflux_id ON articles(remote_id) WHERE remote_id IS NOT NULL;

-- An origin is evidence for this exact remote identity and source incarnation.
-- Account purge clears remote_id; source rebind/retirement invalidates its evidence.
CREATE TRIGGER articles_remote_origin_invalidate
AFTER UPDATE OF remote_id, feed_id ON articles
WHEN OLD.remote_id IS NOT NEW.remote_id OR OLD.feed_id IS NOT NEW.feed_id
BEGIN
    UPDATE articles SET remote_origin_feed_id = NULL WHERE id = NEW.id;
END;

CREATE TRIGGER feeds_remote_origin_invalidate
AFTER UPDATE OF remote_id, sync_id, lifecycle_version, active ON feeds
WHEN OLD.remote_id IS NOT NEW.remote_id OR OLD.sync_id IS NOT NEW.sync_id
    OR OLD.lifecycle_version IS NOT NEW.lifecycle_version OR OLD.active IS NOT NEW.active
BEGIN
    UPDATE articles SET remote_origin_feed_id = NULL WHERE feed_id = NEW.id;
END;

-- Reuse the source lifecycle as the in-flight identity token. Value comparison
-- alone cannot reject a remote binding A -> NULL -> A while HTTP is in flight.
CREATE TRIGGER feeds_remote_identity_advance
AFTER UPDATE OF remote_id, sync_id ON feeds
WHEN (OLD.remote_id IS NOT NEW.remote_id OR OLD.sync_id IS NOT NEW.sync_id)
     AND OLD.lifecycle_version = NEW.lifecycle_version
BEGIN
    SELECT CASE WHEN OLD.lifecycle_version >= 9223372036854775807
        THEN RAISE(ABORT, 'source lifecycle overflow') END;
    UPDATE feeds SET lifecycle_version = lifecycle_version + 1 WHERE id=NEW.id;
END;

CREATE INDEX idx_articles_url_norm ON articles(url_norm);

CREATE INDEX idx_articles_feed_published ON articles(feed_id, published_at);

CREATE INDEX idx_articles_read_published ON articles(is_read, published_at);

CREATE INDEX idx_articles_published_id ON articles(published_at, id);

CREATE INDEX idx_sync_queue_article ON sync_queue(article_id, action);

CREATE INDEX idx_sync_queue_created ON sync_queue(created_at);

CREATE INDEX idx_contents_url_norm ON contents(url_norm);

CREATE INDEX idx_articles_content ON articles(content_id);
CREATE INDEX idx_articles_rss_content ON articles(rss_content_id);
CREATE INDEX idx_articles_body_retention ON articles(body_retention_since,id)
    WHERE body_retention_since IS NOT NULL AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL);
CREATE INDEX idx_articles_body_retention_null ON articles(id)
    WHERE body_retention_since IS NULL AND is_read=1 AND is_starred=0 AND (content_id IS NOT NULL OR rss_content_id IS NOT NULL);

-- Device-local cache age, never a read register or publication timestamp.
-- Replaying the same provider state or physical projection does not renew it.
CREATE TRIGGER trg_articles_retention_ai AFTER INSERT ON articles
WHEN NEW.is_read=1 AND NEW.is_starred=0 AND NEW.body_retention_since IS NULL
BEGIN
    UPDATE articles SET body_retention_since=CAST(strftime('%s','now') AS INTEGER) WHERE id=NEW.id;
END;
CREATE TRIGGER trg_articles_retention_state AFTER UPDATE OF is_read,is_starred ON articles
WHEN NEW.is_read IS NOT OLD.is_read OR NEW.is_starred IS NOT OLD.is_starred
BEGIN
    UPDATE articles SET body_retention_since=CASE WHEN NEW.is_read=1 AND NEW.is_starred=0
        THEN CAST(strftime('%s','now') AS INTEGER) END WHERE id=NEW.id;
END;
CREATE TRIGGER trg_articles_retention_restore AFTER UPDATE OF content_id ON articles
WHEN OLD.content_id IS NULL AND NEW.content_id IS NOT NULL
 AND EXISTS(SELECT 1 FROM contents WHERE id=NEW.content_id AND (COALESCE(content_html,'')!='' OR body_text!=''))
BEGIN
    UPDATE articles SET body_retention_since=CASE WHEN NEW.is_read=1 AND NEW.is_starred=0
        THEN CAST(strftime('%s','now') AS INTEGER) END WHERE id=NEW.id;
END;

CREATE INDEX idx_subscription_outbox_url ON subscription_outbox(feed_url);

CREATE INDEX idx_subscription_outbox_pending ON subscription_outbox(status) WHERE status = 'pending';

CREATE INDEX idx_articles_url_norm_unread
            ON articles(url_norm) WHERE is_read = 0;

CREATE INDEX idx_sync_queue_status ON sync_queue(status);

CREATE TRIGGER trg_articles_published_fallback_ins AFTER INSERT ON articles
        WHEN NEW.published_at IS NULL OR NEW.published_at = ''
        BEGIN
            UPDATE articles SET published_at = NEW.fetched_at WHERE id = NEW.id;
        END;

CREATE TRIGGER trg_articles_published_fallback_upd AFTER UPDATE OF published_at ON articles
        WHEN NEW.published_at IS NULL OR NEW.published_at = ''
        BEGIN
            UPDATE articles SET published_at = NEW.fetched_at WHERE id = NEW.id;
        END;

CREATE TRIGGER trg_articles_recycle_ad AFTER DELETE ON articles BEGIN
            DELETE FROM contents WHERE id IN (old.content_id,old.rss_content_id)
                AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = contents.id OR a.rss_content_id = contents.id)
                AND NOT EXISTS (SELECT 1 FROM local_sync_body_hold h WHERE json_extract(h.body_ref,'$.primary')=contents.identity OR json_extract(h.body_ref,'$.rss')=contents.identity);
        END;

CREATE TRIGGER trg_articles_recycle_au AFTER UPDATE OF content_id,rss_content_id ON articles BEGIN
            DELETE FROM contents WHERE id IN (old.content_id,old.rss_content_id)
                AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = contents.id OR a.rss_content_id = contents.id)
                AND NOT EXISTS (SELECT 1 FROM local_sync_body_hold h WHERE json_extract(h.body_ref,'$.primary')=contents.identity OR json_extract(h.body_ref,'$.rss')=contents.identity);
        END;

CREATE TRIGGER trg_articles_trigram_ai AFTER INSERT ON articles BEGIN
            INSERT INTO articles_trigram_idx(rowid, doc)
            VALUES (new.id, lower(COALESCE(new.title,'') || ' ' || COALESCE(new.summary,'') || ' ' ||
                   COALESCE(new.ai_summary,'') || ' ' || COALESCE(new.translated_content,'') || ' ' ||
                   COALESCE((SELECT body_text FROM contents WHERE id = new.content_id), '')));
        END;

CREATE TRIGGER trg_articles_trigram_ad AFTER DELETE ON articles BEGIN
            DELETE FROM articles_trigram_idx WHERE rowid = old.id;
        END;

CREATE TRIGGER trg_articles_trigram_au AFTER UPDATE OF title, summary, ai_summary, translated_content, content_id ON articles BEGIN
            DELETE FROM articles_trigram_idx WHERE rowid = old.id;
            INSERT INTO articles_trigram_idx(rowid, doc)
            VALUES (new.id, lower(COALESCE(new.title,'') || ' ' || COALESCE(new.summary,'') || ' ' ||
                   COALESCE(new.ai_summary,'') || ' ' || COALESCE(new.translated_content,'') || ' ' ||
                   COALESCE((SELECT body_text FROM contents WHERE id = new.content_id), '')));
        END;

CREATE TRIGGER trg_contents_trigram_au AFTER UPDATE OF body_text ON contents BEGIN
            DELETE FROM articles_trigram_idx WHERE rowid IN (SELECT id FROM articles WHERE content_id = new.id);
            INSERT INTO articles_trigram_idx(rowid, doc)
                SELECT a.id, lower(COALESCE(a.title,'') || ' ' || COALESCE(a.summary,'') || ' ' ||
                       COALESCE(a.ai_summary,'') || ' ' || COALESCE(a.translated_content,'') || ' ' ||
                       new.body_text)
                  FROM articles a WHERE a.content_id = new.id;
        END;

CREATE TRIGGER trg_sync_queue_article_bound
        AFTER UPDATE OF remote_origin_feed_id ON articles
        WHEN NEW.remote_id IS NOT NULL AND NEW.remote_origin_feed_id IS NOT NULL
             AND EXISTS(SELECT 1 FROM feeds f WHERE f.id=NEW.feed_id AND f.active=1
                 AND (f.remote_id IS NULL OR f.remote_id=NEW.remote_origin_feed_id))
        BEGIN
            UPDATE sync_queue
               SET status = 'bound_pending',
                   unbound_reason = NULL,
                   last_error = NULL,
                   last_error_class = NULL,
                   updated_at = datetime('now')
             WHERE article_id = NEW.id
               AND kind = 'article_state'
               AND status IN ('local_only', 'bindable_retry');
        END;

CREATE TRIGGER trg_sync_queue_origin_waiting
        AFTER UPDATE OF remote_origin_feed_id ON articles
        WHEN NEW.remote_origin_feed_id IS NULL
        BEGIN
            UPDATE sync_queue
               SET status = 'bindable_retry',
                   unbound_reason = 'awaiting_verified_origin',
                   last_error_class = 'unbound',
                   updated_at = datetime('now')
             WHERE article_id = NEW.id AND kind = 'article_state'
               AND status IN ('bound_pending', 'retry_wait');
        END;

CREATE TRIGGER trg_articles_first_seen_fallback_ins
        AFTER INSERT ON articles
        WHEN NEW.first_seen_at IS NULL OR trim(NEW.first_seen_at) = ''
        BEGIN
            UPDATE articles
               SET first_seen_at = COALESCE(
                 CASE
                   WHEN NEW.fetched_at IS NULL OR trim(NEW.fetched_at) = '' THEN NULL
                   WHEN NEW.fetched_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]'
                     THEN replace(NEW.fetched_at, ' ', 'T') || '+00:00'
                   ELSE NEW.fetched_at
                 END,
                 CASE
                   WHEN NEW.published_at IS NULL OR trim(NEW.published_at) = '' THEN NULL
                   WHEN NEW.published_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]'
                     THEN replace(NEW.published_at, ' ', 'T') || '+00:00'
                   ELSE NEW.published_at
                 END,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
               )
             WHERE id = NEW.id;
        END;

CREATE TRIGGER trg_feeds_refresh_title_defaults_ins
        AFTER INSERT ON feeds
        WHEN NEW.fetched_title = '' OR NEW.title_custom IS NULL
        BEGIN
            UPDATE feeds
               SET fetched_title = CASE
                     WHEN NEW.fetched_title = '' THEN NEW.title
                     ELSE NEW.fetched_title
                   END,
                   title_custom = CASE
                     WHEN NEW.title_custom IS NULL THEN ''
                     ELSE NEW.title_custom
                   END
             WHERE id = NEW.id;
        END;

CREATE INDEX idx_articles_rss_group ON articles(feed_id,rss_content_id,title,id);
CREATE UNIQUE INDEX idx_feeds_sync_id ON feeds(sync_id);
CREATE TRIGGER trg_local_feed_folder_insert AFTER INSERT ON feeds
WHEN NEW.origin!='remote' OR NEW.origin_was_local=1
BEGIN
    UPDATE folders SET origin='local' WHERE id=NEW.folder_id AND origin='remote';
END;
CREATE TRIGGER trg_local_feed_folder_update AFTER UPDATE OF folder_id,origin,origin_was_local ON feeds
WHEN NEW.origin!='remote' OR NEW.origin_was_local=1
BEGIN
    UPDATE folders SET origin='local' WHERE id=NEW.folder_id AND origin='remote';
END;
CREATE TRIGGER trg_source_local_history AFTER UPDATE OF origin ON feeds
WHEN OLD.origin!='remote' AND NEW.origin='remote' AND NEW.origin_was_local=0
BEGIN
    UPDATE feeds SET origin_was_local=1 WHERE id=NEW.id;
END;
CREATE TRIGGER trg_articles_media_au AFTER UPDATE OF content_id,image_url,enclosure_url,enclosure_mime,duration_sec ON articles
WHEN NEW.images_json != (CASE WHEN COALESCE((SELECT image_count FROM contents WHERE id=NEW.content_id),0)>0 THEN (SELECT images_json FROM contents WHERE id=NEW.content_id) WHEN NEW.image_url IS NOT NULL AND trim(NEW.image_url)!='' THEN json_array(NEW.image_url) ELSE '[]' END)
  OR NEW.image_url IS NOT OLD.image_url
  OR NEW.enclosure_url IS NOT OLD.enclosure_url
  OR NEW.enclosure_mime IS NOT OLD.enclosure_mime
  OR NEW.duration_sec IS NOT OLD.duration_sec
BEGIN
    UPDATE articles SET images_json=(CASE WHEN COALESCE((SELECT image_count FROM contents WHERE id=NEW.content_id),0)>0 THEN (SELECT images_json FROM contents WHERE id=NEW.content_id) WHEN NEW.image_url IS NOT NULL AND trim(NEW.image_url)!='' THEN json_array(NEW.image_url) ELSE '[]' END), image_count=json_array_length((CASE WHEN COALESCE((SELECT image_count FROM contents WHERE id=NEW.content_id),0)>0 THEN (SELECT images_json FROM contents WHERE id=NEW.content_id) WHEN NEW.image_url IS NOT NULL AND trim(NEW.image_url)!='' THEN json_array(NEW.image_url) ELSE '[]' END)),
        media_revision=media_revision+1 WHERE id=NEW.id;
END;


-- Durable local-library sync. Secrets remain in the protected settings store.
CREATE TABLE local_sync_control (
 id INTEGER PRIMARY KEY CHECK(id=1), actor TEXT NOT NULL DEFAULT (lower(hex(randomblob(16)))),
 library_id TEXT NOT NULL DEFAULT '', generation INTEGER NOT NULL DEFAULT 0,
 capture INTEGER NOT NULL DEFAULT 0, initialized INTEGER NOT NULL DEFAULT 0, applying INTEGER NOT NULL DEFAULT 0,
 seq INTEGER NOT NULL DEFAULT 0, lamport INTEGER NOT NULL DEFAULT 0,
 frontier TEXT NOT NULL DEFAULT '{}', published_seq INTEGER NOT NULL DEFAULT 0,
 publication INTEGER NOT NULL DEFAULT 0, last_state_sync INTEGER NOT NULL DEFAULT 0,
 last_body_sync INTEGER NOT NULL DEFAULT 0, next_retry_at INTEGER NOT NULL DEFAULT 0,
 failures INTEGER NOT NULL DEFAULT 0, last_error TEXT, data_revision INTEGER NOT NULL DEFAULT 0,
 logical_revision INTEGER NOT NULL DEFAULT 0, accepted_job TEXT,
 completed_frontier TEXT NOT NULL DEFAULT '{}', folding INTEGER NOT NULL DEFAULT 0,
 bootstrap_cursor INTEGER NOT NULL DEFAULT 0,bootstrap_phase TEXT NOT NULL DEFAULT 'groups',
 selected_baseline TEXT, selected_base TEXT, fast_stored_seq INTEGER NOT NULL DEFAULT 0
);
INSERT INTO local_sync_control(id) VALUES(1);
CREATE TABLE local_sync_fields(key TEXT PRIMARY KEY, register TEXT NOT NULL);
CREATE INDEX local_sync_article_guid ON local_sync_fields(json_extract(register,'$.value.value.guid')) WHERE key LIKE 'a/%/meta';
CREATE TABLE local_sync_published_fields(key TEXT PRIMARY KEY, register TEXT NOT NULL);
CREATE TABLE local_sync_journal(seq INTEGER PRIMARY KEY, lamport INTEGER NOT NULL, observed TEXT NOT NULL, patch TEXT NOT NULL);
CREATE TABLE local_sync_epochs(source_key TEXT PRIMARY KEY, stamp TEXT NOT NULL);
CREATE TABLE local_sync_pending(lane TEXT PRIMARY KEY CHECK(lane IN ('state','body')), generation INTEGER NOT NULL, publication INTEGER NOT NULL, payload TEXT NOT NULL);
CREATE TABLE local_sync_objects(actor TEXT NOT NULL, kind TEXT NOT NULL, digest TEXT NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY(actor,kind,digest));
CREATE TABLE local_sync_object_links(actor TEXT NOT NULL,kind TEXT NOT NULL,parent TEXT NOT NULL,child TEXT NOT NULL,PRIMARY KEY(actor,kind,parent,child));
CREATE INDEX local_sync_object_child ON local_sync_object_links(actor,kind,child,parent);
CREATE TABLE local_sync_object_pins(owner TEXT NOT NULL,actor TEXT NOT NULL,kind TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(owner,actor,kind,digest));
CREATE TABLE local_sync_body_hold(id INTEGER PRIMARY KEY AUTOINCREMENT,
 article_id INTEGER REFERENCES articles(id) ON DELETE SET NULL,
 entry TEXT NOT NULL,body_ref TEXT NOT NULL,UNIQUE(entry,body_ref));
CREATE INDEX local_sync_hold_article ON local_sync_body_hold(article_id);
CREATE INDEX local_sync_body_ref ON local_sync_body_hold(body_ref);
CREATE INDEX local_sync_hold_primary ON local_sync_body_hold(json_extract(body_ref,'$.primary'));
CREATE INDEX local_sync_hold_rss ON local_sync_body_hold(json_extract(body_ref,'$.rss'));
CREATE TABLE local_sync_body_receipts(manifest TEXT NOT NULL,entry TEXT NOT NULL,body_ref TEXT NOT NULL,PRIMARY KEY(manifest,entry,body_ref));
CREATE TABLE local_sync_body_receipt_holds(manifest TEXT NOT NULL,
 hold_id INTEGER NOT NULL REFERENCES local_sync_body_hold(id) ON DELETE CASCADE,
 PRIMARY KEY(manifest,hold_id));
CREATE INDEX local_sync_receipt_hold_id ON local_sync_body_receipt_holds(hold_id);
CREATE TABLE local_sync_gc(path TEXT PRIMARY KEY,generation INTEGER NOT NULL);
CREATE TABLE local_sync_lanes(kind TEXT PRIMARY KEY,last_sync INTEGER NOT NULL DEFAULT 0,next_retry INTEGER NOT NULL DEFAULT 0,failures INTEGER NOT NULL DEFAULT 0,error TEXT);
INSERT INTO local_sync_lanes(kind) VALUES('state'),('body');
CREATE TABLE local_sync_downloads(actor TEXT NOT NULL,kind TEXT NOT NULL,manifest TEXT NOT NULL,PRIMARY KEY(actor,kind));

-- F and one immutable accepted C form the only logical register read surface.
-- Folding C into F is a physical move: it must leave this view unchanged.
CREATE TABLE local_sync_receive (
 id TEXT PRIMARY KEY, generation INTEGER NOT NULL, local_actor TEXT NOT NULL,
 library_id TEXT NOT NULL, manifest TEXT NOT NULL, phase TEXT NOT NULL DEFAULT 'staging',
 next_page INTEGER NOT NULL DEFAULT 0, validated_revision INTEGER,
 validated_clock INTEGER NOT NULL DEFAULT 0, fold_cursor TEXT NOT NULL DEFAULT '',
 rejected_code TEXT, rejected_reason TEXT
);
CREATE TABLE local_sync_candidates(job TEXT NOT NULL, key TEXT NOT NULL, register TEXT NOT NULL,
 PRIMARY KEY(job,key), FOREIGN KEY(job) REFERENCES local_sync_receive(id) ON DELETE CASCADE);
CREATE INDEX local_sync_fields_source ON local_sync_fields(json_extract(register,'$.value.value.source'),key) WHERE key LIKE 'a/%/meta';
CREATE INDEX local_sync_fields_url ON local_sync_fields(json_extract(register,'$.value.value'),key) WHERE key LIKE 's/%/feed_url';
CREATE INDEX local_sync_fields_body_rss ON local_sync_fields(json_extract(register,'$.value.value.rss'),key) WHERE key LIKE 'a/%/body';
CREATE INDEX local_sync_candidates_source ON local_sync_candidates(job,json_extract(register,'$.value.value.source'),key) WHERE key LIKE 'a/%/meta';
CREATE INDEX local_sync_candidates_guid ON local_sync_candidates(job,json_extract(register,'$.value.value.guid'),key) WHERE key LIKE 'a/%/meta';
CREATE INDEX local_sync_candidates_url ON local_sync_candidates(job,json_extract(register,'$.value.value'),key) WHERE key LIKE 's/%/feed_url';
CREATE INDEX local_sync_candidates_body_rss ON local_sync_candidates(job,json_extract(register,'$.value.value.rss'),key) WHERE key LIKE 'a/%/body';
CREATE VIEW local_sync_winners AS
 SELECT f.key,f.register FROM local_sync_fields f
 WHERE NOT EXISTS(SELECT 1 FROM local_sync_candidates c
   WHERE c.job=(SELECT accepted_job FROM local_sync_control WHERE id=1) AND c.key=f.key
   AND (json_extract(c.register,'$.stamp.lamport'),json_extract(c.register,'$.stamp.actor'),json_extract(c.register,'$.stamp.seq'))
     > (json_extract(f.register,'$.stamp.lamport'),json_extract(f.register,'$.stamp.actor'),json_extract(f.register,'$.stamp.seq')))
 UNION ALL
 SELECT c.key,c.register FROM local_sync_candidates c
 WHERE c.job=(SELECT accepted_job FROM local_sync_control WHERE id=1)
 AND NOT EXISTS(SELECT 1 FROM local_sync_fields f WHERE f.key=c.key
   AND (json_extract(f.register,'$.stamp.lamport'),json_extract(f.register,'$.stamp.actor'),json_extract(f.register,'$.stamp.seq'))
     >= (json_extract(c.register,'$.stamp.lamport'),json_extract(c.register,'$.stamp.actor'),json_extract(c.register,'$.stamp.seq')));
-- Derived application predicates consume L immediately. These views neither
-- allocate identity nor retain another winner; physical rows are only locators.
CREATE VIEW local_sync_source_urls AS
 SELECT substr(key,3,32) source,json_extract(register,'$.value.value') url
 FROM local_sync_winners WHERE key LIKE 's/%/feed_url';
CREATE VIEW local_sync_source_values AS
 SELECT u.url,substr(w.key,36) field,w.key wire_key,w.register,
 row_number() OVER(PARTITION BY u.url,substr(w.key,36) ORDER BY
 json_extract(w.register,'$.stamp.lamport') DESC,json_extract(w.register,'$.stamp.actor') COLLATE BINARY DESC,json_extract(w.register,'$.stamp.seq') DESC,w.key COLLATE BINARY DESC) winner
 FROM local_sync_source_urls u JOIN local_sync_winners w ON w.key>='s/'||u.source||'/' AND w.key<'s/'||u.source||'0';
CREATE VIEW local_sync_source_state AS
 SELECT u.source,u.url,COALESCE(json_extract(c.register,'$.value.value')=1,0) AND
 (d.register IS NULL OR (r.register IS NOT NULL
 AND (json_extract(r.register,'$.stamp.lamport'),json_extract(r.register,'$.stamp.actor'),json_extract(r.register,'$.stamp.seq'))>
 (json_extract(d.register,'$.stamp.lamport'),json_extract(d.register,'$.stamp.actor'),json_extract(d.register,'$.stamp.seq'))
 AND json_extract(r.register,'$.value.value.lamport')=json_extract(d.register,'$.stamp.lamport')
 AND json_extract(r.register,'$.value.value.actor')=json_extract(d.register,'$.stamp.actor')
 AND json_extract(r.register,'$.value.value.seq')=json_extract(d.register,'$.stamp.seq'))) active
 FROM local_sync_source_urls u
 LEFT JOIN local_sync_source_values c ON c.url=u.url AND c.field='created' AND c.winner=1
 LEFT JOIN local_sync_source_values d ON d.url=u.url AND d.field='deleted' AND d.winner=1
 LEFT JOIN local_sync_source_values r ON r.url=u.url AND r.field='reactivate' AND r.winner=1;
CREATE VIEW local_sync_feed_state AS
 SELECT f.id,COALESCE(s.url,f.feed_url) url,CASE WHEN s.source IS NULL OR (f.origin='remote' AND f.origin_was_local=0) THEN f.active ELSE s.active END active
 FROM feeds f LEFT JOIN local_sync_source_state s ON s.source=f.sync_id;
CREATE VIEW local_sync_article_locator AS
 SELECT a.id,a.local_sync_source source,a.guid,
 CASE WHEN (f.origin='remote' AND f.origin_was_local=0) OR (SELECT initialized=0 FROM local_sync_control WHERE id=1) THEN 0 ELSE 1 END managed,
 EXISTS(SELECT 1 FROM local_sync_winners m WHERE m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.source')=a.local_sync_source AND json_extract(m.register,'$.value.value.guid')=a.guid) valid
 FROM articles a JOIN feeds f ON f.id=a.feed_id;
CREATE VIEW local_sync_article_state AS
 SELECT a.id,a.feed_id,CASE WHEN l.managed=0 THEN f.sync_id ELSE l.source END source,a.guid,
 CASE WHEN l.managed=1 AND l.valid=0 THEN NULL ELSE COALESCE(own.url,fs.url) END url,
 CASE WHEN l.managed=1 AND l.valid=0 THEN 1 ELSE COALESCE(own.active,fs.active) END active,
 (l.managed=0 OR l.valid=1) identity_valid,
 CASE WHEN l.managed=1 AND l.valid=0 THEN 0 ELSE COALESCE((SELECT json_extract(v.register,'$.value.value') FROM local_sync_source_urls u
 JOIN local_sync_winners m ON m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.source')=u.source AND json_extract(m.register,'$.value.value.guid')=a.guid
 JOIN local_sync_winners v ON v.key=substr(m.key,1,66)||'/read' WHERE u.url=COALESCE(own.url,fs.url)
 ORDER BY json_extract(v.register,'$.stamp.lamport') DESC,json_extract(v.register,'$.stamp.actor') COLLATE BINARY DESC,json_extract(v.register,'$.stamp.seq') DESC,v.key COLLATE BINARY DESC LIMIT 1),a.is_read) END is_read,
 CASE WHEN l.managed=1 AND l.valid=0 THEN 0 ELSE COALESCE((SELECT json_extract(v.register,'$.value.value') FROM local_sync_source_urls u
 JOIN local_sync_winners m ON m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.source')=u.source AND json_extract(m.register,'$.value.value.guid')=a.guid
 JOIN local_sync_winners v ON v.key=substr(m.key,1,66)||'/star' WHERE u.url=COALESCE(own.url,fs.url)
 ORDER BY json_extract(v.register,'$.stamp.lamport') DESC,json_extract(v.register,'$.stamp.actor') COLLATE BINARY DESC,json_extract(v.register,'$.stamp.seq') DESC,v.key COLLATE BINARY DESC LIMIT 1),a.is_starred) END is_starred,
 CASE WHEN l.managed=1 AND l.valid=0 THEN NULL ELSE (SELECT json_extract(v.register,'$.value.value') FROM local_sync_source_urls u
 JOIN local_sync_winners m ON m.key LIKE 'a/%/meta' AND json_extract(m.register,'$.value.value.source')=u.source AND json_extract(m.register,'$.value.value.guid')=a.guid
 JOIN local_sync_winners v ON v.key=substr(m.key,1,66)||'/body' WHERE u.url=COALESCE(own.url,fs.url)
 ORDER BY json_extract(v.register,'$.stamp.lamport') DESC,json_extract(v.register,'$.stamp.actor') COLLATE BINARY DESC,json_extract(v.register,'$.stamp.seq') DESC,v.key COLLATE BINARY DESC LIMIT 1) END body_ref
 FROM articles a JOIN feeds f ON f.id=a.feed_id JOIN local_sync_feed_state fs ON fs.id=f.id
 JOIN local_sync_article_locator l ON l.id=a.id LEFT JOIN local_sync_source_state own ON own.source=l.source AND l.valid=1;
CREATE TABLE local_sync_work(job TEXT NOT NULL DEFAULT '',key TEXT NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL DEFAULT '', PRIMARY KEY(job,key));
CREATE TABLE local_sync_known_actors(actor TEXT PRIMARY KEY);
CREATE TABLE local_sync_intents(actor TEXT NOT NULL,seq INTEGER NOT NULL,intent TEXT NOT NULL,
 PRIMARY KEY(actor,seq));
CREATE TABLE local_sync_fast_pending(id INTEGER PRIMARY KEY CHECK(id=1),generation INTEGER NOT NULL,
 actor TEXT NOT NULL,first_seq INTEGER NOT NULL,last_seq INTEGER NOT NULL,path TEXT NOT NULL,bytes BLOB NOT NULL);
CREATE TABLE local_sync_seen_batches(actor TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(actor,digest));
CREATE TABLE local_sync_fast_objects(path TEXT PRIMARY KEY,actor TEXT NOT NULL,
 first_seq INTEGER NOT NULL,last_seq INTEGER NOT NULL,bytes BLOB NOT NULL,generation INTEGER NOT NULL);
CREATE TABLE local_sync_roots(owner TEXT PRIMARY KEY,kind TEXT NOT NULL,reference TEXT NOT NULL);
CREATE TABLE local_sync_baselines(id TEXT PRIMARY KEY,base TEXT NOT NULL,parent TEXT,ready INTEGER NOT NULL DEFAULT 0);
CREATE TABLE local_sync_baseline_fields(version TEXT NOT NULL,key TEXT NOT NULL,register TEXT NOT NULL,
 PRIMARY KEY(version,key));
CREATE INDEX local_sync_baseline_key ON local_sync_baseline_fields(key,version);
CREATE VIEW local_sync_selected_fields AS
 WITH RECURSIVE versions(id,parent,depth) AS (
  SELECT b.id,b.parent,0 FROM local_sync_baselines b JOIN local_sync_control c ON c.selected_baseline=b.id
  UNION ALL SELECT b.id,b.parent,v.depth+1 FROM local_sync_baselines b JOIN versions v ON b.id=v.parent WHERE v.depth<32
 )
 SELECT f.key,f.register FROM local_sync_baseline_fields f JOIN versions v ON v.id=f.version
 WHERE NOT EXISTS(SELECT 1 FROM local_sync_baseline_fields newer JOIN versions n ON n.id=newer.version WHERE newer.key=f.key AND n.depth<v.depth);

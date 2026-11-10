-- Current FluxReader data format; legacy databases are rejected before any write.
PRAGMA application_id = 1179796018;

CREATE TABLE folders (
            id            INTEGER PRIMARY KEY,
            name          TEXT NOT NULL,
            position      INTEGER NOT NULL DEFAULT 0,
            layout        TEXT NOT NULL DEFAULT 'article',
            auto_summary  INTEGER NOT NULL DEFAULT 0,
            auto_translate INTEGER NOT NULL DEFAULT 0,
            collapsed     INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        , remote_id INTEGER, sync_id TEXT,
            summary_mode TEXT NOT NULL DEFAULT 'inherit' CHECK(summary_mode IN ('inherit','on','off')),
            translate_mode TEXT NOT NULL DEFAULT 'inherit' CHECK(translate_mode IN ('inherit','on','off')));

CREATE TABLE feeds (
            id              INTEGER PRIMARY KEY,
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
            id            INTEGER PRIMARY KEY,
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
            is_starred    INTEGER NOT NULL DEFAULT 0, remote_id INTEGER, remote_origin_feed_id INTEGER, url_norm TEXT, read_version INTEGER NOT NULL DEFAULT 0, star_version INTEGER NOT NULL DEFAULT 0, content_id INTEGER REFERENCES contents(id), ai_epoch INTEGER NOT NULL DEFAULT 0, ai_summary_sig TEXT, translated_sig TEXT, first_seen_at TEXT, published_at_source TEXT NOT NULL DEFAULT 'legacy_unknown'
            CHECK (published_at_source IN ('feed', 'feed_clamped', 'inferred_fetch', 'legacy_unknown')),
            rss_content_id INTEGER REFERENCES contents(id),
            images_json TEXT NOT NULL DEFAULT '[]',
            image_count INTEGER NOT NULL DEFAULT 0,
            media_revision INTEGER NOT NULL DEFAULT 0,
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
                AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = contents.id OR a.rss_content_id = contents.id);
        END;

CREATE TRIGGER trg_articles_recycle_au AFTER UPDATE OF content_id,rss_content_id ON articles BEGIN
            DELETE FROM contents WHERE id IN (old.content_id,old.rss_content_id)
                AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.content_id = contents.id OR a.rss_content_id = contents.id);
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

//! Small declarative trigger builder. Ordinary settings use the same allowlist
//! as wire validation and portable export. The generated SQL is not checked in.
use super::model::{local_source, SETTINGS};
const ENABLED: &str = "(SELECT capture=1 AND applying=0 FROM local_sync_control WHERE id=1)";
fn set(value: &str) -> String {
    format!("json_object('op','set','value',{value})")
}
fn record(name: &str, table: &str, event: &str, patch: &str, when: &str, extra: &str) -> String {
    let row = if event == "DELETE" { "OLD" } else { "NEW" };
    let scope = match table {
        "feeds" if event == "DELETE" => {
            "EXISTS(SELECT 1 FROM local_sync_winners WHERE key='s/'||OLD.sync_id||'/created')"
                .to_owned()
        }
        "feeds" => local_source(row),
        "folders" => format!("{row}.origin='local'"),
        "articles" => format!(
            "EXISTS(SELECT 1 FROM feeds f WHERE f.id={row}.feed_id AND {})",
            local_source("f")
        ),
        _ => "1".into(),
    };
    format!("CREATE TRIGGER IF NOT EXISTS {name} AFTER {event} ON {table} WHEN {ENABLED} AND ({scope}) AND ({when}) AND ({patch})!='{{}}' BEGIN
      SELECT CASE WHEN (SELECT seq>=9223372036854775807 OR lamport>=9223372036854775807 FROM local_sync_control WHERE id=1) THEN RAISE(ABORT,'local sync logical clock exhausted') END;
      UPDATE local_sync_control SET seq=seq+1,lamport=lamport+1,logical_revision=logical_revision+1 WHERE id=1;
      INSERT INTO local_sync_journal(seq,lamport,observed,patch) SELECT seq,lamport,frontier,{patch} FROM local_sync_control WHERE id=1;
      INSERT INTO local_sync_work(job,key,revision) SELECT '','url/'||j.value,c.logical_revision FROM local_sync_control c,json_each(flx_affected_urls((SELECT patch FROM local_sync_journal WHERE seq=c.seq))) j WHERE c.id=1
        ON CONFLICT(job,key) DO UPDATE SET revision=excluded.revision,cursor='';
      INSERT INTO local_sync_work(job,key,revision) SELECT '',j.value,c.logical_revision FROM local_sync_control c,json_each(flx_meta_alias_work((SELECT patch FROM local_sync_journal WHERE seq=c.seq))) j WHERE c.id=1
        ON CONFLICT(job,key) DO UPDATE SET revision=excluded.revision,cursor='';
      INSERT INTO local_sync_fields(key,register) SELECT j.key,json_object('stamp',json_object('lamport',c.lamport,'actor',c.actor,'seq',c.seq),'value',json(j.value)) FROM local_sync_control c,json_each((SELECT patch FROM local_sync_journal WHERE seq=c.seq)) j WHERE c.id=1 ON CONFLICT(key) DO UPDATE SET register=excluded.register;
      SELECT flx_validate_capture(patch) FROM local_sync_journal WHERE seq=(SELECT seq FROM local_sync_control WHERE id=1);
      UPDATE local_sync_control SET frontier=json_set(frontier,'$.'||actor,seq) WHERE id=1;
      {extra} END;\n")
}
struct Field {
    name: String,
    value: String,
    changed: String,
}
fn column(name: &str) -> Field {
    Field {
        name: name.into(),
        value: format!("NEW.{name}"),
        changed: format!("NEW.{name} IS NOT OLD.{name}"),
    }
}
fn patch(prefix: &str, fields: &[Field], updates: bool) -> String {
    let select: Vec<_> = fields
        .iter()
        .map(|f| {
            format!(
                "SELECT {prefix}||'/{}' k,{} v{}",
                f.name,
                set(&f.value),
                if updates {
                    format!(" WHERE {}", f.changed)
                } else {
                    format!(" WHERE NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key={prefix}||'/{}')",f.name)
                }
            )
        })
        .collect();
    format!(
        "(SELECT json_group_object(k,json(v)) FROM ({}))",
        select.join(" UNION ALL ")
    )
}
fn changed(fields: &[Field]) -> String {
    fields
        .iter()
        .map(|f| f.changed.as_str())
        .collect::<Vec<_>>()
        .join(" OR ")
}
fn create() -> Field {
    Field {
        name: "created".into(),
        value: "json('true')".into(),
        changed: "0".into(),
    }
}
pub fn sql() -> String {
    let mut sql = super::logical::capture_sql();
    // Ordinary SQL capture validates the completed patch through the same
    // projection/affected-closure code as explicit and fast intents. It includes
    // source fields and accepted C, rather than a separate alias winner guard.
    // Locator initialization is independent of whether fill-only capture emits
    // any fields. Existing non-null hints are never silently reassigned.
    for (name, event) in [("insert", "INSERT"), ("adopt", "UPDATE OF feed_id")] {
        sql+=&format!("CREATE TRIGGER IF NOT EXISTS flx_article_locator_{name} AFTER {event} ON articles WHEN {ENABLED} AND NEW.local_sync_source IS NULL AND EXISTS(SELECT 1 FROM feeds f WHERE f.id=NEW.feed_id AND {}) BEGIN UPDATE articles SET local_sync_source=(SELECT sync_id FROM feeds WHERE id=NEW.feed_id) WHERE id=NEW.id AND (EXISTS(SELECT 1 FROM local_sync_winners WHERE key='a/'||flx_entry((SELECT sync_id FROM feeds WHERE id=NEW.feed_id),NEW.guid)||'/meta') OR NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key LIKE 'a/%/meta' AND json_extract(register,'$.value.value.guid')=NEW.guid)); END;\n",local_source("f"));
    }
    let source = "'s/'||NEW.sync_id";
    let mut feeds: Vec<_> = [
        "feed_url",
        "title",
        "title_custom",
        "fetched_title",
        "site_url",
        "favicon_url",
        "layout",
        "summary_mode",
        "translate_mode",
    ]
    .into_iter()
    .map(column)
    .collect();
    feeds.push(Field {
        name: "folder".into(),
        value: "(SELECT sync_id FROM folders WHERE id=NEW.folder_id)".into(),
        changed: "NEW.folder_id IS NOT OLD.folder_id".into(),
    });
    // A local source joining a remote-created group is one logical operation.
    // Promote and capture the group before the feed patch is validated; relying
    // on migration trigger creation order can otherwise validate the source
    // before the referenced group has an explicit created field.
    sql += &format!(
        "CREATE TRIGGER IF NOT EXISTS flx_feed_group_promote_insert BEFORE INSERT ON feeds
         WHEN {ENABLED} AND {local} AND NEW.folder_id IS NOT NULL
          AND EXISTS(SELECT 1 FROM folders g WHERE g.id=NEW.folder_id AND g.origin='remote')
         BEGIN UPDATE folders SET origin='local' WHERE id=NEW.folder_id AND origin='remote'; END;\n
         CREATE TRIGGER IF NOT EXISTS flx_feed_group_promote_update BEFORE UPDATE OF folder_id,origin,origin_was_local ON feeds
         WHEN {ENABLED} AND {local} AND NEW.folder_id IS NOT NULL
          AND EXISTS(SELECT 1 FROM folders g WHERE g.id=NEW.folder_id AND g.origin='remote')
         BEGIN UPDATE folders SET origin='local' WHERE id=NEW.folder_id AND origin='remote'; END;\n",
        local = local_source("NEW")
    );
    sql+=&record("flx_feed_fields","feeds","UPDATE OF feed_url,title,title_custom,fetched_title,site_url,favicon_url,layout,summary_mode,translate_mode,folder_id",&patch(source,&feeds,true),&changed(&feeds),"");
    feeds.push(create());
    sql += &record(
        "flx_feed_insert",
        "feeds",
        "INSERT",
        &patch(source, &feeds, false),
        "1",
        "",
    );
    sql += &record(
        "flx_feed_promoted",
        "feeds",
        "UPDATE OF origin,origin_was_local",
        &patch(source, &feeds, false),
        &format!("NOT {}", local_source("OLD")),
        "UPDATE articles SET feed_id=feed_id,content_id=content_id WHERE feed_id=NEW.id;",
    );
    sql+=&record("flx_feed_removed","feeds","DELETE",&format!("json_object('s/'||OLD.sync_id||'/deleted',{})",set("json('true')")),"OLD.active=1 OR NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key='s/'||OLD.sync_id||'/deleted')","INSERT INTO local_sync_epochs(source_key,stamp) SELECT OLD.sync_id,json_object('lamport',lamport,'actor',actor,'seq',seq) FROM local_sync_control WHERE id=1 ON CONFLICT(source_key) DO UPDATE SET stamp=excluded.stamp;");
    sql+=&record("flx_feed_deleted","feeds","UPDATE OF active",&format!("json_object({source}||'/deleted',{})",set("json('true')")),"OLD.active=1 AND NEW.active=0","INSERT INTO local_sync_epochs(source_key,stamp) SELECT NEW.sync_id,json_object('lamport',lamport,'actor',actor,'seq',seq) FROM local_sync_control WHERE id=1 ON CONFLICT(source_key) DO UPDATE SET stamp=excluded.stamp;");
    sql += &record(
        "flx_feed_reactivated",
        "feeds",
        "UPDATE OF active",
        &format!(
            "json_object({source}||'/reactivate',{})",
            set("json((SELECT stamp FROM local_sync_epochs WHERE source_key=NEW.sync_id))")
        ),
        "OLD.active=0 AND NEW.active=1",
        "",
    );
    let group = "'g/'||NEW.sync_id";
    let mut groups: Vec<_> = [
        "name",
        "layout",
        "summary_mode",
        "translate_mode",
        "position",
        "collapsed",
    ]
    .into_iter()
    .map(column)
    .collect();
    sql += &record(
        "flx_group_fields",
        "folders",
        "UPDATE OF name,layout,summary_mode,translate_mode,position,collapsed",
        &patch(group, &groups, true),
        &changed(&groups),
        "",
    );
    groups.push(create());
    sql += &record(
        "flx_group_promoted",
        "folders",
        "UPDATE OF origin",
        &patch(group, &groups, false),
        "OLD.origin='remote' AND NEW.sync_id IS NOT NULL",
        "",
    );
    sql += &record(
        "flx_group_insert",
        "folders",
        "INSERT",
        &patch(group, &groups, false),
        "NEW.sync_id IS NOT NULL",
        "",
    );
    sql += &record(
        "flx_group_delete",
        "folders",
        "DELETE",
        &format!(
            "json_object('g/'||OLD.sync_id||'/deleted',{})",
            set("json('true')")
        ),
        "OLD.sync_id IS NOT NULL",
        "",
    );
    let entry = "'a/'||flx_entry(COALESCE(NEW.local_sync_source,(SELECT sync_id FROM feeds WHERE id=NEW.feed_id)),NEW.guid)";
    let meta="json_object('source',COALESCE(NEW.local_sync_source,(SELECT sync_id FROM feeds WHERE id=NEW.feed_id)),'guid',NEW.guid,'url',NEW.url,'title',NEW.title,'author',NEW.author,'published_at',NEW.published_at,'published_at_source',NEW.published_at_source,'first_seen_at',NEW.first_seen_at,'image_url',NEW.image_url,'image_source',NEW.image_source,'enclosure_url',NEW.enclosure_url,'enclosure_mime',NEW.enclosure_mime,'duration_sec',NEW.duration_sec)";
    let metadata_columns: Vec<_> = [
        "url",
        "title",
        "author",
        "published_at",
        "published_at_source",
        "first_seen_at",
        "image_url",
        "image_source",
        "enclosure_url",
        "enclosure_mime",
        "duration_sec",
    ]
    .into_iter()
    .map(column)
    .collect();
    // Metadata is one wire field. Automatic ingestion reports only columns it
    // actually changed, merged onto L, never onto a stale physical row image.
    let meta_delta = metadata_columns
        .iter()
        .map(|field| {
            format!(
                "SELECT '{}' k,NEW.{} v WHERE {}",
                field.name, field.name, field.changed
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let merged_meta=format!("json_patch(COALESCE((SELECT json_extract(register,'$.value.value') FROM local_sync_winners WHERE key={entry}||'/meta'),{meta}),(SELECT json_group_object(k,v) FROM ({meta_delta})))");
    let articles = vec![
        Field {
            name: "meta".into(),
            value: meta.into(),
            changed: changed(&metadata_columns),
        },
        Field {
            name: "read".into(),
            value: "json(CASE WHEN NEW.is_read=1 THEN 'true' ELSE 'false' END)".into(),
            changed: "NEW.is_read IS NOT OLD.is_read".into(),
        },
        Field {
            name: "star".into(),
            value: "json(CASE WHEN NEW.is_starred=1 THEN 'true' ELSE 'false' END)".into(),
            changed: "NEW.is_starred IS NOT OLD.is_starred".into(),
        },
    ];
    sql += &record(
        "flx_article_insert",
        "articles",
        "INSERT",
        &patch(entry, &articles, false),
        "1",
        "UPDATE articles SET local_sync_source=(SELECT sync_id FROM feeds WHERE id=NEW.feed_id) WHERE id=NEW.id AND local_sync_source IS NULL;",
    );
    sql += &record(
        "flx_article_adopt",
        "articles",
        "UPDATE OF feed_id",
        &patch(entry, &articles, false),
        &format!("NOT EXISTS(SELECT 1 FROM local_sync_winners WHERE key={entry}||'/meta')"),
        "UPDATE articles SET local_sync_source=(SELECT sync_id FROM feeds WHERE id=NEW.feed_id) WHERE id=NEW.id AND local_sync_source IS NULL;",
    );
    let mut update_articles = articles;
    update_articles[0].value = merged_meta;
    sql+=&record("flx_article_fields","articles","UPDATE OF url,title,author,published_at,published_at_source,first_seen_at,image_url,image_source,enclosure_url,enclosure_mime,duration_sec,is_read,is_starred",&patch(entry,&update_articles,true),&changed(&update_articles),"");
    let body="json_object('primary',(SELECT identity FROM contents WHERE id=NEW.content_id),'rss',(SELECT identity FROM contents WHERE id=NEW.rss_content_id))";
    let exists="EXISTS(SELECT 1 FROM contents WHERE id=NEW.content_id AND (COALESCE(content_html,'')!='' OR body_text!=''))";
    let hold=format!("INSERT OR IGNORE INTO local_sync_body_hold(article_id,entry,body_ref) SELECT NEW.id,substr({entry},3),{body} WHERE NEW.is_read=0 OR NEW.is_starred=1;");
    let changed=format!("(SELECT json_extract(register,'$.value.value') FROM local_sync_winners WHERE key={entry}||'/body') IS NOT {body}");
    sql += &record(
        "flx_article_body",
        "articles",
        "UPDATE OF content_id,rss_content_id",
        &format!("json_object({entry}||'/body',{})", set(body)),
        &format!("({changed}) AND ({exists})"),
        &hold,
    );
    sql+=&format!("CREATE TRIGGER IF NOT EXISTS flx_article_protection AFTER UPDATE OF is_read,is_starred ON articles WHEN {ENABLED} AND EXISTS(SELECT 1 FROM feeds f WHERE f.id=NEW.feed_id AND {}) AND (NEW.is_read=0 OR NEW.is_starred=1) AND ({exists}) BEGIN {hold} END;\n",local_source("f"));
    sql+=&format!("CREATE TRIGGER IF NOT EXISTS flx_content_body AFTER UPDATE OF identity,content_html,body_text ON contents WHEN {ENABLED} BEGIN UPDATE articles SET content_id=content_id WHERE content_id=NEW.id OR rss_content_id=NEW.id; END;\n");
    let allowed = SETTINGS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    for (name, event, old) in [
        ("flx_settings_insert", "INSERT", "'{}'"),
        ("flx_settings_update", "UPDATE OF value", "OLD.value"),
    ] {
        let patch=format!("(SELECT json_group_object('p/'||key,json(v)) FROM (SELECT n.key,json_object('op','set','value',CASE WHEN n.type IN ('true','false') THEN json(n.type) WHEN n.type IN ('array','object') THEN json(n.value) ELSE n.value END) v FROM json_each(NEW.value) n WHERE n.key IN ({allowed}) AND (json_type({old},'$.\"'||n.key||'\"') IS NULL OR json_extract({old},'$.\"'||n.key||'\"') IS NOT n.value) UNION ALL SELECT o.key,json_object('op','delete') v FROM json_each({old}) o WHERE o.key IN ({allowed}) AND json_type(NEW.value,'$.\"'||o.key||'\"') IS NULL))");
        sql+=&record(name,"settings",event,&patch,&format!("NEW.key='app_settings' AND json_valid(NEW.value) AND json_type(NEW.value)='object' AND {patch}!='{{}}'"),"");
    }
    sql
}

//! Portable application data. Field names are a strict allowlist, not settings dumps.
use crate::{
    db,
    error::{AppError, AppResult},
    webdav_transport::{Checkpoint, FieldValue, Register},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const FORMAT: u32 = 3;
pub const MAX_FIELDS: usize = 1_000_000;
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
/// One WebDAV source-scope predicate. Account mirroring/remote bindings do not
/// change a source that was originally local. Portable backups intentionally do
/// not use this filter: they retain every locally readable favorite.
pub(super) fn local_source(alias: &str) -> String {
    format!("({alias}.origin!='remote' OR {alias}.origin_was_local=1)")
}
pub use crate::db::SYNC_SETTING_KEYS as SETTINGS;

pub fn invalid(message: impl Into<String>) -> AppError {
    AppError::new("localSyncFormat", message)
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn entry_key(source: &str, guid: &str) -> String {
    digest(format!("{source}\0{guid}").as_bytes())
}
pub fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn value<'a>(
    fields: &'a BTreeMap<String, Register>,
    prefix: &str,
    field: &str,
) -> Option<&'a Value> {
    match &fields.get(&format!("{prefix}/{field}"))?.value {
        FieldValue::Set(value) => Some(value),
        FieldValue::Delete => None,
    }
}
pub fn active_source(fields: &BTreeMap<String, Register>, prefix: &str) -> bool {
    if value(fields, prefix, "created") != Some(&Value::Bool(true)) {
        return false;
    }
    let Some(deleted) = fields.get(&format!("{prefix}/deleted")) else {
        return true;
    };
    let Some(reactivate) = fields.get(&format!("{prefix}/reactivate")) else {
        return false;
    };
    reactivate.stamp > deleted.stamp
        && matches!(&reactivate.value, FieldValue::Set(v) if serde_json::to_value(&deleted.stamp).ok().as_ref() == Some(v))
}

/// A standalone deletion tombstone is valid without prior creation. Every live
/// group needs its explicit creation register; ordinary fields cannot create or
/// resurrect it. Capture, trial and candidate projection share this contract.
pub(super) fn validate_group_entity(
    fields: &BTreeMap<String, Register>,
    prefix: &str,
) -> AppResult<()> {
    if value(fields, prefix, "deleted") != Some(&Value::Bool(true))
        && value(fields, prefix, "created") != Some(&Value::Bool(true))
    {
        return Err(invalid("分类缺少显式创建"));
    }
    Ok(())
}

/// Display aliases are different wire keys, not duplicate CRDT registers.
/// Keep provenance until selection: Stamp first, then the original key's UTF-8
/// bytes (larger wins). SQLite consumers use the same BINARY key ordering.
/// This transient selector never changes the stored key, stamp or epoch.
#[derive(Clone, Default)]
pub(super) struct Projection {
    fields: BTreeMap<String, (String, Register)>,
}
impl Projection {
    pub fn select(
        &mut self,
        display_key: String,
        wire_key: &str,
        register: Register,
    ) -> AppResult<()> {
        if let Some((old_key, old)) = self.fields.get(&display_key) {
            if old_key == wire_key && old.stamp == register.stamp && old.value != register.value {
                return Err(invalid("同原始字段与操作戳包含不同值"));
            }
            if (&old.stamp, old_key.as_bytes()) >= (&register.stamp, wire_key.as_bytes()) {
                return Ok(());
            }
        }
        self.fields
            .insert(display_key, (wire_key.to_owned(), register));
        Ok(())
    }
    pub fn into_fields(self) -> BTreeMap<String, Register> {
        self.fields
            .into_iter()
            .map(|(key, (_, reg))| (key, reg))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BodyRef {
    pub primary: String,
    pub rss: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArticleMeta {
    pub source: String,
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub published_at_source: String,
    pub first_seen_at: Option<String>,
    pub image_url: Option<String>,
    pub image_source: String,
    pub enclosure_url: Option<String>,
    pub enclosure_mime: Option<String>,
    pub duration_sec: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyContent {
    pub identity: String,
    pub html: Option<String>,
    pub text: String,
    pub source: String,
    pub extracted: bool,
}
impl BodyContent {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(self.source.as_str(), "rss" | "readability")
            || self.html.as_ref().map_or(0, String::len) + self.text.len() > MAX_BODY_BYTES
            || !db::content_non_empty(self.html.as_deref(), &self.text)
            || db::portable_content_identity(&self.source, self.html.as_deref(), &self.text)
                != self.identity
        {
            return Err(invalid("正文类型、内容身份或大小校验失败"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyRecord {
    pub entry: String,
    pub body_ref: BodyRef,
    pub primary: BodyContent,
    pub rss: Option<BodyContent>,
}
impl BodyRecord {
    pub fn validate(&self) -> AppResult<()> {
        self.primary.validate()?;
        if let Some(rss) = &self.rss {
            rss.validate()?;
        }
        if !valid_hex(&self.entry, 64)
            || self.primary.identity != self.body_ref.primary
            || self.rss.as_ref().map(|body| &body.identity) != self.body_ref.rss.as_ref()
        {
            return Err(invalid("正文引用不完整或不匹配"));
        }
        Ok(())
    }
}

pub fn validate_setting(key: &str, value: &Value) -> AppResult<()> {
    if !SETTINGS.contains(&key) {
        return Err(invalid(format!("设置 {key} 不属于同步范围")));
    }
    crate::db::validate_app_setting(key, value).map_err(|error| invalid(error.message))
}

pub fn validate_checkpoint(checkpoint: &Checkpoint) -> AppResult<()> {
    checkpoint.validate().map_err(super::transport_error)?;
    if checkpoint.fields.len() > MAX_FIELDS {
        return Err(invalid("库超过一百万字段容量；本地数据保留待同步"));
    }
    for (key, register) in &checkpoint.fields {
        let parts: Vec<&str> = key.split('/').collect();
        let v = match &register.value {
            FieldValue::Set(v) => Some(v),
            FieldValue::Delete => None,
        };
        if parts.first() == Some(&"p") && parts.len() == 2 && SETTINGS.contains(&parts[1]) {
            if let Some(v) = v {
                validate_setting(parts[1], v)?;
            }
            continue;
        }
        if parts.len() != 3
            || !matches!(parts[0], "s" | "a" | "g")
            || !valid_hex(parts[1], if parts[0] == "a" { 64 } else { 32 })
        {
            return Err(invalid("库中出现未允许的字段或身份"));
        }
        let Some(v) = v else {
            return Err(invalid("对象字段不能用未类型化删除代替生命周期操作"));
        };
        let valid = match (parts[0], parts[2]) {
            ("s" | "g", "created" | "deleted") => v == &Value::Bool(true),
            ("s", "reactivate") => {
                serde_json::from_value::<crate::webdav_transport::Stamp>(v.clone()).is_ok()
            }
            ("s", "feed_url") => v.as_str().is_some_and(valid_url),
            ("s", "folder") => v.is_null() || v.as_str().is_some_and(|s| valid_hex(s, 32)),
            ("s", "title") | ("g", "name") => v
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 16_384),
            ("s", "site_url" | "favicon_url") => {
                v.is_null() || v.as_str().is_some_and(|s| s.len() <= 16_384)
            }
            ("s", "title_custom") => v.is_null() || v.as_str().is_some_and(|s| s.len() <= 16_384),
            ("s", "fetched_title") => v.as_str().is_some_and(|s| s.len() <= 16_384),
            ("s" | "g", "layout") => v.as_str().is_some_and(|s| {
                [
                    "inherit",
                    "article",
                    "social",
                    "image",
                    "podcast",
                    "notification",
                ]
                .contains(&s)
            }),
            ("s" | "g", "summary_mode" | "translate_mode") => v
                .as_str()
                .is_some_and(|s| ["inherit", "on", "off"].contains(&s)),
            ("g", "position") => v.as_i64().is_some(),
            ("g", "collapsed") => v.as_i64().is_some_and(|n| n == 0 || n == 1) || v.is_boolean(),
            ("a", "read" | "star") => v.is_boolean(),
            ("a", "body") => serde_json::from_value::<BodyRef>(v.clone()).is_ok(),
            ("a", "meta") => {
                let m: ArticleMeta =
                    serde_json::from_value(v.clone()).map_err(|_| invalid("文章元数据不完整"))?;
                valid_hex(&m.source, 32)
                    && !m.guid.is_empty()
                    && m.guid.len() <= 65_536
                    && entry_key(&m.source, &m.guid) == parts[1]
                    && m.title.len() <= 65_536
                    && matches!(m.image_source.as_str(), "rss" | "page")
                    && ["feed", "feed_clamped", "inferred_fetch", "legacy_unknown"]
                        .contains(&m.published_at_source.as_str())
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(format!("字段 {key} 的类型、身份或值校验失败")));
        }
    }
    Ok(())
}
fn valid_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

//! 直连条目身份键（guid）的确定性构造。
//!
//! 身份顺序只由稳定来源决定：真实 feed id/guid → 同 feed 规范化 URL →
//! 有源日期时的 title+date → 无 URL/date 时的 title+HTML+正文指纹。抓取时间不进入
//! 任何持久身份；未来日期被钳制时也不使用钳制后的当前时间构造身份。
//!
//! 合成键统一带控制字符前缀，避免与真实 feed id 混淆；DB 只允许把这种弱身份
//! 升级为真实 feed id，不会反向把真实 id 改写成合成键。

use md5::{Digest, Md5};

/// 合成 guid 前缀。控制字符不会出现在正常 feed id 中。
pub(crate) const SYNTHETIC_GUID_PREFIX: &str = "\u{1}fluxreader:identity\u{1}";

fn hash_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Md5::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    format!("{:x}", hasher.finalize())
}

fn synthetic(kind: &str, material: &str) -> String {
    format!(
        "{SYNTHETIC_GUID_PREFIX}{kind}:{}",
        hash_hex(&[kind.as_bytes(), material.as_bytes()])
    )
}

/// 内容/标题指纹（不成 guid 的原始 hex）。同一 feed 内可用它做可重复兜底；
/// 跨 feed 不在同一唯一键空间，绝不共享身份。
pub(crate) fn content_fingerprint(title: &str, html: Option<&str>, body: &str) -> String {
    hash_hex(&[
        b"fluxreader-content-fingerprint-v1",
        title.as_bytes(),
        html.unwrap_or("").as_bytes(),
        body.as_bytes(),
    ])
}

/// 无 id/URL/date 时的内容身份 guid（标题 + HTML + 提取正文）。
pub(crate) fn content_identity_guid(title: &str, html: Option<&str>, body: &str) -> String {
    synthetic("content", &content_fingerprint(title, html, body))
}

/// 同 feed 规范化 URL 身份 guid。
pub(crate) fn url_identity_guid(raw_url: &str) -> String {
    synthetic("url", &super::url_norm::normalize_url(raw_url))
}

/// 有源日期且无 id/URL 时的 title+date 身份 guid。
pub(crate) fn title_date_identity_guid(title: &str, published_at: &str) -> String {
    synthetic("title-date", &format!("{title}\u{1f}{published_at}"))
}

pub(crate) fn is_synthetic_guid(guid: &str) -> bool {
    guid.starts_with(SYNTHETIC_GUID_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_keys_are_deterministic_and_source_sensitive() {
        assert_eq!(
            content_identity_guid("t", Some("<p>a</p>"), "a"),
            content_identity_guid("t", Some("<p>a</p>"), "a")
        );
        assert_ne!(
            content_identity_guid("t", Some("<p>a</p>"), "a"),
            content_identity_guid("t", Some("<p>b</p>"), "b")
        );
        assert!(is_synthetic_guid(&content_identity_guid(
            "t",
            Some("<p>a</p>"),
            "a"
        )));
        assert!(!is_synthetic_guid("real-feed-id"));
    }
}

//! 直连条目身份键（guid）的确定性构造。
//!
//! 真实 GUID 保留精确身份；无真实 GUID 时，解析层把规范 URL、标题与 clamp 前的
//! 选定源日期编码为完整观察 token。空正文只能用这些观察线索区分候选。
//! DB 的非空规范内容键不消费观察 token：日期补正/撤回及正文清理不会换身份。
//! None 明确编码，抓取时间/首次见到时间/钳制后的 now 不得进入观察 token。

use sha2::{Digest, Sha256};

/// 合成 guid 前缀。控制字符不会出现在正常 feed id 中。
pub(crate) const SYNTHETIC_GUID_PREFIX: &str = "\u{1}fluxreader:identity\u{1}";

/// Protocol entry keys share the GUID column but cannot collide with valid XML feed IDs.
/// This is a namespace for one existing entity, not an alias table.
pub fn remote_identity_guid(remote_id: &str) -> String {
    format!("\u{1}fluxreader:remote\u{1}{remote_id}")
}

/// 无真实 GUID 的完整观察；不是正文等价证明。
/// source_date 必须是 published.or(updated) 在钳制前的规范 UTC 值，缺失保持 None。
pub(crate) fn weak_observation_guid(
    raw_url: Option<&str>,
    title: &str,
    source_date: Option<&str>,
) -> String {
    let url = raw_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(super::url_norm::normalize_url);
    let mut hash = Sha256::new();
    hash.update(b"fluxreader-weak-observation-v1");
    for field in [url.as_deref(), Some(title), source_date] {
        match field {
            Some(value) => {
                hash.update([1]);
                hash.update((value.len() as u64).to_le_bytes());
                hash.update(value.as_bytes());
            }
            None => hash.update([0]),
        }
    }
    format!("{SYNTHETIC_GUID_PREFIX}observation:{:x}", hash.finalize())
}

pub(crate) fn is_synthetic_guid(guid: &str) -> bool {
    guid.starts_with(SYNTHETIC_GUID_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_repeatable_and_preserve_all_candidate_fields() {
        let a = weak_observation_guid(
            Some("https://example.com/a"),
            "title",
            Some("2000-01-01T00:00:00+00:00"),
        );
        assert_eq!(
            a,
            weak_observation_guid(
                Some("https://example.com/a"),
                "title",
                Some("2000-01-01T00:00:00+00:00")
            )
        );
        for b in [
            weak_observation_guid(
                Some("https://example.com/a"),
                "title",
                Some("2000-01-02T00:00:00+00:00"),
            ),
            weak_observation_guid(
                Some("https://example.com/b"),
                "title",
                Some("2000-01-01T00:00:00+00:00"),
            ),
            weak_observation_guid(
                Some("https://example.com/a"),
                "other",
                Some("2000-01-01T00:00:00+00:00"),
            ),
            weak_observation_guid(None, "title", Some("2000-01-01T00:00:00+00:00")),
            weak_observation_guid(Some("https://example.com/a"), "title", None),
        ] {
            assert_ne!(a, b);
        }
        assert!(is_synthetic_guid(&a));
        assert!(!is_synthetic_guid("real-feed-id"));
        assert!(!is_synthetic_guid(&remote_identity_guid("17")));
    }

    #[test]
    fn observation_encoding_distinguishes_absence_and_field_boundaries() {
        assert_ne!(
            weak_observation_guid(None, "ab", Some("c")),
            weak_observation_guid(None, "a", Some("bc"))
        );
        assert_ne!(
            weak_observation_guid(None, "title", None),
            weak_observation_guid(None, "title", Some(""))
        );
        assert_eq!(
            weak_observation_guid(Some("  "), "title", None),
            weak_observation_guid(None, "title", None)
        );
    }
}

/// 明确归属的营销/统计参数（utm 系 + 各家点击 ID/分享标识）：名字自带厂商
/// 归属，剥掉后不影响定位同一篇文章。通用名字（t/s/ref/group_id 等）可能是
/// 业务参数，一律保留——只有 X 状态页例外，见 [`X_STATUS_TRACKING_PARAMS`]。
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "utm_name",
    "utm_cid",
    "utm_reader",
    "utm_social",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid", // Google Ads
    "fbclid",
    "fb_action_ids",
    "fb_action_types",
    "fb_source", // Facebook
    "igshid",
    "igsh",   // Instagram
    "twclid", // X/Twitter 点击 ID
    "mc_cid",
    "mc_eid", // Mailchimp
];

/// 仅在 X/Twitter 状态页剥除的已知跟踪参数：名字太通用（论坛主题号、会话、
/// 引荐来源），站外可能是业务参数，只对 [`is_x_status_url`] 成立的确切状态页生效。
const X_STATUS_TRACKING_PARAMS: &[&str] = &["t", "s", "ref_src", "ref_url"];

/// 规范化算法版本：规则语义变化必须递增。存量 url_norm 按此版本重建——
/// settings.url_norm_backfill_version 缺失或不等即从原始 url 重算
/// （见 db/migrations.rs ensure_url_norm_backfill）。
pub(crate) const NORM_VERSION: &str = "2";

/// X/Twitter 状态页判定（**必须用未归一的原始 URL 调用**——显式非默认端口与
/// scheme 都是身份信号，先 set_scheme 会把 https:80 归一掉）：http(s)、无显式
/// 非默认端口、host 恰为 twitter.com / x.com（含标准 www./m./mobile. 子域）、
/// 路径严格为 /<user>/status/<纯数字 id>（不滤空段：/u//status/123 等非规范
/// 形态不是状态页）。伪域名（x.com.evil、evil.x.com）、/i/web/status、
/// /user/status/123/photo/1 与任意其它站点都不套用。
fn is_x_status_url(u: &url::Url) -> bool {
    if !matches!(u.scheme(), "http" | "https") || u.port().is_some() {
        return false;
    }
    let Some(host) = u.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    let host_ok = ["twitter.com", "x.com"].iter().any(|base| {
        host == *base
            || host == format!("www.{base}")
            || host == format!("m.{base}")
            || host == format!("mobile.{base}")
    });
    if !host_ok {
        return false;
    }
    let segs: Vec<&str> = u
        .path()
        .strip_prefix('/')
        .unwrap_or_default()
        .split('/')
        .collect();
    segs.len() == 3
        && !segs[0].is_empty()
        && segs[1] == "status"
        && !segs[2].is_empty()
        && segs[2].bytes().all(|b| b.is_ascii_digit())
}

/// URL 规范化为去重匹配键：同文不同饰（明确归属的跟踪参数/协议/www./m./
/// 尾斜杠/AMP）归一为一个键。失败（非 URL 形态）返回原串小写——匹配键退化
/// 但可用。规则从宽到严排序：只做「无损压缩」，绝不合并可能不同的文章
/// （通用参数是否剥除按 X 状态页例外保守区分）。
/// Note: 算法语义变化必须递增 [`NORM_VERSION`]，存量键随版本重建 —
/// 见 .agents/notes/implemented/bug-fix/2026-10-08-保守URL匹配与重建标记.md
pub fn normalize_url(url: &str) -> String {
    let Ok(mut u) = url::Url::parse(url.trim()) else {
        return url.trim().to_lowercase();
    };
    // X 状态页例外用**原始 URL**判定：必须早于 set_scheme/host 归一
    // （https:80 → http 后端口会被当默认值清掉，先判才不会绕过端口约束）
    let x_status = is_x_status_url(&u);
    // https 统一（http 降级为同一篇；其他 scheme 保留原样区分）
    if u.scheme() == "https" {
        let _ = u.set_scheme("http");
    }
    // 规整 host：www./m./mobile. 前缀剥掉（多数站点移动/桌面同文）
    if let Some(host) = u.host_str() {
        let trimmed = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("m."))
            .or_else(|| host.strip_prefix("mobile."));
        if let Some(t) = trimmed {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let _ = u.set_host(Some(&format!("{t}{port}")));
        }
    }
    // 跟踪参数剥离：明确归属的全局剥；通用名只在 X 状态页剥
    let filtered: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !TRACKING_PARAMS.contains(&k.as_str())
                && !(x_status && X_STATUS_TRACKING_PARAMS.contains(&k.as_str()))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if filtered.is_empty() {
        u.set_query(None);
    } else {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &filtered {
            q.append_pair(k, v);
        }
        u.set_query(Some(&q.finish()));
    }
    // 尾斜杠归一（/a/ 与 /a 同文）；AMP 页归一（/amp/x → /x）
    let mut path = u.path().trim_end_matches('/').to_string();
    if let Some(rest) = path.strip_prefix("/amp") {
        if rest.is_empty() || rest.starts_with('/') {
            path = rest.to_string();
        }
    }
    u.set_path(&path);
    // fragment 无定位意义（纯锚点），丢弃
    u.set_fragment(None);
    u.to_string()
}

/// 旧算法（v1，通用参数全局剥除时代）规范化快照——**只有一处用途**：匹配/回收
/// v19 迁入 feed_tombstones_legacy_v1 的旧退订墓碑键。旧键的原始 URL 已丢，
/// 只有复刻同一算法才能把「同一订阅」重新对上，保住用户删除意图不被复活。
///
/// 严禁用于文章/订阅身份匹配或写新键：通用参数收敛（t/s/ref 等只在 X 状态页剥）
/// 是 F12 的修复方向，把它重新用于普通匹配等于回退审计缺陷。算法随 v1 发布冻结，
/// 不再跟随 [`normalize_url`] 变化。
pub(crate) fn legacy_v1_normalize(url: &str) -> String {
    /// v1 时代的完整跟踪参数清单（通用名字也在内，这是与新算法的关键差异）。
    const V1_TRACKING_PARAMS: &[&str] = &[
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "utm_id",
        "utm_name",
        "utm_cid",
        "utm_reader",
        "utm_social",
        "gclid",
        "gclsrc",
        "dclid",
        "gbraid",
        "wbraid", // Google Ads
        "fbclid",
        "fb_action_ids",
        "fb_action_types",
        "fb_source", // Facebook
        "igshid",
        "igsh", // Instagram
        "twclid",
        "t",
        "s", // X/Twitter（t/s 短链跳转带参）
        "mc_cid",
        "mc_eid", // Mailchimp
        "ref",
        "ref_src",
        "ref_url",
        "referrer", // 引荐来源
        "spm_id",
        "scm",
        "share_token",
        "nsfrom",
        "nstoken", // 国内生态（掘金/微信/知乎）
        "share_source",
        "tt_from",
        "group_id",
        "web_chapter_id",
    ];
    let Ok(mut u) = url::Url::parse(url.trim()) else {
        return url.trim().to_lowercase();
    };
    if u.scheme() == "https" {
        let _ = u.set_scheme("http");
    }
    if let Some(host) = u.host_str() {
        let trimmed = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("m."))
            .or_else(|| host.strip_prefix("mobile."));
        if let Some(t) = trimmed {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            let _ = u.set_host(Some(&format!("{t}{port}")));
        }
    }
    let filtered: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !V1_TRACKING_PARAMS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if filtered.is_empty() {
        u.set_query(None);
    } else {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &filtered {
            q.append_pair(k, v);
        }
        u.set_query(Some(&q.finish()));
    }
    let mut path = u.path().trim_end_matches('/').to_string();
    if let Some(rest) = path.strip_prefix("/amp") {
        if rest.is_empty() || rest.starts_with('/') {
            path = rest.to_string();
        }
    }
    u.set_path(&path);
    u.set_fragment(None);
    u.to_string()
}

/* ============================================================
OPT-008A（审计 F12）单元矩阵：通用参数收敛 + X 状态页例外。
先行为反例（旧实现在业务参数上错误合并/伪域名上错误合并），再实现。
============================================================ */
#[cfg(test)]
mod tests {
    use super::normalize_url;

    /// 普通站点上的业务参数（论坛主题 t、会话/章节 s、业务 ID）必须各自区分；
    /// 通用引荐名 ref/referrer 保留，不得并入无参形态。
    #[test]
    fn business_params_stay_distinct_on_plain_sites() {
        let n = normalize_url;
        // F12 原始反例：论坛 /viewtopic.php?t=123 与 ?t=456 是不同主题
        assert_ne!(
            n("https://forum.example/viewtopic.php?t=123"),
            n("https://forum.example/viewtopic.php?t=456")
        );
        assert_ne!(
            n("https://forum.example/viewtopic.php?s=foo"),
            n("https://forum.example/viewtopic.php?s=bar")
        );
        assert_ne!(
            n("https://novel.example/read?group_id=1"),
            n("https://novel.example/read?group_id=2")
        );
        assert_ne!(
            n("https://novel.example/read?web_chapter_id=1"),
            n("https://novel.example/read?web_chapter_id=2")
        );
        assert_ne!(
            n("https://blog.example/post?ref=first"),
            n("https://blog.example/post?ref=second")
        );
        assert_ne!(
            n("https://blog.example/post?ref=first"),
            n("https://blog.example/post")
        );
        assert_ne!(
            n("https://blog.example/post?referrer=first"),
            n("https://blog.example/post")
        );
        // 跨域与显式端口是身份信号（既有语义防退化）
        assert_ne!(n("https://a.example/p"), n("https://b.example/p"));
        assert_ne!(n("https://a.example:8443/p"), n("https://a.example/p"));
    }

    /// 明确归属的营销/点击参数变体照旧合并（防收敛过度把真去重一并关掉）。
    #[test]
    fn attributed_marketing_params_are_still_stripped() {
        let n = normalize_url;
        assert_eq!(
            n("https://a.example/p?utm_source=x&utm_medium=rss"),
            n("http://a.example/p")
        );
        assert_eq!(n("https://a.example/p?fbclid=abc"), n("http://a.example/p"));
        assert_eq!(
            n("https://a.example/p?gclid=abc&mc_cid=1&mc_eid=2"),
            n("http://a.example/p")
        );
        assert_ne!(n("https://a.example/p?id=1"), n("https://a.example/p?id=2"));
    }

    /// X 状态页例外：仅确切 twitter.com/x.com（标准 www/m/mobile 子域）+ 纯数字
    /// status 路径才剥已知跟踪参数；非状态路径、非纯数字 id、子路径、显式非默认
    /// 端口都不套用。
    #[test]
    fn x_status_links_dedupe_known_tracking_variants() {
        let n = normalize_url;
        let bare = n("https://x.com/user/status/123");
        assert_eq!(
            n("https://x.com/user/status/123?s=20&t=abc&ref_src=twsrc%5Etfw"),
            bare
        );
        assert_eq!(
            n("https://www.x.com/user/status/123?s=20"),
            n("http://x.com/user/status/123")
        );
        assert_eq!(
            n("https://m.x.com/user/status/123?t=abc"),
            n("http://x.com/user/status/123")
        );
        assert_eq!(
            n("https://mobile.twitter.com/user/status/123?ref_url=https%3A%2F%2Ft.co%2Fx"),
            n("http://twitter.com/user/status/123")
        );
        assert_ne!(n("https://x.com/home?s=20"), n("https://x.com/home"));
        assert_ne!(
            n("https://x.com/user/status/abc?s=20"),
            n("https://x.com/user/status/abc")
        );
        assert_ne!(
            n("https://x.com/user/status/123/photo/1?s=20"),
            n("https://x.com/user/status/123/photo/1")
        );
        assert_ne!(
            n("https://x.com:8443/user/status/123?s=20"),
            n("https://x.com:8443/user/status/123")
        );
    }

    /// 伪域名（伪后缀/深子域）不得套 X 状态页例外。
    #[test]
    fn fake_x_domains_do_not_get_status_exception() {
        for host in [
            "x.com.evil",
            "twitter.com.evil",
            "evil.x.com",
            "x.com.example.com",
        ] {
            let bare = normalize_url(&format!("https://{host}/u/status/123"));
            let dressed = normalize_url(&format!("https://{host}/u/status/123?s=20&t=abc"));
            assert_ne!(bare, dressed, "{host} 不得套 X 状态页例外");
        }
    }

    /// R1 P2-2：状态路径必须严格是 /<user>/status/<id>——内部空段（/u//status/123、
    /// /u/status//123、//status/123）不是规范状态页，不得套例外把不同 t/s 抹成同键。
    #[test]
    fn x_status_exception_rejects_non_canonical_paths() {
        let n = normalize_url;
        assert_ne!(
            n("https://x.com/u//status/123?s=20"),
            n("https://x.com/u//status/123"),
            "内部空段不得套用 X 状态页例外"
        );
        assert_ne!(
            n("https://x.com/u/status//123?s=20"),
            n("https://x.com/u/status//123"),
            "id 前空段不得套用 X 状态页例外"
        );
        assert_ne!(
            n("https://x.com//status/123?s=20"),
            n("https://x.com//status/123"),
            "空 user 段不得套用 X 状态页例外"
        );
    }

    /// R1 P2-3：例外判定必须用**原始 URL**——https 的显式非默认端口（:80）是
    /// 身份信号，不得因先 set_scheme 把端口归一掉而放宽成状态页。
    #[test]
    fn x_status_exception_checks_port_before_scheme_normalization() {
        let n = normalize_url;
        assert_ne!(
            n("https://x.com:80/u/status/123?s=20&t=abc"),
            n("https://x.com:80/u/status/123"),
            "原始显式非默认端口（https:80）不得套用 X 状态页例外"
        );
    }

    /// R1：legacy_v1_normalize 冻结旧算法行为（旧退订墓碑键就是它的输出），
    /// 只用于墓碑匹配——对新算法同样会收敛的 URL 两者刻意不同，证明不可混用。
    #[test]
    fn legacy_v1_normalize_freezes_old_algorithm_for_tombstones_only() {
        use super::legacy_v1_normalize;
        // 旧算法全局剥 t/s/ref：带业务参数的 URL 与裸 URL 同键（旧墓碑的形态）
        assert_eq!(
            legacy_v1_normalize("https://forum.example/viewtopic.php?t=123&utm_source=x"),
            legacy_v1_normalize("http://forum.example/viewtopic.php")
        );
        // 旧键 ≠ 新键（F12 收敛后两命名空间必须分开，不得混用旧算法做身份匹配）
        assert_ne!(
            legacy_v1_normalize("https://forum.example/viewtopic.php?t=123"),
            normalize_url("https://forum.example/viewtopic.php?t=123")
        );
        // 其余旧策略（https/www./尾斜杠/fragment）冻结不变
        assert_eq!(
            legacy_v1_normalize("https://www.example.com/a/#frag"),
            legacy_v1_normalize("http://example.com/a")
        );
    }
}

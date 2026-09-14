/// 已知跟踪/统计参数（utm 系 + 各家统计 SDK）。剥掉后不影响定位同一篇文章。
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

/// URL 规范化为去重匹配键：同文不同饰（跟踪参数/协议/www./m./尾斜杠/AMP）
/// 归一为一个键。失败（非 URL 形态）返回原串小写——匹配键退化但可用。
/// 规则从宽到严排序：只做「无损压缩」，绝不合并可能不同的文章。
pub fn normalize_url(url: &str) -> String {
    let Ok(mut u) = url::Url::parse(url.trim()) else {
        return url.trim().to_lowercase();
    };
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
    // 跟踪参数剥离
    let filtered: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !TRACKING_PARAMS.contains(&k.as_str())
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

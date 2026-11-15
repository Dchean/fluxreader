//! favicon 自动发现：站点 `<link rel~=icon>` 优先，退回 `<origin>/favicon.ico`。

use super::http::{decode_body_bytes, read_body_capped, MAX_BODY_BYTES};
use super::parse::resolve_url;
use reqwest::Client;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::Duration;
use url::{Host, Url};

/// favicon 发现负缓存（feed_id 集合）：发现失败的源本进程生命周期内不重试。
/// 成功的 icon 已写库（feeds.favicon_url），不依赖此表。
pub(super) static FAVICON_TRIED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<i64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// 重定向跟随上限（P2-2）：reqwest 默认策略是 10 跳（Policy::default =
/// limited(10)），favicon 不需要给任何一方更长的链式探测通道；
/// 正常站点两三跳已足够。
const MAX_REDIRECTS: u32 = 5;

/// favicon 专用客户端（P2-2）：**禁自动重定向**——默认策略会在守卫之外
/// 直连任意 Location（含内网地址），重定向必须逐跳过 scheme/私网守卫后
/// 手动跟随。其余语义与 build_client 相同（系统代理按环境变量、rustls）；
/// UA 与超时仍按请求设置（首页 5s / 探活 3s）。
static FAVICON_CLIENT: std::sync::LazyLock<Client> = std::sync::LazyLock::new(|| {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build reqwest client")
});

/// 单跳回复的最小投影：手动重定向循环需要的字段 + 首页已收体。
/// 纯数据结构便于以 mock 发送器单测循环语义（不依赖 reqwest::Response）。
#[derive(Debug, PartialEq)]
struct ProbeReply {
    status: u16,
    location: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// favicon 自动发现：先取站点 HTML 解析 `<link rel~=icon>`（svg/png 优先），
/// 失败或无 link 则退回 `<origin>/favicon.ico`。返回通过 GET 探活
/// 确认可达（200-299 且非 HTML）的图标 URL；任何失败返回 None（调用方静默）。
/// UA 用浏览器伪装——部分站点对非浏览器 UA 直接 403。
/// 超时收紧（首页 5s / 探活 3s）：favicon 是锦上添花，不值得长等——
/// 探测发生在刷新信号量内，超时越长并发刷新被拖得越久。
/// （P2-2）仅探测 http/https 目标；域名解析后逐地址拒绝私网/保留段；
/// 重定向手动逐跳守卫，上限 MAX_REDIRECTS。探测全程不带 Referer。
pub(super) async fn discover_favicon(_client: &Client, site_url: &str) -> Option<String> {
    // 共享抓取 client 不再参与探测：favicon 改用禁自动重定向的专用客户端
    // （FAVICON_CLIENT）。形参仅为 staged.rs 调用点兼容保留。
    let origin = Url::parse(site_url).ok()?;
    // P2-2：favicon 探测只打 http/https；file/ftp/data 等一律拒绝（并入负缓存）。
    if !matches!(origin.scheme(), "http" | "https") {
        return None;
    }
    // base 保留显式端口：探测必须打回源站同一 origin——丢端口会把带端口的
    // 源探到默认端口的另一个服务上。
    let mut base = format!("{}://{}", origin.scheme(), origin.host_str()?);
    if let Some(port) = origin.port() {
        base = format!("{base}:{port}");
    }
    let base_url = Url::parse(&base).ok()?;

    // ① 站点首页 HTML 里的 <link rel="icon">（rel 包含 icon 的各种变体）。
    // 首页读取与 feed 主管线共用 16 MiB 界——恶意/异常站点可回无界大响应，
    // favicon 是锦上添花，超限等同读取失败（返回 None，走负缓存）。
    // 解码按 Content-Type charset（GBK 站点的 rel 属性也要正确识别）。
    let homepage = guarded_probe(base_url, |u: Url| {
        fetch_reply(u, Duration::from_secs(5), true)
    })
    .await?;
    let html = decode_body_bytes(&homepage.body, homepage.content_type.as_deref());
    let link_icon = extract_icon_link(&html).map(|href| resolve_url(&href, &base));

    // ② 兜底 /favicon.ico（link icon 可能指向任意第三方地址，候选逐个过守卫）
    let fallback = format!("{base}/favicon.ico");
    let candidates = link_icon.into_iter().chain(std::iter::once(fallback));
    for url in candidates {
        // 解析失败的候选跳过（与旧行为一致：该候选落空，继续下一个）
        let Ok(target) = Url::parse(&url) else {
            continue;
        };
        let reply = guarded_probe(target, |u: Url| {
            fetch_reply(u, Duration::from_secs(3), false)
        })
        .await;
        if let Some(reply) = reply {
            let ct = reply
                .content_type
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase();
            if (200..300).contains(&reply.status) && !ct.starts_with("text/html") {
                return Some(url);
            }
        }
    }
    None
}

/// 受守卫的探测取回（P2-2）：目标先过 scheme/私网守卫（域名解析后逐地址判定）
/// 再发请求；重定向逐跳 join Location、重守卫、计数，超过 MAX_REDIRECTS
/// 放弃。发送器返回 None 视为传输失败，整体并入 None（负缓存）。
/// 3xx 无 Location / 空 Location 同样放弃——旧策略下这类响应也只是探活失败。
async fn guarded_probe<F, Fut>(start: Url, mut send: F) -> Option<ProbeReply>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Option<ProbeReply>>,
{
    let mut current = start;
    let mut hops = 0u32;
    loop {
        if !target_allowed(&current).await {
            return None;
        }
        let reply = send(current.clone()).await?;
        if !is_redirect(reply.status) {
            return Some(reply);
        }
        if hops >= MAX_REDIRECTS {
            return None;
        }
        hops += 1;
        let location = reply.location.as_deref().filter(|l| !l.is_empty())?;
        current = current.join(location).ok()?;
    }
}

/// 手动重定向循环中的一跳：GET + 字段投影。`read_body` 仅首页为真
/// （探活只看状态与 Content-Type，不下载图标体）；重定向响应同样不收体。
/// 不设 Referer——no-referrer 探活语义保持（reqwest 仅在自动跟随重定向
/// 时自动补 Referer，手动逐跳天然没有）。
async fn fetch_reply(url: Url, timeout: Duration, read_body: bool) -> Option<ProbeReply> {
    let resp = FAVICON_CLIENT
        .get(url)
        .header("User-Agent", BROWSER_UA)
        .timeout(timeout)
        .send()
        .await
        .ok()?;
    let status = resp.status().as_u16();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = if read_body && !is_redirect(status) {
        read_body_capped(resp, MAX_BODY_BYTES).await.ok()?
    } else {
        Vec::new()
    };
    Some(ProbeReply {
        status,
        location,
        content_type,
        body,
    })
}

/// 与 reqwest 0.12 实际跟随的重定向状态集合一致（tower-http
/// follow_redirect：301/302/303/307/308）。
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// 目标是否允许探测（P2-2）：scheme 白名单 + 解析后逐地址判定。
/// 域名经系统解析器取全部地址，任一地址落入拒绝集合即整体拒绝
/// （A/AAAA 混合记录按最坏情况处置）；解析失败视同失败（负缓存）。
async fn target_allowed(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => !ip_disallowed(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !ip_disallowed(IpAddr::V6(ip)),
        Some(Host::Domain(host)) => resolve_allowed(host, url.port_or_known_default()).await,
        None => false,
    }
}

/// 域名解析与逐地址判定。std 解析器放 spawn_blocking（tokio 未启用 net
/// feature；阻塞的 getaddrinfo 不能占 async 线程）。reqwest 内部解析同为
/// 系统解析器，口径一致。
async fn resolve_allowed(host: &str, port: Option<u16>) -> bool {
    let port = port.unwrap_or(80);
    let host = host.to_string();
    let Ok(resolved) = tokio::task::spawn_blocking(move || {
        (host.as_str(), port)
            .to_socket_addrs()
            .map(|it| it.collect::<Vec<_>>())
    })
    .await
    else {
        // join 失败（运行时关闭）：无法证明目标安全，保守拒绝
        return false;
    };
    match resolved {
        Ok(addrs) => !addrs.is_empty() && addrs.iter().all(|a| !ip_disallowed(a.ip())),
        // 解析失败：目标不可达，探测本就会失败
        Err(_) => false,
    }
}

/// 单地址判定（P2-2）：loopback / link-local / 私网 / 保留段全部拒绝。
/// v6 用白名单形态——全球单播 2000::/3 是当前唯一可公网路由的 v6 空间，
/// 未登记/未来特殊段自动落入拒绝；内嵌 IPv4 形态（映射、NAT64、6to4）
/// 先还原内嵌地址再按 v4 判定，防「v6 皮肤的内网地址」。
fn ip_disallowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_disallowed(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                // ::ffff:0:0/96 IPv4 映射皮肤
                return v4_disallowed(v4);
            }
            let o = v6.octets();
            // 64:ff9b::/96 NAT64（RFC 6052）：真实目标由网关翻译到内嵌 IPv4
            if o[0..6] == [0x00, 0x64, 0xff, 0x9b, 0x00, 0x00] {
                return v4_disallowed(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
            }
            // 2002::/16 6to4（已废弃）：内嵌 IPv4 判定
            if o[0] == 0x20 && o[1] == 0x02 {
                return v4_disallowed(Ipv4Addr::new(o[2], o[3], o[4], o[5]));
            }
            // 2000::/3 全球单播白名单（剔除 2001:db8::/32 文档段）；其余一律拒绝
            !((o[0] & 0xe0) == 0x20 && o[0..4] != [0x20, 0x01, 0x0d, 0xb8])
        }
    }
}

/// IPv4 拒绝集合：特殊用途段逐一枚举（std 的 is_global 尚未稳定，段位
/// 判定手写；is_private/is_loopback 等稳定判定直接复用）。
fn v4_disallowed(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()                                   // 10/8、172.16/12、192.168/16
        || v4.is_loopback()                           // 127/8
        || v4.is_link_local()                         // 169.254/16
        || v4.is_multicast()                          // 224/4
        || v4.is_documentation()                      // 192.0.2/24、198.51.100/24、203.0.113/24
        || o[0] == 0                                  // 0.0.0.0/8 本网络段
        || (o[0] == 100 && (o[1] & 0xc0) == 64)       // 100.64/10 CGNAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)    // 192.0.0/24 IETF 协议分配
        || (o[0] == 192 && o[1] == 88 && o[2] == 99)  // 192.88.99/24 6to4 中继（已废弃）
        || (o[0] == 198 && (o[1] & 0xfe) == 18)       // 198.18/15 基准测试
        || o[0] >= 240 // 240/4 保留段（含 255.255.255.255）
}

/// 从 HTML 提取 `<link rel="…icon…">` 的 href（首个命中）。
/// rel 判定在小写副本上（属性名/值大小写容错）；href 从**原文**提取
/// ——URL path 大小写敏感，小写化会拿到错误地址。
fn extract_icon_link(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find("<link ") {
        let start = search + rel;
        let end = match lower[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let tag_lower = &lower[start..end];
        if rel_is_icon(tag_lower) {
            if let Some(href) = extract_html_attr(&html[start..end], "href") {
                if !href.is_empty() {
                    return Some(href);
                }
            }
        }
        search = end + 1;
    }
    None
}

/// rel 属性值是否为 icon 类。词级判定：rel="shortcut icon" 按空白拆词后
/// `icon` 完整相等，或 image/x-icon / mask-icon 等 `-icon` / `/icon` 后缀词
/// ——排除 `iconx` 子串误中。
fn rel_is_icon(tag_lower: &str) -> bool {
    let rel_val = extract_html_attr(tag_lower, "rel").unwrap_or_default();
    rel_val
        .split_whitespace()
        .map(|w| w.trim_matches('"').trim_matches('\''))
        .any(|w| w == "icon" || w.ends_with("/icon") || w.ends_with("-icon"))
}

/// 从标签原文提取 `name="…"` / `name='…'` / `name=裸值` 的值。
/// 属性名匹配容错大小写（Link HREF= 合法）；值保持原文（URL 大小写敏感）。
fn extract_html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pat = format!("{name}=");
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find(&pat) {
        let idx = search + rel;
        // 独立属性名：前一字符不是 [A-Za-z0-9-_.]
        let prev_ok = idx == 0
            || lower
                .as_bytes()
                .get(idx.wrapping_sub(1))
                .map(|c| !c.is_ascii_alphanumeric() && *c != b'-' && *c != b'_' && *c != b'.')
                .unwrap_or(true);
        if prev_ok {
            let vstart = idx + pat.len();
            let rest = tag.get(vstart..)?;
            let quote = rest.chars().next()?;
            return match quote {
                '"' | '\'' => {
                    let end = rest[1..].find(quote)?;
                    Some(rest[1..1 + end].to_string())
                }
                _ => Some(rest.split_whitespace().next()?.to_string()),
            };
        }
        search = idx + pat.len();
    }
    None
}

/// 浏览器伪装 UA（favicon 发现专用）
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[cfg(test)]
mod favicon_tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// P2-2 正控：loopback mock 已备好**合法图标**，私网守卫必须让发现
    /// 先于任何 I/O 失败（请求计数为 0）。
    /// 取代原「超大首页并入失败」用例——loopback 目标在守卫处即被拒，
    /// 16 MiB 收体界本身由 http.rs 的 read_body_capped 两形态测试承担。
    #[tokio::test]
    async fn private_target_is_never_probed() {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_hits = hits.clone();
        std::thread::spawn(move || {
            while let Some(Ok(mut stream)) = listener.incoming().next() {
                server_hits.fetch_add(1, Ordering::SeqCst);
                // 就算真连上：返回合法 icon（200 + image/png），证明 None 只能来自守卫
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\n\r\nPNG!",
                );
                let _ = stream.flush();
            }
        });
        let client = super::super::build_client(10);
        let site = format!("http://127.0.0.1:{port}/feed.xml");
        assert_eq!(
            discover_favicon(&client, &site).await,
            None,
            "私网/loopback 目标必须被守卫拒绝"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "守卫必须先于任何 I/O（请求计数为 0）"
        );
    }

    /// mock 回复构造：mock 发送器单测用（无 reqwest::Response）。
    fn mock_reply(
        status: u16,
        location: Option<&str>,
        content_type: Option<&str>,
    ) -> Option<ProbeReply> {
        Some(ProbeReply {
            status,
            location: location.map(str::to_owned),
            content_type: content_type.map(str::to_owned),
            body: Vec::new(),
        })
    }

    /// P2-2 scheme 白名单：非 http/https 目标一律拒绝（任何 I/O 之前）；
    /// 正控 https + 公网字面 IP 不得误伤。
    #[tokio::test]
    async fn non_http_schemes_are_rejected() {
        for bad in [
            "ftp://8.8.8.8/i.png",
            "file:///C:/x/i.png",
            "javascript:alert(1)",
        ] {
            let url = Url::parse(bad).unwrap();
            assert!(!target_allowed(&url).await, "{bad} 必须被 scheme 守卫拒绝");
        }
        // 正控：字面公网 IP 不经 DNS，直接放行
        assert!(target_allowed(&Url::parse("https://8.8.8.8/i.png").unwrap()).await);
        // discover 级：scheme 不合法的 site_url 直接 None
        let client = super::super::build_client(10);
        assert_eq!(
            discover_favicon(&client, "ftp://8.8.8.8/feed.xml").await,
            None,
            "非 http/https 站点不得进入探测"
        );
    }

    /// P2-2 逐地址判定：拒绝集合（v4 特殊段全枚举；v6 白名单外全拒）+
    /// 正控（公网地址不得误拒，含 v6 全球单播边界 3fff:ffff::1）。
    #[test]
    fn ip_disallowed_rejects_private_and_reserved() {
        let rejected = [
            "0.0.0.0",
            "0.1.2.3", // 0/8 本网络
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1", // RFC 1918
            "100.64.0.1",
            "100.127.255.255", // 100.64/10 CGNAT
            "127.0.0.1",       // 127/8 loopback
            "169.254.9.9",     // 169.254/16 link-local
            "192.0.0.1",
            "192.88.99.1",
            "192.0.2.9",
            "198.51.100.7",
            "203.0.113.9",
            "198.18.0.1",
            "198.19.255.255", // IETF 协议 / 6to4 中继 / TEST-NET / 基准
            "224.0.0.1",
            "239.1.2.3", // 224/4 组播
            "240.0.0.1",
            "255.255.255.255", // 240/4 保留段
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            "4000::1",          // v6 特殊段与白名单外
            "::ffff:127.0.0.1", // v4 映射皮肤
            "64:ff9b::10.0.0.1",
            "2002:a00:1::", // NAT64 / 6to4 内嵌私网
        ];
        for s in rejected {
            let ip: IpAddr = s.parse().unwrap();
            assert!(ip_disallowed(ip), "{s} 必须拒绝");
        }
        let allowed = [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",  // 172.16/12 之外
            "100.128.0.1", // CGNAT 之外
            "198.20.0.1",
            "203.0.114.1", // 相邻段不误伤
            "2606:4700::1111",
            "2001:4860:4860::8888",
            "2a00:1450:4001::1",
            "3fff:ffff::1",
            "::ffff:8.8.8.8", // 映射公网
            "64:ff9b::8.8.8.8",
            "2002:808:808::", // 内嵌公网 v4
        ];
        for s in allowed {
            let ip: IpAddr = s.parse().unwrap();
            assert!(!ip_disallowed(ip), "{s} 不得误拒");
        }
    }

    /// P2-2 解析后逐地址判定（实路径）：localhost 由系统解析到 loopback，
    /// 域名目标整体拒绝。解析失败同样返回 false，两种结局都判拒绝。
    #[tokio::test]
    async fn domain_resolving_to_loopback_is_rejected() {
        let url = Url::parse("http://localhost/favicon.ico").unwrap();
        assert!(
            !target_allowed(&url).await,
            "解析到私网地址的域名必须整体拒绝"
        );
    }

    /// P2-2 重定向手动跟随：Location join、逐跳守卫、终响应交付
    /// （mock 发送器，目标用公网字面 IP 免 DNS）。
    #[tokio::test]
    async fn redirect_chain_is_followed_within_guard() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sender_hits = hits.clone();
        let sender = move |u: Url| {
            let hits = sender_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                match u.path() {
                    "/a" => mock_reply(302, Some("/b"), None),
                    "/b" => mock_reply(302, Some("http://8.8.8.8/c"), None),
                    _ => mock_reply(200, None, Some("image/png")),
                }
            }
        };
        let reply = guarded_probe(Url::parse("http://8.8.8.8/a").unwrap(), sender)
            .await
            .expect("合法链必须交付终响应");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content_type.as_deref(), Some("image/png"));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            3,
            "首跳 + 两次重定向共三次请求"
        );
    }

    /// P2-2 跳数上限：永远 302 的目标在首跳 + MAX_REDIRECTS 次请求后放弃。
    #[tokio::test]
    async fn redirect_hop_cap_is_enforced() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sender_hits = hits.clone();
        let sender = move |u: Url| {
            let hits = sender_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                mock_reply(302, Some(u.as_str()), None)
            }
        };
        assert_eq!(
            guarded_probe(Url::parse("http://8.8.8.8/loop").unwrap(), sender).await,
            None
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            (MAX_REDIRECTS + 1) as usize,
            "上限 = 首跳 + MAX_REDIRECTS 次跟随"
        );
    }

    /// P2-2 重定向目标过守卫：Location 指向私网时不得发出第二个请求。
    #[tokio::test]
    async fn redirect_into_private_target_is_rejected() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sender_hits = hits.clone();
        let sender = move |_u: Url| {
            let hits = sender_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                mock_reply(302, Some("http://127.0.0.1/x"), None)
            }
        };
        assert_eq!(
            guarded_probe(Url::parse("http://8.8.8.8/a").unwrap(), sender).await,
            None
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "私网 Location 不得请求");
    }

    /// P2-2 重定向 scheme 白名单：Location 换成非 http(s) 时拒绝。
    #[tokio::test]
    async fn redirect_into_other_scheme_is_rejected() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sender_hits = hits.clone();
        let sender = move |_u: Url| {
            let hits = sender_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                mock_reply(302, Some("ftp://8.8.8.8/x"), None)
            }
        };
        assert_eq!(
            guarded_probe(Url::parse("http://8.8.8.8/a").unwrap(), sender).await,
            None
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "非 http(s) Location 不得请求"
        );
    }

    /// P2-2：3xx 无 Location 无法跟随——放弃（旧策略下同样只是探活失败）。
    #[tokio::test]
    async fn redirect_without_location_abandons() {
        let hits = Arc::new(AtomicUsize::new(0));
        let sender_hits = hits.clone();
        let sender = move |_u: Url| {
            let hits = sender_hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                mock_reply(302, None, None)
            }
        };
        assert_eq!(
            guarded_probe(Url::parse("http://8.8.8.8/a").unwrap(), sender).await,
            None
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn extract_icon_link_finds_variants() {
        // 常规写法
        let h1 = r#"<head><link rel="icon" href="/Icon.PNG"><title>x</title></head>"#;
        assert_eq!(
            extract_icon_link(h1).as_deref(),
            Some("/Icon.PNG"),
            "大小写保持原文"
        );
        // shortcut icon 变体 + 单引号
        let h2 = r#"<link rel='shortcut icon' href='/f.ico'>"#;
        assert_eq!(extract_icon_link(h2).as_deref(), Some("/f.ico"));
        // 属性顺序不固定（href 在前）
        let h3 = r#"<link href="https://cdn.example/a.svg" rel="icon" type="image/svg+xml">"#;
        assert_eq!(
            extract_icon_link(h3).as_deref(),
            Some("https://cdn.example/a.svg")
        );
        // 大写属性名 + 大小写混合 rel
        let h4 = r#"<LINK REL="Icon" HREF="https://x.example/i.png">"#;
        assert_eq!(
            extract_icon_link(h4).as_deref(),
            Some("https://x.example/i.png")
        );
        // 无 icon link → None
        assert_eq!(
            extract_icon_link(r#"<link rel="stylesheet" href="a.css">"#),
            None
        );
        // 非 icon 的 link 不误中（含 "icon" 子串的其他 rel）
        assert_eq!(extract_icon_link(r#"<link rel="iconx" href="a">"#), None);
    }

    #[test]
    fn extract_html_attr_all_quote_styles() {
        assert_eq!(
            extract_html_attr(r#"rel="icon" href="/a.png" "#, "href").as_deref(),
            Some("/a.png")
        );
        assert_eq!(
            extract_html_attr("href='/b.ico'", "href").as_deref(),
            Some("/b.ico")
        );
        assert_eq!(
            extract_html_attr("href=bare.ico ", "href").as_deref(),
            Some("bare.ico")
        );
        // 独立属性名判定（X-HREF 不得误中 href）
        assert_eq!(extract_html_attr("data-href='no'", "href"), None);
    }
}

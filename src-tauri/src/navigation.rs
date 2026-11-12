use tauri::{plugin::TauriPlugin, Manager, Runtime};
use url::Url;

fn same_origin(target: &Url, allowed: &Url) -> bool {
    target.username().is_empty()
        && target.password().is_none()
        && target.scheme() == allowed.scheme()
        && target.host_str() == allowed.host_str()
        && target.port_or_known_default() == allowed.port_or_known_default()
}

fn local_dev_origin(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        && url.username().is_empty()
        && url.password().is_none()
}

fn allows_navigation(target: &Url, app_origin: &Url, dev_origin: Option<&Url>) -> bool {
    same_origin(target, app_origin)
        || dev_origin
            .is_some_and(|allowed| local_dev_origin(allowed) && same_origin(target, allowed))
}

// Note: Windows 的 NavigationStarting 只约束主 frame；iframe 保留媒体域策略 — 见 .agents/notes/implemented/architecture/2026-11-05-主页面导航与外链边界.md
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("navigation-policy")
        .on_navigation(|webview, target| {
            let config = webview.app_handle().config();
            let https = config
                .app
                .windows
                .iter()
                .find(|window| window.label == webview.label())
                .is_some_and(|window| window.use_https_scheme);
            let app_origin = if cfg!(windows) || cfg!(target_os = "android") {
                if https {
                    "https://tauri.localhost"
                } else {
                    "http://tauri.localhost"
                }
            } else {
                "tauri://localhost"
            };
            let app_origin = Url::parse(app_origin).expect("fixed application origin");
            let dev_origin = tauri::is_dev()
                .then_some(config.build.dev_url.as_ref())
                .flatten();
            allows_navigation(target, &app_origin, dev_origin)
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_origins_reject_lookalikes_ports_credentials_and_schemes() {
        let app = Url::parse("http://tauri.localhost").unwrap();
        let dev = Url::parse("http://localhost:5173").unwrap();
        for target in [
            "http://tauri.localhost/",
            "http://tauri.localhost/index.html#reader",
            "http://tauri.localhost:80/path?query=1",
            "http://localhost:5173/reader",
        ] {
            assert!(
                allows_navigation(&Url::parse(target).unwrap(), &app, Some(&dev)),
                "{target}"
            );
        }
        for target in [
            "https://example.com/",
            "https://www.youtube.com/embed/allowed-only-as-iframe",
            "http://tauri.localhost.evil.invalid/",
            "http://tauri.localhost:5173/",
            "https://tauri.localhost/",
            "http://localhost:5174/",
            "http://127.0.0.1:5173/",
            "http://localhost.evil.invalid:5173/",
            "http://user@tauri.localhost/",
            "http://localhost:5173@evil.invalid/",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,hello",
            "about:blank",
        ] {
            assert!(
                !allows_navigation(&Url::parse(target).unwrap(), &app, Some(&dev)),
                "{target}"
            );
        }
        assert!(!allows_navigation(&dev, &app, None));
        let remote_dev = Url::parse("https://evil.invalid:5173").unwrap();
        assert!(!allows_navigation(&remote_dev, &app, Some(&remote_dev)));
        let ipv6 = Url::parse("http://[::1]:5173").unwrap();
        assert!(allows_navigation(&ipv6, &app, Some(&ipv6)));
    }
}

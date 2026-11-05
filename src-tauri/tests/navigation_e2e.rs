use app_lib::navigation;
use tauri::plugin::Plugin;
use url::Url;

#[test]
fn production_plugin_callback_uses_runtime_config() {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().build.dev_url = Some(Url::parse("http://localhost:5173").unwrap());
    let app = tauri::test::mock_builder().build(context).unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let webview = window.as_ref();
    let mut plugin = navigation::init();
    let app_origin = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    assert!(plugin.on_navigation(webview, &Url::parse(app_origin).unwrap()));
    assert_eq!(
        plugin.on_navigation(webview, &Url::parse("http://localhost:5173/path").unwrap()),
        tauri::is_dev()
    );
    for target in [
        "https://evil.invalid",
        "http://localhost:5174",
        "file:///C:/test",
        "https://www.youtube.com/embed/a",
    ] {
        assert!(!plugin.on_navigation(webview, &Url::parse(target).unwrap()));
    }
}

#[test]
fn production_plugin_callback_respects_https_window_origin() {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().app.windows = vec![tauri::utils::config::WindowConfig {
        label: "main".into(),
        use_https_scheme: true,
        ..Default::default()
    }];
    context.config_mut().build.dev_url = Some(Url::parse("http://localhost:5173").unwrap());
    let app = tauri::test::mock_builder().build(context).unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .use_https_scheme(true)
        .build()
        .unwrap();
    let mut plugin = navigation::init();
    if cfg!(windows) {
        assert!(plugin.on_navigation(
            window.as_ref(),
            &Url::parse("https://tauri.localhost/path").unwrap()
        ));
        assert!(!plugin.on_navigation(
            window.as_ref(),
            &Url::parse("http://tauri.localhost").unwrap()
        ));
    }
}

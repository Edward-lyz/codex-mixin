use super::*;
use crate::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn transport(client: Client, path: &std::path::Path) -> UpstreamAccess {
    let stored = StoredGatewayConfig {
        official_ech_proxy: true,
        ..StoredGatewayConfig::default()
    };
    save_stored_config_to_path(path, &stored).unwrap();
    let config = GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: Vec::new(),
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        official_ech_proxy: true,
        official_ech_fallback_reason: None,
        codex_auth_path: path.with_extension("auth.json"),
        gateway_api_key: None,
        gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
        accept_codex_oauth: true,
        official_selected_models: None,
        default_max_tokens: 1024,
        default_context_window: 4096,
        request_timeout: Duration::from_secs(1),
        thinking_mode: crate::config::ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: String::new(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };
    let mut upstream = UpstreamAccess::new(Arc::new(config), client);
    upstream.official_ech = Arc::new(crate::ech::OfficialEch::for_config(path.to_path_buf()));
    upstream
}

#[tokio::test]
async fn ech_failure_disables_and_sends_once_directly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let count = Arc::new(AtomicUsize::new(0));
    let requests = Arc::clone(&count);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(axum::routing::post(move |body: axum::body::Bytes| {
        let requests = Arc::clone(&requests);
        async move {
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["model"],
                "gpt-test"
            );
            requests.fetch_add(1, Ordering::SeqCst);
            "ok"
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // A bad inherited proxy proves fallback uses a direct client.
    let client = Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
        .build()
        .unwrap();
    let upstream = transport(client, &path);
    let url = reqwest::Url::parse(&format!("http://{address}/responses")).unwrap();
    for _ in 0..2 {
        let request = upstream
            .official_request(reqwest::Method::POST, url.clone())
            .await
            .unwrap();
        let request = body::prepare_json(request, json!({"model": "gpt-test"}))
            .await
            .unwrap();
        assert_eq!(
            upstream
                .send_official(request)
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(!upstream.official_ech_active());
    let saved = load_stored_config_from_path(&path).unwrap().unwrap();
    assert!(!saved.official_ech_proxy);
    assert!(
        saved
            .official_ech_fallback_reason
            .unwrap()
            .contains("ECH requires HTTPS")
    );
    // Disabling the official transport does not replace third-party transport.
    let request = upstream.request(reqwest::Method::POST, url).await.unwrap();
    assert!(upstream.send_official(request).await.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn sent_request_timeout_is_not_replayed_or_classified_as_ech_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let count = Arc::new(AtomicUsize::new(0));
    let requests = Arc::clone(&count);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(axum::routing::post(move |_: axum::body::Bytes| {
        let requests = Arc::clone(&requests);
        async move {
            requests.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(1)).await;
            "ok"
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Client::builder()
        .no_proxy()
        .resolve_to_addrs("chatgpt.com", &[address])
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let upstream = transport(client.clone(), &path);
    let request = client
        .post(format!("http://chatgpt.com:{}/responses", address.port()))
        .json(&json!({"model": "gpt-test"}));
    assert!(upstream.send_official(request).await.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(upstream.official_ech_active());
    assert!(
        load_stored_config_from_path(&path)
            .unwrap()
            .unwrap()
            .official_ech_proxy
    );
    server.abort();
}

#[tokio::test]
async fn failed_streaming_connection_disables_without_replaying_body() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = Client::builder()
        .no_proxy()
        .resolve_to_addrs("chatgpt.com", &[address])
        .build()
        .unwrap();
    let upstream = transport(client.clone(), &path);
    let request = body::prepare_json(
        client.post(format!("http://chatgpt.com:{}/responses", address.port())),
        json!({"model": "gpt-test"}),
    )
    .await
    .unwrap();
    let error = upstream.send_official(request).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("streaming request was not replayed")
    );
    assert!(!upstream.official_ech_active());
    assert!(
        !load_stored_config_from_path(&path)
            .unwrap()
            .unwrap()
            .official_ech_proxy
    );
}

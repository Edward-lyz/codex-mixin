use super::*;

fn env(pairs: &[(&str, &str)]) -> ProxyEnv {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    ProxyEnv::from_lookup(&move |name| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    })
}

fn system(http: Option<&str>, https: Option<&str>, bypass: &[&str]) -> SystemProxy {
    SystemProxy {
        http: http.map(str::to_owned),
        https: https.map(str::to_owned),
        bypass: bypass.iter().map(|entry| (*entry).to_owned()).collect(),
    }
}

#[test]
fn environment_scheme_variable_beats_all_proxy_and_system() {
    let selection = ProxySelection::with_system(
        env(&[
            ("HTTPS_PROXY", "http://env-https:1"),
            ("ALL_PROXY", "http://env-all:2"),
        ]),
        Some(system(Some("http://sys:3"), None, &[])),
    );
    assert_eq!(
        selection.select("https", "example.com", 443).as_deref(),
        Some("http://env-https:1")
    );
    assert_eq!(
        selection.select("http", "example.com", 80).as_deref(),
        Some("http://env-all:2")
    );
}

#[test]
fn system_proxy_is_used_when_environment_is_empty() {
    let selection = ProxySelection::with_system(
        ProxyEnv::default(),
        Some(system(
            Some("http://sys-http:3"),
            Some("http://sys-https:4"),
            &[],
        )),
    );
    assert_eq!(
        selection.select("wss", "api.example.com", 443).as_deref(),
        Some("http://sys-https:4")
    );
    assert_eq!(
        selection.select("http", "api.example.com", 80).as_deref(),
        Some("http://sys-http:3")
    );
}

#[test]
fn no_proxy_and_loopback_go_direct() {
    let selection = ProxySelection::with_system(
        env(&[
            ("HTTPS_PROXY", "http://env:1"),
            ("NO_PROXY", ".internal,example.org:8443"),
        ]),
        None,
    );
    assert_eq!(selection.select("https", "svc.internal", 443), None);
    assert_eq!(selection.select("https", "example.org", 8443), None);
    assert!(selection.select("https", "example.org", 443).is_some());
}

#[test]
fn loopback_skips_system_proxy_but_follows_environment() {
    let system_only = ProxySelection::with_system(
        ProxyEnv::default(),
        Some(system(Some("http://sys:3"), Some("http://sys:3"), &[])),
    );
    assert_eq!(system_only.select("https", "127.0.0.1", 443), None);
    assert_eq!(system_only.select("http", "localhost", 80), None);
    assert_eq!(system_only.select("https", "[::1]", 443), None);
    let with_env = ProxySelection::with_system(env(&[("HTTP_PROXY", "http://env:1")]), None);
    assert_eq!(
        with_env.select("http", "127.0.0.1", 80).as_deref(),
        Some("http://env:1")
    );
}

#[test]
fn system_bypass_list_only_affects_system_proxy() {
    let selection = ProxySelection::with_system(
        ProxyEnv::default(),
        Some(system(
            Some("http://sys:3"),
            None,
            &["*.corp.test", "10.0/16", "<local>"],
        )),
    );
    assert_eq!(selection.select("http", "git.corp.test", 80), None);
    assert_eq!(selection.select("http", "10.0.5.9", 80), None);
    assert_eq!(selection.select("http", "intranet", 80), None);
    assert!(selection.select("http", "10.1.0.1", 80).is_some());
    assert!(selection.select("http", "example.com", 80).is_some());
}

#[test]
fn direct_when_nothing_is_configured() {
    let selection = ProxySelection::with_system(ProxyEnv::default(), None);
    assert_eq!(selection.select("https", "example.com", 443), None);
}

#[test]
fn windows_proxy_string_forms() {
    let single = parse_windows_proxy("proxy.corp:8080", "*.corp;<local>");
    assert_eq!(single.http.as_deref(), Some("http://proxy.corp:8080"));
    assert_eq!(single.https.as_deref(), Some("http://proxy.corp:8080"));
    assert_eq!(single.bypass, ["*.corp", "<local>"]);

    let per_scheme = parse_windows_proxy("http=a:1;https=b:2;socks=c:3", "");
    assert_eq!(per_scheme.http.as_deref(), Some("http://a:1"));
    assert_eq!(per_scheme.https.as_deref(), Some("http://b:2"));
    assert!(per_scheme.bypass.is_empty());
}

#[tokio::test]
async fn reqwest_client_routes_through_selected_proxy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0u8; 2048];
        let read = socket.read(&mut buffer).await.expect("read");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await
            .expect("write");
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    });
    // The proxy itself is on loopback; only the target host must be non-local.
    let selection = ProxySelection::with_system(
        ProxyEnv::default(),
        Some(system(Some(&format!("http://127.0.0.1:{port}")), None, &[])),
    );
    let client = selection
        .apply(reqwest::Client::builder())
        .build()
        .expect("client");
    let body = client
        .get("http://upstream.example.test/probe")
        .send()
        .await
        .expect("send")
        .text()
        .await
        .expect("body");
    assert_eq!(body, "ok");
    let seen = server.await.expect("server");
    assert!(
        seen.starts_with("GET http://upstream.example.test/probe HTTP/1.1"),
        "{seen}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn plist_exception_list_is_read_back() {
    let xml = "<array><string>*.local</string><string>169.254/16</string><string>a&amp;b</string></array>";
    assert_eq!(plist_strings(xml), ["*.local", "169.254/16", "a&b"]);
}

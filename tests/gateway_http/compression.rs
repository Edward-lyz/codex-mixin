use super::*;

#[tokio::test]
async fn http_compression_preserves_long_context_and_stream() {
    let (upstream, requests) = spawn_compression_upstream(false, None).await;
    let gateway = spawn_gateway_with_config(test_config(upstream)).await;
    let mut body = responses_request();
    let context = "coding context with unicode \u{4e2d}\u{6587} ".repeat(10_000);
    body["input"][1]["content"][0]["text"] = json!(context);
    let response = reqwest::Client::builder()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .unwrap()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .header(header::CONTENT_ENCODING, "gzip")
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(gzip_bytes(&serde_json::to_vec(&body).unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    let stream = response.text().await.unwrap();
    assert!(stream.contains("response.completed"), "{stream}");
    assert!(stream.contains("hello"), "{stream}");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["encoding"], "gzip");
    assert!(
        requests[0]["wire_bytes"].as_u64().unwrap()
            < requests[0]["decoded_bytes"].as_u64().unwrap()
    );
    assert!(
        requests[0]["body"]
            .to_string()
            .contains("coding context with unicode")
    );
    assert!(
        requests[0]["accept_encoding"]
            .as_str()
            .unwrap()
            .contains("gzip")
    );
}

#[tokio::test]
async fn http_compression_streams_before_upstream_finishes() {
    use std::io::Write;
    let release = Arc::new(tokio::sync::Notify::new());
    let complete = Arc::clone(&release);
    let upstream = spawn_router(Router::new().route(
        "/v1/messages",
        post(move || {
            let complete = Arc::clone(&complete);
            async move {
                let sse = text_sse();
                let split = sse.find("\n\n").unwrap() + 2;
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder.write_all(&sse.as_bytes()[..split]).unwrap();
                encoder.flush().unwrap();
                let first = encoder.get_ref().clone();
                encoder.write_all(&sse.as_bytes()[split..]).unwrap();
                let compressed = encoder.finish().unwrap();
                let last = compressed[first.len()..].to_vec();
                let stream = async_stream::stream! {
                    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(first));
                    complete.notified().await;
                    yield Ok(bytes::Bytes::from(last));
                };
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CONTENT_ENCODING, "gzip")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    ))
    .await;
    let gateway = spawn_gateway_with_config(test_config(upstream)).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key").json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("message_start"));
    release.notify_one();
    let mut remainder = Vec::new();
    while let Some(chunk) = stream.next().await {
        remainder.extend_from_slice(&chunk.unwrap());
    }
    assert!(String::from_utf8_lossy(&remainder).contains("message_stop"));
}

#[tokio::test]
async fn http_compression_negotiates_unsupported_request_encoding() {
    let (upstream, requests) = spawn_compression_upstream(true, None).await;
    let gateway = spawn_gateway_with_config(test_config(upstream)).await;
    let mut body = responses_request();
    body["input"][1]["content"][0]["text"] = json!("long coding context ".repeat(10_000));
    for _ in 0..2 {
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("response.completed")
        );
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["encoding"], "gzip");
    assert_eq!(requests[1]["encoding"], Value::Null);
    assert_eq!(requests[2]["encoding"], Value::Null);
    assert_eq!(requests[0]["body"], requests[1]["body"]);
    assert_eq!(requests[1]["body"], requests[2]["body"]);
}

#[tokio::test]
async fn http_compression_does_not_replay_application_failures() {
    for status in [
        StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let (upstream, requests) = spawn_compression_upstream(false, Some(status)).await;
        let gateway = spawn_gateway_with_config(test_config(upstream)).await;
        let mut body = responses_request();
        body["input"][1]["content"][0]["text"] = json!("context ".repeat(20_000));
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn http_compression_rejects_malformed_request_encoding() {
    let (upstream, requests) = spawn_compression_upstream(false, None).await;
    let gateway = spawn_gateway_with_config(test_config(upstream)).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .header(header::CONTENT_ENCODING, "gzip")
        .body("not a gzip stream")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn http_compression_negotiates_json_responses() {
    use std::io::Read;
    let (upstream, _) = spawn_mock_upstream(MockMode::Text).await;
    let gateway = spawn_gateway_with_config(test_config(upstream)).await;
    let client = reqwest::Client::builder()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .unwrap();
    let response = client
        .get(format!("{gateway}/v1/models"))
        .bearer_auth("gateway-key")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert!(
        response.headers()[header::VARY]
            .to_str()
            .unwrap()
            .contains("accept-encoding")
    );
    let bytes = response.bytes().await.unwrap();
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_ref())
        .read_to_end(&mut decoded)
        .unwrap();
    assert!(
        !serde_json::from_slice::<Value>(&decoded).unwrap()["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let response = client
        .get(format!("{gateway}/v1/models"))
        .bearer_auth("gateway-key")
        .header(header::ACCEPT_ENCODING, "gzip;q=0,identity;q=1")
        .send()
        .await
        .unwrap();
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    assert!(response.json::<Value>().await.is_ok());
}

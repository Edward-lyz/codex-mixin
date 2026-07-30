use super::auth::{FORWARDED_OFFICIAL_HEADERS, check_gateway_auth, forward_official_headers};
use super::*;
use crate::provider::ProviderProtocol;

const REALTIME_CALL_BODY_LIMIT: usize = 4 * 1024 * 1024;
const CUSTOM_CALL_ID_PREFIX: &str = "codex-mixin";

enum RealtimeRoute<'a> {
    Official {
        authorization: axum::http::HeaderValue,
        account_id: axum::http::HeaderValue,
    },
    Provider {
        provider: &'a ProviderRuntime,
        upstream_model_id: Option<&'a str>,
    },
}

enum RealtimeWebsocketAuth<'a> {
    Official {
        authorization: axum::http::HeaderValue,
        account_id: axum::http::HeaderValue,
    },
    Provider(&'a ProviderRuntime),
}

pub(super) async fn realtime_ws(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    proxy_realtime_ws(state, uri, headers, ws, None).await
}

pub(super) async fn live_ws(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    proxy_realtime_ws(state, uri, headers, ws, None).await
}

pub(super) async fn live_sideband_ws(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    proxy_realtime_ws(state, uri, headers, ws, Some(call_id)).await
}

pub(super) async fn realtime_call(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| GatewayError::BadRequest("missing Content-Type".to_owned()))?;
    let boundary = multipart_boundary(content_type)?;
    let body = axum::body::to_bytes(body, REALTIME_CALL_BODY_LIMIT)
        .await
        .map_err(|error| {
            GatewayError::BadRequest(format!("invalid realtime call body: {error}"))
        })?;
    let sdp = multipart_text_field(&body, &boundary, "sdp")?;
    let session = multipart_text_field(&body, &boundary, "session")?;
    let mut session = serde_json::from_str::<Value>(&session).map_err(|error| {
        GatewayError::BadRequest(format!("invalid realtime call session JSON: {error}"))
    })?;
    let requested_model = session.get("model").and_then(Value::as_str);
    let route = resolve_realtime_route(&state, requested_model).await?;
    let is_live = uri.path() == "/v1/live";
    let (upstream, provider_id) = match route {
        RealtimeRoute::Official {
            authorization,
            account_id,
        } => {
            let mut url = official_codex_base_url(&state)?;
            url.path_segments_mut()
                .map_err(|_| GatewayError::BadRequest("invalid official Codex URL".to_owned()))?
                .extend(["realtime", "calls"]);
            set_official_call_query(&mut url, uri.query(), is_live);
            let upstream = forward_official_headers(
                state
                    .client
                    .post(url)
                    .header(header::AUTHORIZATION, authorization)
                    .header("chatgpt-account-id", account_id),
                &headers,
            )
            .json(&json!({"sdp": sdp, "session": session}))
            .send()
            .await?;
            (upstream, None)
        }
        RealtimeRoute::Provider {
            provider,
            upstream_model_id,
        } => {
            let upstream_model_id = upstream_model_id.ok_or_else(|| {
                GatewayError::BadRequest("custom realtime call is missing a model".to_owned())
            })?;
            session["model"] = Value::String(upstream_model_id.to_owned());
            let mut url =
                provider_realtime_url(provider, Some(upstream_model_id), is_live, true, None)?;
            set_mapped_query(&mut url, uri.query(), None, None)?;
            let multipart = encode_realtime_multipart(&boundary, &sdp, &session);
            let request = forward_official_headers(
                provider.apply_auth_for_protocol(
                    state.client.post(url),
                    ProviderProtocol::OpenAiResponses,
                ),
                &headers,
            )
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(multipart);
            (request.send().await?, Some(provider.id()))
        }
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let location = upstream
        .headers()
        .get(header::LOCATION)
        .map(|location| rewrite_custom_call_location(location, provider_id, is_live))
        .transpose()?;
    let body = upstream.bytes().await?;
    if !status.is_success() {
        return Err(GatewayError::Upstream(format!(
            "realtime call endpoint returned {status}: {}",
            String::from_utf8_lossy(&body)
        )));
    }
    let mut response = Response::builder().status(status);
    if let Some(content_type) = content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(location) = location {
        response = response.header(header::LOCATION, location);
    }
    response
        .body(Body::from(body))
        .map_err(|error| GatewayError::Other(error.into()))
}

async fn proxy_realtime_ws(
    state: AppState,
    uri: axum::http::Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    call_id: Option<String>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let query_call_id = query_value(&uri, "call_id");
    let requested_call_id = call_id.as_deref().or(query_call_id.as_deref());
    let requested_model = query_value(&uri, "model");
    let (route, upstream_call_id) = if let Some((provider_id, upstream_call_id)) =
        requested_call_id.and_then(parse_custom_call_id)
    {
        let provider = state
            .providers
            .provider(provider_id)
            .filter(|provider| provider.definition().enabled)
            .ok_or_else(|| {
                GatewayError::BadRequest(format!(
                    "custom realtime provider {provider_id} is unavailable"
                ))
            })?;
        (
            RealtimeRoute::Provider {
                provider,
                upstream_model_id: None,
            },
            Some(upstream_call_id.to_owned()),
        )
    } else {
        (
            resolve_realtime_route(&state, requested_model.as_deref()).await?,
            requested_call_id.map(str::to_owned),
        )
    };
    let upstream = connect_realtime_ws(&state, &headers, &uri, route, upstream_call_id.as_deref())
        .await
        .map_err(GatewayError::Other)?;
    Ok(ws
        .on_upgrade(move |client| async move {
            bridge_realtime_websockets(client, upstream).await;
        })
        .into_response())
}

async fn connect_realtime_ws(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    route: RealtimeRoute<'_>,
    upstream_call_id: Option<&str>,
) -> anyhow::Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
    let is_live = uri.path().starts_with("/v1/live");
    let (mut url, auth) = match route {
        RealtimeRoute::Official {
            authorization,
            account_id,
        } => {
            let mut url = official_codex_base_url(state)?;
            if is_live
                && uri.path() != "/v1/live"
                && let Some(call_id) = upstream_call_id
            {
                url.path_segments_mut()
                    .map_err(|_| anyhow::anyhow!("official realtime URL cannot be a base URL"))?
                    .push(call_id);
            }
            url.set_query(uri.query());
            (
                url,
                RealtimeWebsocketAuth::Official {
                    authorization,
                    account_id,
                },
            )
        }
        RealtimeRoute::Provider {
            provider,
            upstream_model_id,
        } => {
            let path_call_id = (is_live && uri.path() != "/v1/live")
                .then_some(upstream_call_id)
                .flatten();
            let mut url =
                provider_realtime_url(provider, upstream_model_id, is_live, false, path_call_id)?;
            set_mapped_query(
                &mut url,
                uri.query(),
                upstream_model_id,
                (!is_live).then_some(upstream_call_id).flatten(),
            )?;
            (url, RealtimeWebsocketAuth::Provider(provider))
        }
    };
    let websocket_scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => anyhow::bail!("unsupported realtime URL scheme: {scheme}"),
    };
    url.set_scheme(websocket_scheme)
        .map_err(|_| anyhow::anyhow!("failed to set realtime websocket scheme"))?;

    let mut request = url.as_str().into_client_request()?;
    {
        let request_headers = request.headers_mut();
        match auth {
            RealtimeWebsocketAuth::Official {
                authorization,
                account_id,
            } => {
                request_headers.insert(header::AUTHORIZATION, authorization);
                request_headers.insert("chatgpt-account-id", account_id);
            }
            RealtimeWebsocketAuth::Provider(provider) => {
                apply_provider_websocket_auth(provider, request_headers)?
            }
        }
        for &name in FORWARDED_OFFICIAL_HEADERS {
            if let Some(value) = headers.get(name) {
                request_headers.insert(name, value.clone());
            }
        }
    }
    let (upstream, _) = tokio::time::timeout(state.config.request_timeout, connect_async(request))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "realtime websocket connect timed out after {:?}",
                state.config.request_timeout
            )
        })??;
    Ok(upstream)
}

fn official_codex_base_url(state: &AppState) -> anyhow::Result<reqwest::Url> {
    reqwest::Url::parse(
        state
            .config
            .official_responses_url
            .strip_suffix("/responses")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "official responses URL must end with /responses: {}",
                    state.config.official_responses_url
                )
            })?,
    )
    .map_err(Into::into)
}

async fn resolve_realtime_route<'a>(
    state: &'a AppState,
    requested_model: Option<&str>,
) -> Result<RealtimeRoute<'a>, GatewayError> {
    if let Some(requested_model) = requested_model
        && let Some(resolved) = state.providers.resolve(requested_model)
    {
        return Ok(RealtimeRoute::Provider {
            provider: resolved.provider,
            upstream_model_id: Some(resolved.upstream_model_id),
        });
    }

    if let Some(requested_model) = requested_model
        && let Some(resolved) = state.providers.resolve_known(requested_model)
        && resolved.provider.definition().enabled
        && resolved.model.is_some()
    {
        return Ok(RealtimeRoute::Provider {
            provider: resolved.provider,
            upstream_model_id: Some(resolved.upstream_model_id),
        });
    }

    if let Some(requested_model) = requested_model
        && let Some(resolved) = state.providers.resolve_auxiliary_model(requested_model)
    {
        return Ok(RealtimeRoute::Provider {
            provider: resolved.provider,
            upstream_model_id: Some(resolved.upstream_model_id),
        });
    }

    if state.config.accept_codex_oauth
        && let Ok((authorization, account_id)) = state.official_auth().await
    {
        return Ok(RealtimeRoute::Official {
            authorization,
            account_id,
        });
    }

    let requested_model = requested_model.ok_or_else(|| {
        GatewayError::BadRequest(
            "realtime model is required when official OAuth is unavailable".to_owned(),
        )
    })?;
    let matches = state
        .providers
        .providers()
        .iter()
        .filter(|provider| provider.definition().enabled)
        .filter_map(|provider| {
            provider
                .definition()
                .cached_models
                .iter()
                .find(|model| model.id == requested_model)
                .map(|model| (provider, model.id.as_str()))
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(GatewayError::BadRequest(format!(
            "realtime model {requested_model} is unavailable: no official OAuth session and no enabled custom provider reports it"
        )));
    }
    let &(provider, upstream_model_id) = matches
        .iter()
        .find(|(provider, upstream_model_id)| {
            provider
                .definition()
                .selected_models
                .iter()
                .any(|selected| selected == upstream_model_id)
        })
        .unwrap_or(&matches[0]);
    Ok(RealtimeRoute::Provider {
        provider,
        upstream_model_id: Some(upstream_model_id),
    })
}

fn query_value(uri: &axum::http::Uri, name: &str) -> Option<String> {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.set_query(uri.query());
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

fn provider_realtime_url(
    provider: &ProviderRuntime,
    upstream_model_id: Option<&str>,
    is_live: bool,
    call_creation: bool,
    call_id: Option<&str>,
) -> anyhow::Result<reqwest::Url> {
    let mut url = upstream_model_id
        .map(|model| provider.api_url_for_model(model))
        .unwrap_or_else(|| provider.api_url())
        .clone();
    let api_path = url.path().trim_end_matches('/');
    let base_path = ["/chat/completions", "/responses", "/messages"]
        .into_iter()
        .find_map(|suffix| api_path.strip_suffix(suffix))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "provider {} API path cannot be mapped to realtime: {}",
                provider.id(),
                url.path()
            )
        })?;
    let mut path = format!("{base_path}/{}", if is_live { "live" } else { "realtime" });
    if call_creation && !is_live {
        path.push_str("/calls");
    }
    url.set_path(&path);
    url.set_query(None);
    if let Some(call_id) = call_id {
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("provider realtime URL cannot be a base URL"))?
            .push(call_id);
    }
    Ok(url)
}

fn set_mapped_query(
    url: &mut reqwest::Url,
    query: Option<&str>,
    upstream_model_id: Option<&str>,
    upstream_call_id: Option<&str>,
) -> anyhow::Result<()> {
    let mut source = reqwest::Url::parse("http://localhost/")?;
    source.set_query(query);
    let pairs = source
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if pairs.is_empty() {
        return Ok(());
    }
    let mut target = url.query_pairs_mut();
    for (name, value) in pairs {
        let value = match name.as_str() {
            "model" => upstream_model_id.unwrap_or(&value),
            "call_id" => upstream_call_id.unwrap_or(&value),
            _ => &value,
        };
        target.append_pair(&name, value);
    }
    Ok(())
}

fn set_official_call_query(url: &mut reqwest::Url, query: Option<&str>, is_live: bool) {
    url.set_query(query);
    if !is_live {
        return;
    }
    let existing_query_names = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect::<Vec<_>>();
    let mut query = url.query_pairs_mut();
    if !existing_query_names.iter().any(|name| name == "intent") {
        query.append_pair("intent", "quicksilver");
    }
    if !existing_query_names
        .iter()
        .any(|name| name == "architecture")
    {
        query.append_pair("architecture", "avas");
    }
}

fn apply_provider_websocket_auth(
    provider: &ProviderRuntime,
    headers: &mut HeaderMap,
) -> anyhow::Result<()> {
    let auth = &provider.definition().auth;
    match auth.header {
        crate::provider::ProviderAuthHeader::AuthorizationBearer => {
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {}", auth.api_key).parse()?,
            );
        }
        crate::provider::ProviderAuthHeader::XApiKey => {
            headers.insert("x-api-key", auth.api_key.parse()?);
        }
    }
    provider.apply_custom_headers(headers);
    Ok(())
}

fn encode_realtime_multipart(boundary: &str, sdp: &str, session: &Value) -> String {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\n{sdp}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{session}\r\n--{boundary}--\r\n"
    )
}

fn rewrite_custom_call_location(
    location: &axum::http::HeaderValue,
    provider_id: Option<&str>,
    is_live: bool,
) -> Result<axum::http::HeaderValue, GatewayError> {
    let Some(provider_id) = provider_id else {
        return Ok(location.clone());
    };
    let location = location.to_str().map_err(|error| {
        GatewayError::Upstream(format!("invalid realtime call Location header: {error}"))
    })?;
    let tail = location.rsplit_once('/').map_or(location, |(_, tail)| tail);
    let split_at = tail.find(['?', '#']).unwrap_or(tail.len());
    let (call_id, suffix) = tail.split_at(split_at);
    if call_id.is_empty() {
        return Err(GatewayError::Upstream(
            "realtime call Location header has no call id".to_owned(),
        ));
    }
    let token = format!("{CUSTOM_CALL_ID_PREFIX}~{provider_id}~{call_id}");
    let gateway_path = if is_live {
        "/v1/live"
    } else {
        "/v1/realtime/calls"
    };
    let location = format!("{gateway_path}/{token}{suffix}");
    location.parse().map_err(|error| {
        GatewayError::Upstream(format!(
            "invalid mapped realtime call Location header: {error}"
        ))
    })
}

fn parse_custom_call_id(call_id: &str) -> Option<(&str, &str)> {
    let mut parts = call_id.splitn(3, '~');
    if parts.next()? != CUSTOM_CALL_ID_PREFIX {
        return None;
    }
    let provider_id = parts.next()?;
    let upstream_call_id = parts.next()?;
    (!provider_id.is_empty() && !upstream_call_id.is_empty())
        .then_some((provider_id, upstream_call_id))
}

fn multipart_boundary(content_type: &str) -> Result<String, GatewayError> {
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("multipart/form-data"))
    {
        return Err(GatewayError::BadRequest(
            "realtime call Content-Type must be multipart/form-data".to_owned(),
        ));
    }
    content_type
        .split(';')
        .skip(1)
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("boundary")
                .then(|| value.trim().trim_matches('"').to_owned())
        })
        .filter(|boundary| !boundary.is_empty())
        .ok_or_else(|| GatewayError::BadRequest("missing multipart boundary".to_owned()))
}

fn multipart_text_field(
    body: &[u8],
    boundary: &str,
    field_name: &str,
) -> Result<String, GatewayError> {
    let body = std::str::from_utf8(body).map_err(|error| {
        GatewayError::BadRequest(format!(
            "realtime call multipart body is not UTF-8: {error}"
        ))
    })?;
    let marker = format!("--{boundary}");
    for part in body.split(&marker) {
        let Some((headers, value)) = part.split_once("\r\n\r\n") else {
            continue;
        };
        let expected_name = format!("name=\"{field_name}\"");
        if headers.lines().any(|line| {
            line.to_ascii_lowercase()
                .starts_with("content-disposition:")
                && line.contains(&expected_name)
        }) {
            return Ok(value.strip_suffix("\r\n").unwrap_or(value).to_owned());
        }
    }
    Err(GatewayError::BadRequest(format!(
        "missing realtime call multipart field: {field_name}"
    )))
}

async fn bridge_realtime_websockets(
    client: WebSocket,
    upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let (mut client_sender, mut client_receiver) = client.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();
    loop {
        tokio::select! {
            client_message = client_receiver.next() => {
                let Some(client_message) = client_message else {
                    let _ = upstream_sender.send(TungsteniteMessage::Close(None)).await;
                    return;
                };
                let message = match client_message {
                    Ok(AxumWsMessage::Text(text)) => {
                        TungsteniteMessage::Text(text.to_string().into())
                    }
                    Ok(AxumWsMessage::Binary(bytes)) => TungsteniteMessage::Binary(bytes),
                    Ok(AxumWsMessage::Ping(bytes)) => TungsteniteMessage::Ping(bytes),
                    Ok(AxumWsMessage::Pong(bytes)) => TungsteniteMessage::Pong(bytes),
                    Ok(AxumWsMessage::Close(_)) => {
                        let _ = upstream_sender.send(TungsteniteMessage::Close(None)).await;
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "realtime client websocket read failed");
                        let _ = upstream_sender.send(TungsteniteMessage::Close(None)).await;
                        return;
                    }
                };
                if let Err(error) = upstream_sender.send(message).await {
                    tracing::warn!(%error, "upstream realtime websocket write failed");
                    return;
                }
            }
            upstream_message = upstream_receiver.next() => {
                let Some(upstream_message) = upstream_message else {
                    let _ = client_sender.send(AxumWsMessage::Close(None)).await;
                    return;
                };
                let message = match upstream_message {
                    Ok(TungsteniteMessage::Text(text)) => {
                        Some(AxumWsMessage::Text(text.to_string().into()))
                    }
                    Ok(TungsteniteMessage::Binary(bytes)) => {
                        Some(AxumWsMessage::Binary(bytes))
                    }
                    Ok(TungsteniteMessage::Ping(bytes)) => Some(AxumWsMessage::Ping(bytes)),
                    Ok(TungsteniteMessage::Pong(bytes)) => Some(AxumWsMessage::Pong(bytes)),
                    Ok(TungsteniteMessage::Close(_)) => Some(AxumWsMessage::Close(None)),
                    Ok(TungsteniteMessage::Frame(_)) => None,
                    Err(error) => {
                        tracing::warn!(%error, "upstream realtime websocket read failed");
                        let _ = client_sender.send(AxumWsMessage::Close(None)).await;
                        return;
                    }
                };
                if let Some(message) = message
                    && let Err(error) = client_sender.send(message).await
                {
                    tracing::warn!(%error, "realtime client websocket write failed");
                    return;
                }
            }
        }
    }
}

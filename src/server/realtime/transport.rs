use super::auth::{FORWARDED_OFFICIAL_HEADERS, check_gateway_auth};
use super::routing::{
    RealtimeRoute, official_codex_base_url, official_live_sideband_url, parse_custom_call_id,
    provider_realtime_url, resolve_realtime_route, set_mapped_query,
};
use super::websocket_proxy::connect_upstream_websocket;
use super::{AppState, GatewayError, ProviderRuntime};
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite_proxy::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream};

pub(super) async fn proxy_realtime_ws(
    state: AppState,
    uri: Uri,
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

fn query_value(uri: &Uri, name: &str) -> Option<String> {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.set_query(uri.query());
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

enum RealtimeWebsocketAuth<'a> {
    Official {
        authorization: axum::http::HeaderValue,
        account_id: axum::http::HeaderValue,
    },
    Provider(&'a ProviderRuntime),
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

async fn connect_realtime_ws(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    route: RealtimeRoute<'_>,
    upstream_call_id: Option<&str>,
) -> anyhow::Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
    let is_live = uri.path().starts_with("/v1/live");
    let (mut url, auth) = match route {
        RealtimeRoute::Official {
            authorization,
            account_id,
        } => {
            let url = if is_live {
                let call_id = upstream_call_id
                    .ok_or_else(|| anyhow::anyhow!("official live websocket requires a call id"))?;
                official_live_sideband_url(call_id)?
            } else {
                let mut url = official_codex_base_url(state)?;
                url.path_segments_mut()
                    .map_err(|_| anyhow::anyhow!("official realtime URL cannot be a base URL"))?
                    .push("realtime");
                url.set_query(uri.query());
                url
            };
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
    let upstream = tokio::time::timeout(
        state.config.request_timeout,
        connect_upstream_websocket(request, state.websocket_proxy_env()),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "realtime websocket connect timed out after {:?}",
            state.config.request_timeout
        )
    })??;
    Ok(upstream)
}

async fn bridge_realtime_websockets(
    client: WebSocket,
    upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let (mut client_sender, mut client_receiver) = client.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();
    tokio::select! {
        () = forward_realtime_client_messages(&mut client_receiver, &mut upstream_sender) => {}
        () = forward_realtime_upstream_messages(&mut upstream_receiver, &mut client_sender) => {}
    }
}

async fn forward_realtime_client_messages(
    client_receiver: &mut SplitStream<WebSocket>,
    upstream_sender: &mut SplitSink<
        WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        TungsteniteMessage,
    >,
) {
    while let Some(message) = client_receiver.next().await {
        let Some(message) = client_realtime_message(message) else {
            close_upstream_realtime(upstream_sender).await;
            return;
        };
        if let Err(error) = upstream_sender.send(message).await {
            tracing::warn!(%error, "upstream realtime websocket write failed");
            return;
        }
    }
    close_upstream_realtime(upstream_sender).await;
}

fn client_realtime_message(
    message: Result<AxumWsMessage, axum::Error>,
) -> Option<TungsteniteMessage> {
    match message {
        Ok(AxumWsMessage::Text(text)) => Some(TungsteniteMessage::Text(text.to_string().into())),
        Ok(AxumWsMessage::Binary(bytes)) => Some(TungsteniteMessage::Binary(bytes)),
        Ok(AxumWsMessage::Ping(bytes)) => Some(TungsteniteMessage::Ping(bytes)),
        Ok(AxumWsMessage::Pong(bytes)) => Some(TungsteniteMessage::Pong(bytes)),
        Ok(AxumWsMessage::Close(_)) => None,
        Err(error) => {
            tracing::warn!(%error, "realtime client websocket read failed");
            None
        }
    }
}

async fn close_upstream_realtime(
    upstream_sender: &mut SplitSink<
        WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        TungsteniteMessage,
    >,
) {
    if let Err(error) = upstream_sender.send(TungsteniteMessage::Close(None)).await {
        tracing::warn!(%error, "upstream realtime websocket close failed");
    }
}

async fn forward_realtime_upstream_messages(
    upstream_receiver: &mut SplitStream<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
) {
    while let Some(message) = upstream_receiver.next().await {
        let message = upstream_realtime_message(message);
        if let Some(message) = message
            && let Err(error) = client_sender.send(message).await
        {
            tracing::warn!(%error, "realtime client websocket write failed");
            return;
        }
    }
    close_realtime_client(client_sender).await;
}

fn upstream_realtime_message(
    message: Result<TungsteniteMessage, impl std::fmt::Display>,
) -> Option<AxumWsMessage> {
    match message {
        Ok(TungsteniteMessage::Text(text)) => Some(AxumWsMessage::Text(text.to_string().into())),
        Ok(TungsteniteMessage::Binary(bytes)) => Some(AxumWsMessage::Binary(bytes)),
        Ok(TungsteniteMessage::Ping(bytes)) => Some(AxumWsMessage::Ping(bytes)),
        Ok(TungsteniteMessage::Pong(bytes)) => Some(AxumWsMessage::Pong(bytes)),
        Ok(TungsteniteMessage::Close(_)) => Some(AxumWsMessage::Close(None)),
        Ok(TungsteniteMessage::Frame(_)) => None,
        Err(error) => {
            tracing::warn!(%error, "upstream realtime websocket read failed");
            Some(AxumWsMessage::Close(None))
        }
    }
}

async fn close_realtime_client(client_sender: &mut SplitSink<WebSocket, AxumWsMessage>) {
    if let Err(error) = client_sender.send(AxumWsMessage::Close(None)).await {
        tracing::warn!(%error, "realtime client websocket close failed");
    }
}

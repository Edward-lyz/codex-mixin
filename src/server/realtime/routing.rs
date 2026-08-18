use super::{AppState, GatewayError, ProviderRuntime};

const CUSTOM_CALL_ID_PREFIX: &str = "codex-mixin";
const OPENAI_LIVE_BASE_URL: &str = "https://api.openai.com/v1";

pub(super) enum RealtimeRoute<'a> {
    Official {
        authorization: axum::http::HeaderValue,
        account_id: axum::http::HeaderValue,
    },
    Provider {
        provider: &'a ProviderRuntime,
        upstream_model_id: Option<&'a str>,
    },
}

pub(super) fn official_codex_base_url(state: &AppState) -> anyhow::Result<reqwest::Url> {
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

pub(super) fn official_live_sideband_url(call_id: &str) -> anyhow::Result<reqwest::Url> {
    // ChatGPT creates the call, but the live sideband websocket is hosted by
    // the OpenAI API rather than the ChatGPT backend.
    let mut url = reqwest::Url::parse(OPENAI_LIVE_BASE_URL)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("official live URL cannot be a base URL"))?
        .extend(["live", call_id]);
    Ok(url)
}

pub(super) async fn resolve_realtime_route<'a>(
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

pub(super) fn provider_realtime_url(
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

pub(super) fn set_mapped_query(
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

pub(super) fn set_official_call_query(url: &mut reqwest::Url, query: Option<&str>, is_live: bool) {
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

pub(super) fn rewrite_custom_call_location(
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

pub(super) fn parse_custom_call_id(call_id: &str) -> Option<(&str, &str)> {
    let mut parts = call_id.splitn(3, '~');
    if parts.next()? != CUSTOM_CALL_ID_PREFIX {
        return None;
    }
    let provider_id = parts.next()?;
    let upstream_call_id = parts.next()?;
    (!provider_id.is_empty() && !upstream_call_id.is_empty())
        .then_some((provider_id, upstream_call_id))
}

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use futures_util::StreamExt;

#[derive(Default)]
pub(super) struct RequestActivity {
    inner: Mutex<ActivityState>,
}

struct ActivityState {
    active: usize,
    last_activity: Instant,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            active: 0,
            last_activity: Instant::now(),
        }
    }
}

impl RequestActivity {
    pub(super) fn begin(self: &Arc<Self>) -> RequestGuard {
        // Poisoning must never make the updater consider the gateway idle.
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active += 1;
        state.last_activity = Instant::now();
        RequestGuard(Arc::clone(self))
    }

    pub(super) fn snapshot(&self) -> serde_json::Value {
        let Ok(state) = self.inner.lock() else {
            return serde_json::Value::Null;
        };
        serde_json::json!({
            "active_requests":state.active,
            "idle_seconds":if state.active == 0 { Some(state.last_activity.elapsed().as_secs()) } else { None },
        })
    }
}

pub(super) struct RequestGuard(Arc<RequestActivity>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let mut state = self
            .0
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        state.last_activity = Instant::now();
    }
}

pub(super) async fn track_requests(
    State(activity): State<Arc<RequestActivity>>,
    request: Request,
    next: Next,
) -> Response {
    // Model/usage/status polling is control traffic and must not prevent updates.
    if request.method() != axum::http::Method::POST {
        return next.run(request).await;
    }
    let guard = activity.begin();
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _guard = guard;
        let mut body = body.into_data_stream();
        while let Some(chunk) = body.next().await {
            yield chunk;
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lifetime_resets_idle() {
        let activity = Arc::new(RequestActivity::default());
        activity.inner.lock().unwrap().last_activity =
            Instant::now() - std::time::Duration::from_secs(1801);
        assert!(activity.snapshot()["idle_seconds"].as_u64().unwrap() >= 1800);
        let first = activity.begin();
        let second = activity.begin();
        assert_eq!(activity.snapshot()["active_requests"], 2);
        assert!(activity.snapshot()["idle_seconds"].is_null());
        drop(first);
        assert_eq!(activity.snapshot()["active_requests"], 1);
        drop(second);
        assert_eq!(activity.snapshot()["active_requests"], 0);
        assert_eq!(activity.snapshot()["idle_seconds"], 0);
    }

    #[tokio::test]
    async fn streaming_body_and_cancellation_keep_gateway_busy() {
        use axum::{Router, routing::post};
        use tower::ServiceExt;
        let activity = Arc::new(RequestActivity::default());
        let router = Router::new()
            .route(
                "/v1/responses",
                post(|| async {
                    Body::from_stream(
                        futures_util::stream::once(async {
                            Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                                b"data: streaming\n\n",
                            ))
                        })
                        .chain(futures_util::stream::pending()),
                    )
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&activity),
                track_requests,
            ));
        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(activity.snapshot()["active_requests"], 1);
        let mut body = response.into_body().into_data_stream();
        assert!(body.next().await.unwrap().is_ok());
        assert_eq!(activity.snapshot()["active_requests"], 1);
        drop(body);
        assert_eq!(activity.snapshot()["active_requests"], 0);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(activity.snapshot()["active_requests"], 1);
        drop(response);
        assert_eq!(activity.snapshot()["active_requests"], 0);
    }
}

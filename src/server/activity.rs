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

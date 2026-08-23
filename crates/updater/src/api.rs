use std::{net::SocketAddr, sync::Mutex};

use anyhow::{Context, Result, anyhow, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;

use crate::{
    process, progress,
    progress::{UpdateProgress, UpdateState},
    state,
};

const API_PORT: u16 = 8080;

#[derive(Clone)]
struct ApiState {
    queue: std::sync::Arc<Mutex<()>>,
    allowed_origins: std::sync::Arc<[HeaderValue; 2]>,
}

#[derive(Serialize)]
struct UpdateStatus {
    installed_version: String,
    #[serde(flatten)]
    progress: UpdateProgress,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Clone, Copy)]
enum QueueAction {
    Check,
    Update,
}

pub(crate) fn serve() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start updater API runtime")?
        .block_on(run())
}

async fn run() -> Result<()> {
    let host = process::status_address()?;
    let address = SocketAddr::from((host, API_PORT));
    let state = ApiState {
        queue: std::sync::Arc::new(Mutex::new(())),
        allowed_origins: std::sync::Arc::new([
            HeaderValue::from_static("http://gofrowifi.net"),
            HeaderValue::from_str(&format!("http://{host}"))
                .context("failed to build updater API origin")?,
        ]),
    };
    let router = Router::new()
        .route("/api/status", get(status))
        .route("/api/check", post(check))
        .route("/api/start", post(start))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind updater API to {address}"))?;
    println!("Updater API listening on {address}");
    axum::serve(listener, router)
        .await
        .context("updater API failed")
}

async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let origin = match authorize(&state, &headers, false) {
        Ok(origin) => origin,
        Err(error) => return failure(StatusCode::FORBIDDEN, error, None),
    };
    match load_status() {
        Ok(status) => json(StatusCode::OK, status, origin),
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, error, origin),
    }
}

async fn check(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    queue(state, headers, QueueAction::Check).await
}

async fn start(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    queue(state, headers, QueueAction::Update).await
}

async fn queue(state: ApiState, headers: HeaderMap, action: QueueAction) -> Response {
    let origin = match authorize(&state, &headers, true) {
        Ok(origin) => origin,
        Err(error) => return failure(StatusCode::FORBIDDEN, error, None),
    };
    let result = tokio::task::spawn_blocking(move || queue_update(&state, action)).await;
    match result {
        Ok(Ok(status)) => json(StatusCode::ACCEPTED, status, origin),
        Ok(Err(error)) => failure(StatusCode::CONFLICT, error, origin),
        Err(error) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow!(error).context("updater API task failed"),
            origin,
        ),
    }
}

fn queue_update(state: &ApiState, action: QueueAction) -> Result<UpdateStatus> {
    let _guard = state
        .queue
        .lock()
        .map_err(|_| anyhow!("update queue lock is poisoned"))?;
    ensure!(
        !process::updater_busy()?,
        "an update operation is already running"
    );
    if matches!(action, QueueAction::Update) {
        ensure!(
            progress::read()?.state == UpdateState::Available,
            "check for an available update first"
        );
    }

    let checking = UpdateProgress::new(UpdateState::Checking, None);
    if let Err(error) = progress::write(&checking)
        .and_then(|()| process::start_updater(matches!(action, QueueAction::Check)))
    {
        let _ = progress::write_error(&error);
        return Err(error);
    }
    load_status()
}

fn load_status() -> Result<UpdateStatus> {
    Ok(UpdateStatus {
        installed_version: state::read_version()?.to_string(),
        progress: progress::read()?,
    })
}

fn authorize(
    state: &ApiState,
    headers: &HeaderMap,
    require_origin: bool,
) -> Result<Option<HeaderValue>> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        ensure!(!require_origin, "request origin is required");
        return Ok(None);
    };
    ensure!(
        state
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin),
        "request origin is not allowed"
    );
    Ok(Some(origin.clone()))
}

fn json(status: StatusCode, body: impl Serialize, origin: Option<HeaderValue>) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
}

fn failure(status: StatusCode, error: anyhow::Error, origin: Option<HeaderValue>) -> Response {
    json(
        status,
        ErrorBody {
            error: error.to_string(),
        },
        origin,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_require_the_dashboard_origin() {
        let state = ApiState {
            queue: std::sync::Arc::new(Mutex::new(())),
            allowed_origins: std::sync::Arc::new([
                HeaderValue::from_static("http://gofrowifi.net"),
                HeaderValue::from_static("http://10.203.1.1"),
            ]),
        };
        assert!(authorize(&state, &HeaderMap::new(), true).is_err());
        let mut allowed = HeaderMap::new();
        allowed.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://gofrowifi.net"),
        );
        assert!(authorize(&state, &allowed, true).is_ok());
        allowed.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        assert!(authorize(&state, &allowed, true).is_err());
    }
}

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tracing::error;
use wireguard_status::wireguard_peers;

use crate::{
    AppState, controller, dataplane,
    model::{
        AP_ADDRESS, AP_DOMAIN, AgentStatus, ApInput, ApStatus, ModeInput, RoutingConfig,
        RoutingStatus, RoutingTestInput, RoutingTestResult, ServerKeyInput, ServerProfile,
        ServerUpdate,
    },
    network::service_active,
    wifi,
};

const UI: &str = include_str!("../../../assets/index.html");
const UI_JS: &str = include_str!("../../../assets/app.js");
const UI_CSS: &str = include_str!("../../../assets/app.css");

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(javascript))
        .route("/app.css", get(stylesheet))
        .route("/api/status", get(status))
        .route("/api/mode", post(set_mode))
        .route(
            "/api/servers",
            post(add_server).put(update_server).delete(delete_server),
        )
        .route("/api/servers/select", post(select_server))
        .route("/api/ap", post(update_ap))
        .route("/api/routing", post(update_routing))
        .route("/api/routing/test", post(test_routing))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(UI))
}

async fn javascript() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        UI_JS,
    )
}

async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        UI_CSS,
    )
}

async fn status(State(state): State<AppState>) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, |_| Ok(())).await
}

async fn set_mode(
    State(state): State<AppState>,
    Json(input): Json<ModeInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| {
        controller::set_mode(state, input.vpn_enabled)
    })
    .await
}

async fn add_server(
    State(state): State<AppState>,
    Json(mut server): Json<ServerProfile>,
) -> Result<Json<AgentStatus>, ApiError> {
    server.name = server.name.trim().to_owned();
    server.endpoint = server.endpoint.trim().to_owned();
    server.public_key = server.public_key.trim().to_owned();
    run_blocking(state, move |state| controller::add_server(state, server)).await
}

async fn update_server(
    State(state): State<AppState>,
    Json(mut update): Json<ServerUpdate>,
) -> Result<Json<AgentStatus>, ApiError> {
    update.previous_public_key = update.previous_public_key.trim().to_owned();
    update.name = update.name.trim().to_owned();
    update.endpoint = update.endpoint.trim().to_owned();
    update.public_key = update.public_key.trim().to_owned();
    run_blocking(state, move |state| controller::update_server(state, update)).await
}

async fn update_ap(
    State(state): State<AppState>,
    Json(mut input): Json<ApInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    input.ssid = input.ssid.trim().to_owned();
    run_blocking(state, move |state| {
        controller::update_access_point(state, input)
    })
    .await
}

async fn update_routing(
    State(state): State<AppState>,
    Json(input): Json<RoutingConfig>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| controller::update_routing(state, input)).await
}

async fn test_routing(
    State(state): State<AppState>,
    Json(input): Json<RoutingTestInput>,
) -> Result<Json<RoutingTestResult>, ApiError> {
    tokio::task::spawn_blocking(move || {
        state
            .routing
            .read()
            .map_err(|_| anyhow!("routing lock poisoned"))?
            .test(&input.value)
            .map(Json)
    })
    .await
    .context("routing test task failed")
    .map_err(ApiError)?
    .map_err(ApiError)
}

async fn select_server(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| {
        controller::select_server(state, input.public_key.trim())
    })
    .await
}

async fn delete_server(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| {
        controller::delete_server(state, input.public_key.trim())
    })
    .await
}

async fn run_blocking<F>(state: AppState, operation: F) -> Result<Json<AgentStatus>, ApiError>
where
    F: FnOnce(&AppState) -> Result<()> + Send + 'static,
{
    let result = tokio::task::spawn_blocking(move || {
        operation(&state)?;
        load_status(&state).map(Json)
    })
    .await
    .context("controller task failed")
    .map_err(ApiError)?;
    result.map_err(ApiError)
}

fn load_status(state: &AppState) -> Result<AgentStatus> {
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?
        .clone();
    let tunnel_active = service_active(&state.interface)?;
    let peer = if tunnel_active {
        wireguard_peers(&state.interface)?.into_iter().next()
    } else {
        None
    };
    let readings = wifi::devices(&state.wifi_interface);
    let (stats, history, devices) = state
        .stats
        .lock()
        .map_err(|_| anyhow!("statistics lock poisoned"))?
        .sample(peer.as_ref(), readings);

    Ok(AgentStatus {
        version: env!("CARGO_PKG_VERSION"),
        vpn_enabled: config.vpn_enabled,
        tunnel_active,
        interface: state.interface.clone(),
        active_server_key: config.active_server_key,
        servers: config.servers,
        ap: ApStatus {
            ssid: config.ap_ssid,
            address: AP_ADDRESS,
            domain: AP_DOMAIN,
        },
        peer,
        stats,
        history,
        devices,
        routing: RoutingStatus {
            config: config.routing,
            dns_active: state.fake_dns.is_active(),
            fake_ips: state.fake_dns.count(),
            geosite_loaded: state.geodata.has_site("category-ru"),
            geoip_loaded: state.geodata.has_ip("ru"),
            dataplane_active: dataplane::is_installed(),
        },
    })
}

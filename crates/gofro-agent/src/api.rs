use std::{fs, path::Path, process::Command, sync::atomic::Ordering};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde::Serialize;
use tracing::error;
use wireguard_status::wireguard_peers;

use crate::{
    AppState, auth, controller, dataplane,
    model::{
        AgentStatus, AutoUpdateInput, ModeInput, ProfileInput, RoutingConfig, RoutingStatus,
        RoutingTestInput, RoutingTestResult, ServerKeyInput, ServerStatus, ServerUpdate,
        UpdateInput, UpdateResult, UpdateStatus,
    },
    network::service_active,
    onboarding, stats,
};

mod assets;
mod managed;

const UPDATE_LOCK: &str = "/tmp/gofro-update.lock";
const UPDATE_RESULT: &str = "/tmp/gofro/update-result";
const UPDATE_TRIGGER: &str = "/tmp/gofro/update-request";
const UPDATE_COMMAND: &str = "/usr/libexec/gofro/update";

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<OperationOutcome>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum OperationOutcome {
    Committed,
}

struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: self.0.to_string(),
                outcome: self
                    .0
                    .is::<crate::managed::CommittedRefreshFailed>()
                    .then_some(OperationOutcome::Committed),
            }),
        )
            .into_response()
    }
}

pub(crate) fn secure_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::index))
        .route("/app.js", get(assets::javascript))
        .route("/app.css", get(assets::stylesheet))
        .route("/chart.js", get(assets::chart))
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .merge(private_router().layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        )))
        .with_state(state)
}

fn private_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/onboarding", get(onboarding_status))
        .route("/api/onboarding/complete", post(onboarding_complete))
        .route("/api/status", get(status))
        .route("/api/update", post(start_update).put(set_auto_update))
        .route("/api/reboot", post(network_managed_by_openwrt))
        .route("/api/mode", post(set_mode))
        .route(
            "/api/servers",
            axum::routing::put(update_server).delete(delete_server),
        )
        .route("/api/servers/import", post(import_server))
        .route("/api/servers/probe", post(managed::probe_server))
        .route("/api/servers/bootstrap", post(managed::bootstrap_server))
        .route(
            "/api/servers/host-pin",
            axum::routing::delete(managed::reset_host_pin),
        )
        .route("/api/servers/check", post(managed::check_managed_server))
        .route(
            "/api/servers/update-managed",
            post(managed::update_managed_server),
        )
        .route(
            "/api/servers/create-profile",
            post(managed::create_managed_profile),
        )
        .route(
            "/api/servers/management",
            post(managed::managed_server_status),
        )
        .route(
            "/api/servers/restart",
            post(managed::restart_managed_server),
        )
        .route(
            "/api/servers/friends",
            post(managed::create_friend)
                .put(managed::rename_friend)
                .delete(managed::revoke_friend),
        )
        .route(
            "/api/servers/friends/profile",
            post(managed::friend_profile),
        )
        .route("/api/servers/select", post(select_server))
        .route("/api/ap", post(network_managed_by_openwrt))
        .route("/api/onboarding/wifi", post(network_managed_by_openwrt))
        .route("/api/routing", post(update_routing))
        .route("/api/device-exclusions", post(update_device_exclusion))
        .route("/api/lan-devices", get(lan_devices))
        .route("/api/routing/test", post(test_routing))
}

pub(crate) fn redirect_router(state: AppState) -> Router {
    Router::new().fallback(any(redirect)).with_state(state)
}

async fn redirect(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    let Some(host) = auth::request_authority(&uri, &headers) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let destination = if host == crate::model::AP_DOMAIN
        || host == format!("{}:80", crate::model::AP_DOMAIN)
        || host == format!("{}:8081", crate::model::AP_DOMAIN)
        || host == format!("{}:{}", crate::model::AP_DOMAIN, state.http_listen.port())
    {
        format!("https://{}", crate::model::AP_DOMAIN)
    } else if host == format!("{}:{}", state.lan.address, state.http_listen.port()) {
        format!(
            "https://{}:{}",
            state.lan.address,
            state.https_listen.port()
        )
    } else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let location = format!(
        "{}{}",
        destination,
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/")
    );
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, location)],
    )
        .into_response()
}

#[derive(Serialize)]
struct Health {
    version: &'static str,
    degraded: bool,
    dns_active: bool,
    dataplane_active: bool,
    vpn_enabled: bool,
    tunnel_active: bool,
    handshake_age_seconds: Option<u64>,
}

impl IntoResponse for Health {
    fn into_response(self) -> Response {
        let status = if self.degraded {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        };
        (status, Json(self)).into_response()
    }
}
pub(crate) fn health_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .with_state(state)
}
async fn health(State(state): State<AppState>) -> Result<Health, ApiError> {
    let status = load_status(&state).map_err(ApiError)?;
    Ok(Health {
        version: status.version,
        degraded: status.routing.degraded,
        dns_active: status.routing.dns_active,
        dataplane_active: status.routing.dataplane_active,
        vpn_enabled: status.vpn_enabled,
        tunnel_active: status.tunnel_active,
        handshake_age_seconds: status.peer.and_then(|peer| peer.handshake_age_seconds),
    })
}

async fn status(State(state): State<AppState>) -> Result<Json<AgentStatus>, ApiError> {
    tokio::task::spawn_blocking(move || load_status(&state).map(Json))
        .await
        .context("status task failed")
        .map_err(ApiError)?
        .map_err(ApiError)
}

async fn onboarding_status(
    State(state): State<AppState>,
) -> Result<Json<onboarding::Status>, ApiError> {
    tokio::task::spawn_blocking(move || onboarding::status(&state).map(Json))
        .await
        .context("onboarding status task failed")
        .map_err(ApiError)?
        .map_err(|_| ApiError(anyhow!("onboarding unavailable")))
}
async fn onboarding_complete(
    State(state): State<AppState>,
) -> Result<Json<onboarding::Status>, ApiError> {
    tokio::task::spawn_blocking(move || onboarding::complete(&state).map(Json))
        .await
        .context("onboarding completion task failed")
        .map_err(ApiError)?
        .map_err(|_| ApiError(anyhow!("onboarding completion rejected")))
}
async fn network_managed_by_openwrt() -> Response {
    (
        StatusCode::GONE,
        Json(serde_json::json!({"error":"network_managed_by_openwrt"})),
    )
        .into_response()
}

async fn start_update(
    State(state): State<AppState>,
    Json(_): Json<UpdateInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, |_| queue_update()).await
}

async fn set_auto_update(
    State(state): State<AppState>,
    Json(input): Json<AutoUpdateInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| {
        controller::set_auto_update(state, input.enabled)
    })
    .await
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

async fn import_server(
    State(state): State<AppState>,
    Json(mut input): Json<ProfileInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    input.name = input.name.trim().to_owned();
    run_blocking(state, move |state| {
        controller::import_server(state, input.name, input.profile)
    })
    .await
}

async fn update_routing(
    State(state): State<AppState>,
    Json(input): Json<RoutingConfig>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| controller::update_routing(state, input)).await
}

async fn update_device_exclusion(
    State(state): State<AppState>,
    Json(input): Json<crate::model::DeviceExclusionInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, move |state| {
        controller::update_device_exclusion(state, input.mac, input.excluded)
    })
    .await
}

async fn lan_devices(
    State(state): State<AppState>,
) -> Result<Json<crate::devices::LanDeviceInventory>, ApiError> {
    tokio::task::spawn_blocking(move || crate::devices::list(&state).map(Json))
        .await
        .context("LAN inventory task failed")
        .map_err(ApiError)?
        .map_err(ApiError)
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
        load_status(&state)
            .map(Json)
            .context(crate::managed::CommittedRefreshFailed)
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
    let (stats, history) = state
        .stats
        .lock()
        .map_err(|_| anyhow!("statistics lock poisoned"))?
        .sample(stats::interface_traffic(&state.interface).unwrap_or_default());

    Ok(AgentStatus {
        device_exclusions: config.device_exclusions,
        version: env!("CARGO_PKG_VERSION"),
        update: update_status(config.auto_update_enabled),
        vpn_enabled: config.vpn_enabled,
        tunnel_active,
        interface: state.interface.clone(),
        active_server_key: config.active_server_key,
        servers: config.servers.iter().map(ServerStatus::from).collect(),
        peer,
        stats,
        history,
        routing: RoutingStatus {
            config: config.routing,
            dns_active: state.fake_dns.is_active(),
            fake_ips: state.fake_dns.count(),
            geosite_loaded: state.geodata.has_site("category-ru"),
            geoip_loaded: state.geodata.has_ip("ru"),
            dataplane_active: dataplane::is_installed(),
            degraded: state.routing_degraded.load(Ordering::Relaxed),
        },
    })
}

fn queue_update() -> Result<()> {
    if Path::new(UPDATE_TRIGGER).exists() {
        return Ok(());
    }
    if Path::new(UPDATE_LOCK).exists() {
        return Err(anyhow!("update unavailable while rebooting"));
    }

    let started = Command::new(UPDATE_COMMAND)
        .arg("request")
        .status()
        .context("failed to request update")?;
    if !started.success() {
        return Err(anyhow!("updater rejected update request"));
    }
    Ok(())
}

fn update_status(auto_update_enabled: bool) -> UpdateStatus {
    let result = fs::read_to_string(UPDATE_RESULT)
        .ok()
        .and_then(|value| match value.trim() {
            "current" => Some(UpdateResult::Current),
            "updated" => Some(UpdateResult::Updated),
            "failed" => Some(UpdateResult::Failed),
            _ => None,
        });
    UpdateStatus {
        auto_update_enabled,
        running: Path::new(UPDATE_LOCK).exists() || Path::new(UPDATE_TRIGGER).exists(),
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn committed_write_observation_failure_is_additive_and_never_replayed() {
        use std::sync::{Arc, atomic::AtomicUsize};
        let fixture = crate::managed::tests::Fixture::new();
        let config = fixture.state.config.clone();
        // Fail load_status before it can inspect any real interface or service.
        let _ = std::thread::spawn(move || {
            let _guard = config.lock().unwrap();
            panic!("poison status configuration for test");
        })
        .join();
        let writes = Arc::new(AtomicUsize::new(0));
        let committed = writes.clone();
        let response = run_blocking(fixture.state.clone(), move |_| {
            committed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .err()
        .unwrap()
        .into_response();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["outcome"], "committed");
        assert!(!body["error"].as_str().unwrap().contains("poisoned"));
        assert_eq!(writes.load(Ordering::SeqCst), 1);

        for response in [
            run_blocking(fixture.state.clone(), |_| Err(anyhow!("write rejected")))
                .await
                .err()
                .unwrap()
                .into_response(),
            status(State(fixture.state.clone()))
                .await
                .err()
                .unwrap()
                .into_response(),
        ] {
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(body.get("outcome").is_none());
        }
    }

    #[test]
    fn degraded_routing_is_not_healthy_even_with_live_services() {
        for degraded in [false, true] {
            let health = Health {
                version: env!("CARGO_PKG_VERSION"),
                degraded,
                dns_active: true,
                dataplane_active: true,
                vpn_enabled: true,
                tunnel_active: true,
                handshake_age_seconds: Some(1),
            };
            assert_eq!(serde_json::to_value(&health).unwrap()["degraded"], degraded);
            assert_eq!(
                health.into_response().status(),
                if degraded {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                }
            );
        }
    }
}

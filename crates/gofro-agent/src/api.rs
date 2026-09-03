use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tracing::error;
use wireguard_status::wireguard_peers;

use crate::{
    AppState, controller, dataplane,
    model::{
        AP_ADDRESS, AP_DOMAIN, AgentStatus, ApInput, ApStatus, ModeInput, ProfileInput,
        RoutingConfig, RoutingStatus, RoutingTestInput, RoutingTestResult, ServerKeyInput,
        ServerProfile, ServerStatus, ServerUpdate, UpdateInput, UpdateResult, UpdateStatus,
    },
    network::service_active,
    stats, wifi,
};

const UI: &str = include_str!("../../../assets/index.html");
const UI_JS: &[u8] = include_bytes!("../../../assets/app.js");
const UI_JS_GZIP: &[u8] = include_bytes!("../../../assets/app.js.gz");
const UI_JS_HASH: &str = include_str!("../../../assets/app.js.sha256");
const UI_CSS: &[u8] = include_bytes!("../../../assets/app.css");
const UI_CSS_GZIP: &[u8] = include_bytes!("../../../assets/app.css.gz");
const UI_CSS_HASH: &str = include_str!("../../../assets/app.css.sha256");
const UI_CHART: &[u8] = include_bytes!("../../../assets/chart.js");
const UI_CHART_GZIP: &[u8] = include_bytes!("../../../assets/chart.js.gz");
const UI_CHART_HASH: &str = include_str!("../../../assets/chart.js.sha256");
const UPDATE_LOCK: &str = "/tmp/gofro-update.lock";
const UPDATE_RESULT: &str = "/tmp/gofro/update-result";
const UPDATE_TRIGGER: &str = "/tmp/gofro/update-request";
const UPDATE_COMMAND: &str = "/usr/libexec/gofro/update";

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
        .route("/chart.js", get(chart))
        .route("/api/status", get(status))
        .route("/api/update", post(start_update))
        .route("/api/mode", post(set_mode))
        .route(
            "/api/servers",
            post(add_server).put(update_server).delete(delete_server),
        )
        .route("/api/servers/import", post(import_server))
        .route("/api/servers/select", post(select_server))
        .route("/api/ap", post(update_ap))
        .route("/api/routing", post(update_routing))
        .route("/api/routing/test", post(test_routing))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(UI))
}

async fn javascript(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "application/javascript; charset=utf-8",
        UI_JS_HASH,
        UI_JS,
        UI_JS_GZIP,
    )
}

async fn stylesheet(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "text/css; charset=utf-8",
        UI_CSS_HASH,
        UI_CSS,
        UI_CSS_GZIP,
    )
}

async fn chart(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "application/javascript; charset=utf-8",
        UI_CHART_HASH,
        UI_CHART,
        UI_CHART_GZIP,
    )
}

fn asset_response(
    uri: &Uri,
    headers: &HeaderMap,
    content_type: &'static str,
    fingerprint: &str,
    identity: &'static [u8],
    gzip: &'static [u8],
) -> Response {
    let requested_fingerprint = uri.query().and_then(|query| query.strip_prefix("v="));
    let stale = requested_fingerprint.is_some_and(|requested| requested != fingerprint);
    let encoding = preferred_encoding(headers);
    let compressed = encoding == Some(AssetEncoding::Gzip);
    let mut response = Response::new(if stale || encoding.is_none() {
        Body::empty()
    } else {
        Body::from(if compressed { gzip } else { identity })
    });
    *response.status_mut() = if stale {
        StatusCode::NOT_FOUND
    } else if encoding.is_none() {
        StatusCode::NOT_ACCEPTABLE
    } else {
        StatusCode::OK
    };
    let response_headers = response.headers_mut();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(
            if requested_fingerprint == Some(fingerprint) && encoding.is_some() {
                "public, max-age=31536000, immutable"
            } else {
                "no-store"
            },
        ),
    );
    response_headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if !stale && encoding.is_some() && compressed {
        response_headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AssetEncoding {
    Identity,
    Gzip,
}

fn preferred_encoding(headers: &HeaderMap) -> Option<AssetEncoding> {
    if !headers.contains_key(header::ACCEPT_ENCODING) {
        return Some(AssetEncoding::Identity);
    }
    let mut gzip_quality: Option<f32> = None;
    let mut identity_quality: Option<f32> = None;
    let mut wildcard_quality: Option<f32> = None;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for coding in value.split(',') {
            let mut parts = coding.split(';');
            let encoding = parts.next().unwrap_or_default().trim();
            let mut quality = 1.0_f32;
            for parameter in parts {
                let Some((name, value)) = parameter.split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("q") {
                    quality = value
                        .trim()
                        .parse()
                        .ok()
                        .filter(|value| (0.0..=1.0).contains(value))
                        .unwrap_or(0.0);
                }
            }
            if encoding.eq_ignore_ascii_case("gzip") {
                gzip_quality = Some(gzip_quality.unwrap_or(0.0).max(quality));
            } else if encoding.eq_ignore_ascii_case("identity") {
                identity_quality = Some(identity_quality.unwrap_or(0.0).max(quality));
            } else if encoding == "*" {
                wildcard_quality = Some(wildcard_quality.unwrap_or(0.0).max(quality));
            }
        }
    }
    let gzip_quality = gzip_quality.or(wildcard_quality).unwrap_or(0.0);
    let identity_quality = identity_quality.unwrap_or_else(|| {
        if wildcard_quality == Some(0.0) {
            0.0
        } else {
            1.0
        }
    });
    if gzip_quality <= 0.0 && identity_quality <= 0.0 {
        None
    } else if gzip_quality >= identity_quality {
        Some(AssetEncoding::Gzip)
    } else {
        Some(AssetEncoding::Identity)
    }
}

async fn status(State(state): State<AppState>) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, |_| Ok(())).await
}

async fn start_update(
    State(state): State<AppState>,
    Json(_): Json<UpdateInput>,
) -> Result<Json<AgentStatus>, ApiError> {
    run_blocking(state, |_| queue_update()).await
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
    let networks = state
        .access_points
        .lock()
        .map_err(|_| anyhow!("access point lock poisoned"))?
        .clone();
    let ap_ssid = networks[0].ssid.clone();
    let (stats, history, devices) = state
        .stats
        .lock()
        .map_err(|_| anyhow!("statistics lock poisoned"))?
        .sample(stats::interface_traffic(&state.lan_interface), readings);

    Ok(AgentStatus {
        version: env!("CARGO_PKG_VERSION"),
        update: update_status(),
        vpn_enabled: config.vpn_enabled,
        tunnel_active,
        interface: state.interface.clone(),
        active_server_key: config.active_server_key,
        servers: config.servers.iter().map(ServerStatus::from).collect(),
        ap: ApStatus {
            ssid: ap_ssid,
            networks,
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

fn queue_update() -> Result<()> {
    if Path::new(UPDATE_LOCK).exists() || Path::new(UPDATE_TRIGGER).exists() {
        return Ok(());
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

fn update_status() -> UpdateStatus {
    let result = fs::read_to_string(UPDATE_RESULT)
        .ok()
        .and_then(|value| match value.trim() {
            "current" => Some(UpdateResult::Current),
            "updated" => Some(UpdateResult::Updated),
            "failed" => Some(UpdateResult::Failed),
            _ => None,
        });
    UpdateStatus {
        running: Path::new(UPDATE_LOCK).exists() || Path::new(UPDATE_TRIGGER).exists(),
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_versioned_compressed_assets_with_safe_caching() {
        let uri = "/app.js?v=current".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0, *;q=1"),
        );
        let response = asset_response(
            &"/app.js?v=old".parse().unwrap(),
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0.1, identity;q=1"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0, identity;q=0"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

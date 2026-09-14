use anyhow::{Context, Result, anyhow};
use axum::{
    Json,
    body::Body,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use super::{ApiError, load_status};
use crate::{
    AppState,
    model::{AgentStatus, ServerKeyInput},
};

#[derive(serde::Deserialize)]
pub(super) struct ProbeInput {
    host: String,
    port: u16,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BootstrapInput {
    name: String,
    host: String,
    port: u16,
    password: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostPinInput {
    host: String,
    port: u16,
}
#[derive(Serialize)]
pub(super) struct ProbeResult {
    host: String,
    port: u16,
    host_key: String,
    fingerprint: String,
}
#[derive(Serialize)]
pub(super) struct ManagedVersion {
    version: String,
    update_available: bool,
}
#[derive(Serialize)]
pub(super) struct CreatedProfile {
    profile: String,
}

#[derive(serde::Deserialize)]
pub(super) struct CreateFriendInput {
    public_key: String,
    name: String,
}

#[derive(serde::Deserialize)]
pub(super) struct RenameFriendInput {
    public_key: String,
    peer_key: String,
    name: String,
}

#[derive(serde::Deserialize)]
pub(super) struct FriendProfileInput {
    public_key: String,
    peer_key: String,
}

pub(super) async fn probe_server(
    Json(input): Json<ProbeInput>,
) -> Result<Json<ProbeResult>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::probe(input.host.trim().to_owned(), input.port)
    })
    .await
    .context("probe task failed")
    .map_err(ApiError)?
    .map(|probe| {
        Json(ProbeResult {
            host: probe.host,
            port: probe.port,
            host_key: probe.host_key,
            fingerprint: probe.fingerprint,
        })
    })
    .map_err(ApiError)
}

pub(super) async fn bootstrap_server(
    State(state): State<AppState>,
    Json(input): Json<BootstrapInput>,
) -> Response {
    bootstrap_stream(move |progress| {
        crate::managed::bootstrap(
            &state,
            input.name.trim().to_owned(),
            input.host.trim().to_owned(),
            input.port,
            input.password,
            progress,
        )?;
        load_status(&state).map_err(|_| anyhow!("VPS enrolled, but local status could not be read. Refresh the server list before retrying."))
    })
}

pub(super) async fn reset_host_pin(
    State(state): State<AppState>,
    Json(input): Json<HostPinInput>,
) -> Result<Json<bool>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::reset_host_pin(&state, input.host.trim(), input.port)
    })
    .await
    .context("host pin reset task failed")
    .map_err(ApiError)?
    .map_err(ApiError)?;
    Ok(Json(true))
}

fn bootstrap_stream(
    operation: impl FnOnce(&mut dyn FnMut(crate::model::BootstrapStage)) -> Result<AgentStatus>
    + Send
    + 'static,
) -> Response {
    use crate::model::{BootstrapEvent, BootstrapStage};
    // Only a fixed number of stage events and one terminal event are produced.
    // Delivery must never block or cancel installation when the browser goes away.
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || {
        let mut stage = BootstrapStage::Waiting;
        let mut progress = |next| {
            stage = next;
            let _ = sender.send(BootstrapEvent::Stage { stage });
        };
        let terminal = match operation(&mut progress) {
            Ok(status) => BootstrapEvent::Complete {
                status: Box::new(status),
            },
            Err(error) => BootstrapEvent::Error {
                stage,
                message: error.to_string(),
            },
        };
        let _ = sender.send(terminal);
    });
    let stream = futures_util::stream::poll_fn(move |cx| {
        receiver.poll_recv(cx).map(|event| {
            event.map(|event| {
                serde_json::to_vec(&event).map(|mut line| {
                    line.push(b'\n');
                    line
                })
            })
        })
    });
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

pub(super) async fn check_managed_server(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<ManagedVersion>, ApiError> {
    managed_version(state, input.public_key).await
}

pub(super) async fn update_managed_server(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<ManagedVersion>, ApiError> {
    tokio::task::spawn_blocking(move || crate::managed::update(&state, input.public_key.trim()))
        .await
        .context("managed update task failed")
        .map_err(ApiError)?
        .map(|version| {
            Json(ManagedVersion {
                version: version.version,
                update_available: version.update_available,
            })
        })
        .map_err(ApiError)
}

async fn managed_version(
    state: AppState,
    public_key: String,
) -> Result<Json<ManagedVersion>, ApiError> {
    tokio::task::spawn_blocking(move || crate::managed::check(&state, public_key.trim()))
        .await
        .context("managed check task failed")
        .map_err(ApiError)?
        .map(|version| {
            Json(ManagedVersion {
                version: version.version,
                update_available: version.update_available,
            })
        })
        .map_err(ApiError)
}

pub(super) async fn create_managed_profile(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<CreatedProfile>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::create_profile(&state, input.public_key.trim())
    })
    .await
    .context("create profile task failed")
    .map_err(ApiError)?
    .map(|profile| Json(CreatedProfile { profile }))
    .map_err(ApiError)
}

pub(super) async fn managed_server_status(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    managed_status(state, input.public_key).await
}

pub(super) async fn restart_managed_server(
    State(state): State<AppState>,
    Json(input): Json<ServerKeyInput>,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    tokio::task::spawn_blocking(move || crate::managed::restart(&state, input.public_key.trim()))
        .await
        .context("managed restart task failed")
        .map_err(ApiError)?
        .map(Json)
        .map_err(ApiError)
}

pub(super) async fn create_friend(
    State(state): State<AppState>,
    Json(input): Json<CreateFriendInput>,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::create_friend(&state, &input.public_key, &input.name)
    })
    .await
    .context("create friend task failed")
    .map_err(ApiError)?
    .map(Json)
    .map_err(ApiError)
}

pub(super) async fn rename_friend(
    State(state): State<AppState>,
    Json(input): Json<RenameFriendInput>,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::rename_friend(&state, &input.public_key, &input.peer_key, &input.name)
    })
    .await
    .context("rename friend task failed")
    .map_err(ApiError)?
    .map(Json)
    .map_err(ApiError)
}

pub(super) async fn revoke_friend(
    State(state): State<AppState>,
    Json(input): Json<FriendProfileInput>,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::revoke_friend(&state, &input.public_key, &input.peer_key)
    })
    .await
    .context("revoke friend task failed")
    .map_err(ApiError)?
    .map(Json)
    .map_err(ApiError)
}

pub(super) async fn friend_profile(
    State(state): State<AppState>,
    Json(input): Json<FriendProfileInput>,
) -> Result<Json<CreatedProfile>, ApiError> {
    tokio::task::spawn_blocking(move || {
        crate::managed::friend_profile(&state, &input.public_key, &input.peer_key)
    })
    .await
    .context("friend profile task failed")
    .map_err(ApiError)?
    .map(|profile| Json(CreatedProfile { profile }))
    .map_err(ApiError)
}

async fn managed_status(
    state: AppState,
    public_key: String,
) -> Result<Json<wireguard_status::managed::ManagedServerStatus>, ApiError> {
    tokio::task::spawn_blocking(move || crate::managed::status(&state, public_key.trim()))
        .await
        .context("managed status task failed")
        .map_err(ApiError)?
        .map(Json)
        .map_err(ApiError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RoutingStatus, UpdateStatus};
    use std::sync::atomic::Ordering;

    fn bootstrap_status() -> AgentStatus {
        AgentStatus {
            device_exclusions: vec![],
            version: env!("CARGO_PKG_VERSION"),
            update: UpdateStatus {
                auto_update_enabled: false,
                running: false,
                result: None,
            },
            vpn_enabled: false,
            tunnel_active: false,
            interface: "gt0".into(),
            active_server_key: None,
            servers: vec![],
            peer: None,
            stats: Default::default(),
            history: vec![],
            routing: RoutingStatus {
                config: Default::default(),
                dns_active: false,
                fake_ips: 0,
                geosite_loaded: false,
                geoip_loaded: false,
                dataplane_active: false,
                degraded: false,
            },
        }
    }

    #[tokio::test]
    async fn bootstrap_stream_emits_live_stages_then_exact_terminal_contract() {
        use crate::model::BootstrapStage;
        use futures_util::StreamExt;
        let (release, wait) = std::sync::mpsc::channel();
        let response = bootstrap_stream(move |progress| {
            progress(BootstrapStage::Waiting);
            wait.recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            progress(BootstrapStage::Save);
            Ok(bootstrap_status())
        });
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/x-ndjson"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let mut stream = response.into_body().into_data_stream();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(
            first.as_ref(),
            b"{\"type\":\"stage\",\"stage\":\"waiting\"}\n"
        );
        release.send(()).unwrap();
        let save = stream.next().await.unwrap().unwrap();
        assert_eq!(save.as_ref(), b"{\"type\":\"stage\",\"stage\":\"save\"}\n");
        let complete: serde_json::Value =
            serde_json::from_slice(&stream.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(complete["type"], "complete");
        assert_eq!(
            complete["status"],
            serde_json::to_value(bootstrap_status()).unwrap()
        );
        assert!(stream.next().await.is_none());

        let response = bootstrap_stream(|progress| {
            progress(BootstrapStage::Inspect);
            Err(anyhow!("safe preflight failure"))
        });
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let lines: Vec<serde_json::Value> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines,
            vec![
                serde_json::json!({"type":"stage", "stage":"inspect"}),
                serde_json::json!({"type":"error", "stage":"inspect", "message":"safe preflight failure"}),
            ]
        );
        let input: BootstrapInput = serde_json::from_str(
            r#"{"name":"VPS","host":"1.1.1.1","port":22,"password":"secret"}"#,
        )
        .unwrap();
        assert_eq!(input.port, 22);
        assert!(serde_json::from_str::<BootstrapInput>(
            r#"{"name":"VPS","host":"1.1.1.1","port":22,"password":"secret","host_key":"browser-key"}"#
        ).is_err());
    }

    #[tokio::test]
    async fn bootstrap_disconnect_does_not_cancel_commit_or_replay_operation() {
        use crate::model::BootstrapStage;
        use futures_util::StreamExt;
        use std::sync::{Arc, atomic::AtomicUsize};
        let calls = Arc::new(AtomicUsize::new(0));
        let started = calls.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let (committed, finished) = tokio::sync::oneshot::channel();
        let response = bootstrap_stream(move |progress| {
            started.fetch_add(1, Ordering::SeqCst);
            progress(BootstrapStage::Install);
            wait.recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            progress(BootstrapStage::Authorize);
            progress(BootstrapStage::Profile);
            progress(BootstrapStage::Save);
            committed.send(()).unwrap();
            Ok(bootstrap_status())
        });
        let mut stream = response.into_body().into_data_stream();
        assert!(stream.next().await.is_some());
        drop(stream);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), finished)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn friend_request_bodies_require_only_the_fields_the_route_uses() {
        let create: CreateFriendInput =
            serde_json::from_str(r#"{"public_key":"server","name":"Friend"}"#).unwrap();
        assert_eq!(create.name, "Friend");
        assert!(
            serde_json::from_str::<RenameFriendInput>(r#"{"public_key":"server","name":"Friend"}"#)
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(CreatedProfile {
                profile: "profile".into()
            })
            .unwrap(),
            serde_json::json!({"profile":"profile"})
        );
    }
}

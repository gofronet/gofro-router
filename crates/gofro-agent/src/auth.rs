use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use openssl::{memcmp, pkcs5, rand::rand_bytes};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::{AppState, onboarding};

const SESSION: &str = "__Host-gofro-session";
const CSRF: &str = "__Host-gofro-csrf";
const TTL: Duration = Duration::from_secs(1800);

pub(crate) struct Auth {
    password: PathBuf,
    setup_code: PathBuf,
    inner: Mutex<Inner>,
    hashing: Arc<Semaphore>,
}
struct Inner {
    sessions: Vec<Session>,
    failed: Option<Instant>,
}
struct Session {
    token: String,
    csrf: String,
    expires: Instant,
}
#[derive(Deserialize)]
pub(crate) struct PasswordInput {
    setup_code: Option<String>,
    password: String,
}
#[derive(Serialize)]
struct AuthReply {
    state: &'static str,
    csrf_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_method: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_window_seconds: Option<u64>,
}

impl Auth {
    pub(crate) fn open(password: PathBuf, setup_code: PathBuf) -> Result<Self> {
        if password.exists() {
            read_record(&password)?;
        }
        Ok(Self {
            password,
            setup_code,
            inner: Mutex::new(Inner {
                sessions: vec![],
                failed: None,
            }),
            hashing: Arc::new(Semaphore::new(1)),
        })
    }
    fn setup(&self) -> bool {
        !self.password.exists()
    }
    pub(crate) fn configured(&self) -> bool {
        self.password.exists()
    }
    fn issue(&self) -> Result<(String, String)> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("auth lock poisoned"))?;
        inner.sessions.retain(|item| item.expires > Instant::now());
        while inner.sessions.len() >= 32 {
            inner.sessions.remove(0);
        }
        let session_token = token()?;
        let csrf = token()?;
        inner.sessions.push(Session {
            token: session_token.clone(),
            csrf: csrf.clone(),
            expires: Instant::now() + TTL,
        });
        Ok((session_token, csrf))
    }
}

pub(crate) async fn status(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if !allowed_host(&uri, &headers) {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    let csrf = if state.auth.setup() {
        token()
    } else {
        session(&state.auth, &headers)
            .map(|item| item.1)
            .or_else(|_| token())
    };
    match csrf {
        Ok(csrf) => {
            let setup = state.auth.setup();
            let fresh = if setup {
                match onboarding::fresh_admin(&state) {
                    Ok(value) => value,
                    Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
                }
            } else {
                false
            };
            reply(
                if setup {
                    "setup"
                } else if session(&state.auth, &headers).is_ok() {
                    "authenticated"
                } else {
                    "login"
                },
                csrf,
                None,
                setup.then_some(if fresh { "local" } else { "wifi_password" }),
                fresh.then(|| onboarding::setup_window_seconds(&state)),
            )
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

pub(crate) async fn setup(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    Json(input): Json<PasswordInput>,
) -> Response {
    if !preauth(&uri, &headers) {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    if let Some(code) = setup_password_error(&input.password) {
        return error(StatusCode::BAD_REQUEST, code);
    }
    let Ok(hashing) = state.auth.hashing.clone().try_acquire_owned() else {
        return error(StatusCode::CONFLICT, "auth_busy");
    };
    if !state.auth.setup() {
        return error(StatusCode::CONFLICT, "setup_completed");
    }
    let fresh = match onboarding::fresh_admin(&state) {
        Ok(value) => value,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    if fresh {
        if onboarding::setup_window_seconds(&state) == 0 {
            return error(StatusCode::FORBIDDEN, "setup_closed");
        }
    } else {
        let Ok(code) = fs::read_to_string(&state.auth.setup_code) else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "setup_unavailable");
        };
        if !input
            .setup_code
            .as_deref()
            .is_some_and(|input| setup_code_matches(input, &code))
        {
            return error(StatusCode::FORBIDDEN, "invalid_setup_code");
        }
    }
    let auth = state.auth.clone();
    let state_for_write = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _hashing = hashing;
        write_record(&auth.password, &input.password)?;
        if fresh {
            onboarding::write_wifi(&state_for_write)?;
        } else {
            fs::remove_file(&auth.setup_code)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    }
    match state.auth.issue() {
        Ok((token, csrf)) => reply("authenticated", csrf, Some(token), None, None),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

pub(crate) async fn login(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    Json(input): Json<PasswordInput>,
) -> Response {
    if !preauth(&uri, &headers) {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    if password_too_long(&input.password) {
        return error(StatusCode::BAD_REQUEST, "password_too_long");
    }
    if state.auth.setup() {
        return error(StatusCode::CONFLICT, "setup_required");
    }
    let Ok(hashing) = state.auth.hashing.clone().try_acquire_owned() else {
        return error(StatusCode::CONFLICT, "auth_busy");
    };
    {
        let Ok(inner) = state.auth.inner.lock() else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        };
        if inner
            .failed
            .is_some_and(|time| time.elapsed() < Duration::from_secs(1))
        {
            return error(StatusCode::TOO_MANY_REQUESTS, "login_throttled");
        }
    }
    let auth = state.auth.clone();
    let verified = tokio::task::spawn_blocking(move || {
        (
            read_record(&auth.password)
                .and_then(|record| verify(&record, &input.password))
                .unwrap_or(false),
            hashing,
        )
    })
    .await;
    let (valid, hashing) = match verified {
        Ok(result) => result,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    if !valid {
        let Ok(mut inner) = state.auth.inner.lock() else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        };
        inner.failed = Some(Instant::now());
        drop(hashing);
        return error(StatusCode::UNAUTHORIZED, "invalid_password");
    }
    drop(hashing);
    match state.auth.issue() {
        Ok((token, csrf)) => reply("authenticated", csrf, Some(token), None, None),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Ok((token, csrf)) = session(&state.auth, &headers) {
        if headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            != Some(&csrf)
        {
            return error(StatusCode::FORBIDDEN, "request_rejected");
        }
        let Ok(mut inner) = state.auth.inner.lock() else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        };
        inner.sessions.retain(|item| item.token != token);
    }
    let mut response = reply("login", token().unwrap_or_default(), None, None, None);
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-gofro-session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    response
}

pub(crate) async fn require_auth(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let headers = request.headers();
    if !allowed_host(request.uri(), headers) {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    let Ok((_, csrf)) = session(&state.auth, headers) else {
        return error(StatusCode::UNAUTHORIZED, "session_expired");
    };
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) && (!allowed_origin(headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|token| constant_time_eq(token, &csrf)))
    {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    let Ok(step) = onboarding::step(&state) else {
        return error(StatusCode::FORBIDDEN, "onboarding_required");
    };
    if matches!(
        step,
        onboarding::Step::Admin | onboarding::Step::Wifi | onboarding::Step::WifiApplying
    ) && !matches!(
        (request.method(), request.uri().path()),
        (&Method::GET, "/api/status")
            | (_, "/api/auth/logout")
            | (_, "/api/onboarding")
            | (_, "/api/onboarding/wifi")
            | (_, "/api/onboarding/complete")
    ) {
        return error(StatusCode::CONFLICT, "onboarding_required");
    }
    next.run(request).await
}

fn session(auth: &Auth, headers: &HeaderMap) -> Result<(String, String)> {
    let token = cookie(headers, SESSION).context("missing session")?;
    let mut inner = auth
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("auth lock poisoned"))?;
    inner.sessions.retain(|item| item.expires > Instant::now());
    let session = inner
        .sessions
        .iter()
        .find(|item| item.token == token)
        .context("invalid session")?;
    Ok((session.token.clone(), session.csrf.clone()))
}
fn preauth(uri: &Uri, headers: &HeaderMap) -> bool {
    allowed_host(uri, headers)
        && allowed_origin(headers)
        && cookie(headers, CSRF)
            .zip(headers.get("x-csrf-token").and_then(|v| v.to_str().ok()))
            .is_some_and(|(a, b)| constant_time_eq(&a, b))
}
fn setup_password_error(password: &str) -> Option<&'static str> {
    if password.chars().count() < 12 {
        Some("password_too_short")
    } else if password_too_long(password) {
        Some("password_too_long")
    } else {
        None
    }
}
fn password_too_long(password: &str) -> bool {
    password.len() > 128
}
fn allowed_host(uri: &Uri, headers: &HeaderMap) -> bool {
    let authority = uri.authority().map(|value| value.as_str());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if authority.is_some() && host.is_some() && authority != host {
        return false;
    }
    matches!(
        authority.or(host),
        Some("wifi.gofro.net") | Some("10.203.1.1")
    )
}
fn allowed_origin(headers: &HeaderMap) -> bool {
    matches!(
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        Some("https://wifi.gofro.net") | Some("https://10.203.1.1")
    )
}
pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for part in value.to_str().ok()?.split(';') {
            let (key, value) = part.trim().split_once('=')?;
            if key == name && found.replace(value.to_owned()).is_some() {
                return None;
            }
        }
    }
    found
}
fn reply(
    state: &'static str,
    csrf: String,
    session: Option<String>,
    setup_method: Option<&'static str>,
    setup_window_seconds: Option<u64>,
) -> Response {
    let mut response = Json(AuthReply {
        state,
        csrf_token: csrf.clone(),
        setup_method,
        setup_window_seconds,
    })
    .into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{CSRF}={csrf}; Path=/; Secure; HttpOnly; SameSite=Strict"
        ))
        .unwrap(),
    );
    if let Some(token) = session {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&format!(
                "{SESSION}={token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=1800"
            ))
            .unwrap(),
        );
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn error(status: StatusCode, code: &'static str) -> Response {
    let mut response = (status, Json(serde_json::json!({"error":code}))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn token() -> Result<String> {
    let mut bytes = [0; 32];
    rand_bytes(&mut bytes)?;
    Ok(hex(&bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>> {
    if !value.is_ascii() || !value.len().is_multiple_of(2) {
        bail!("bad credential");
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).context("bad credential"))
        .collect()
}
fn read_record(path: &PathBuf) -> Result<(Vec<u8>, Vec<u8>)> {
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o077 != 0 || metadata.len() > 512 {
        bail!("credential permissions are unsafe");
    }
    let value = fs::read_to_string(path)?;
    let parts: Vec<_> = value.split('$').collect();
    if parts.len() != 6 || parts[..4] != ["gofro-scrypt-v1", "16384", "8", "1"] {
        bail!("malformed credential");
    }
    let salt = unhex(parts[4])?;
    let hash = unhex(parts[5])?;
    if salt.len() != 32 || hash.len() != 32 {
        bail!("malformed credential");
    }
    Ok((salt, hash))
}
fn verify(record: &(Vec<u8>, Vec<u8>), password: &str) -> Result<bool> {
    let mut actual = [0; 32];
    pkcs5::scrypt(
        password.as_bytes(),
        &record.0,
        16384,
        8,
        1,
        64 * 1024 * 1024,
        &mut actual,
    )?;
    Ok(memcmp::eq(&actual, &record.1))
}
fn write_record(path: &PathBuf, password: &str) -> Result<()> {
    let mut salt = [0; 32];
    let mut hash = [0; 32];
    rand_bytes(&mut salt)?;
    pkcs5::scrypt(
        password.as_bytes(),
        &salt,
        16384,
        8,
        1,
        64 * 1024 * 1024,
        &mut hash,
    )?;
    let temporary = path.with_extension("new");
    let record = [
        "gofro-scrypt-v1".to_owned(),
        "16384".to_owned(),
        "8".to_owned(),
        "1".to_owned(),
        hex(&salt),
        hex(&hash),
    ]
    .join("$");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(record.as_bytes())?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    fs::File::open(path.parent().context("missing credential parent")?)?.sync_all()?;
    Ok(())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len() && memcmp::eq(left.as_bytes(), right.as_bytes())
}
fn setup_code_matches(input: &str, codes: &str) -> bool {
    codes.lines().any(|code| constant_time_eq(input, code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fake_dns::FakeDns,
        geodata::GeoData,
        model::{ControllerConfig, RoutingConfig},
        routing::RoutingPolicy,
        stats::StatsTracker,
    };
    use tower::ServiceExt;

    fn test_state(dir: &std::path::Path, password: PathBuf, setup_code: PathBuf) -> AppState {
        let geodata = Arc::new(GeoData::default());
        AppState {
            interface: "eth0".into(),
            lan_interface: "eth0".into(),
            wifi_interface: "wlan0".into(),
            config_path: dir.join("config.json"),
            mode_command: dir.join("mode"),
            management_dir: dir.join("management"),
            config: Arc::new(Mutex::new(ControllerConfig {
                vpn_enabled: false,
                active_server_key: None,
                servers: vec![],
                routing: RoutingConfig {
                    domain_rules: vec![],
                    ip_rules: vec![],
                    default_target: crate::model::RouteTarget::Vpn,
                    mode: crate::model::RoutingMode::Rules,
                    rule_order: None,
                },
            })),
            access_points: Arc::new(Mutex::new(vec![])),
            stats: Arc::new(Mutex::new(StatsTracker::default())),
            routing: Arc::new(std::sync::RwLock::new(
                RoutingPolicy::compile(
                    RoutingConfig {
                        domain_rules: vec![],
                        ip_rules: vec![],
                        default_target: crate::model::RouteTarget::Vpn,
                        mode: crate::model::RoutingMode::Rules,
                        rule_order: None,
                    },
                    Arc::clone(&geodata),
                )
                .unwrap(),
            )),
            routing_degraded: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            geodata,
            fake_dns: Arc::new(FakeDns::open(&dir.join("routing.sqlite")).unwrap()),
            auth: Arc::new(Auth::open(password, setup_code).unwrap()),
            managed_operations: Arc::new(Mutex::new(())),
        }
    }
    fn preauth_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("wifi.gofro.net"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://wifi.gofro.net"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-gofro-csrf=token"),
        );
        headers.insert("x-csrf-token", HeaderValue::from_static("token"));
        headers
    }
    async fn setup_response(
        state: &AppState,
        uri: &Uri,
        headers: &HeaderMap,
        setup_code: Option<&str>,
        password: &str,
    ) -> Response {
        setup(
            State(state.clone()),
            uri.clone(),
            headers.clone(),
            Json(PasswordInput {
                setup_code: setup_code.map(str::to_owned),
                password: password.to_owned(),
            }),
        )
        .await
    }
    async fn login_response(
        state: &AppState,
        uri: &Uri,
        headers: &HeaderMap,
        password: &str,
    ) -> Response {
        login(
            State(state.clone()),
            uri.clone(),
            headers.clone(),
            Json(PasswordInput {
                setup_code: None,
                password: password.to_owned(),
            }),
        )
        .await
    }

    #[test]
    fn rejects_duplicate_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-gofro-csrf=a; __Host-gofro-csrf=b"),
        );
        assert!(cookie(&headers, CSRF).is_none());
    }
    #[test]
    fn accepts_http2_authority_without_host_header() {
        assert!(allowed_host(
            &"https://wifi.gofro.net/api/auth/status".parse().unwrap(),
            &HeaderMap::new()
        ));
    }
    #[test]
    fn rejects_conflicting_authority_and_host_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("10.203.1.1"));
        assert!(!allowed_host(
            &"https://wifi.gofro.net/api/auth/status".parse().unwrap(),
            &headers
        ));
    }
    #[test]
    fn accepts_each_migrated_access_point_password() {
        assert!(setup_code_matches("second", "first\nsecond\n"));
    }
    #[test]
    fn validates_setup_password_character_and_byte_limits() {
        assert_eq!(
            setup_password_error(&"a".repeat(11)),
            Some("password_too_short")
        );
        assert_eq!(setup_password_error(&"a".repeat(12)), None);
        assert_eq!(setup_password_error(&"a".repeat(128)), None);
        assert_eq!(
            setup_password_error(&"a".repeat(129)),
            Some("password_too_long")
        );
        assert_eq!(setup_password_error(&"é".repeat(12)), None);
        assert_eq!(
            setup_password_error(&"😀".repeat(33)),
            Some("password_too_long")
        );
    }
    #[test]
    fn preauth_requires_matching_csrf_token() {
        let uri: Uri = "https://wifi.gofro.net/api/auth/setup".parse().unwrap();
        let mut headers = preauth_headers();
        assert!(preauth(&uri, &headers));
        headers.insert("x-csrf-token", HeaderValue::from_static("wrong"));
        assert!(!preauth(&uri, &headers));
    }
    #[tokio::test]
    async fn handlers_enforce_setup_password_and_login_flow() {
        let dir = std::env::temp_dir().join(format!(
            "gofro-auth-handler-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let password_path = dir.join("admin-password");
        let setup_code = dir.join("setup-code");
        fs::write(&setup_code, "setup-code\n").unwrap();
        let state = test_state(&dir, password_path.clone(), setup_code);
        let uri: Uri = "https://wifi.gofro.net/api/auth/setup".parse().unwrap();
        let mut headers = preauth_headers();
        let short = setup_response(&state, &uri, &headers, Some("setup-code"), "short").await;
        assert_eq!(short.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            axum::body::to_bytes(short.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            br#"{"error":"password_too_short"}"#
        );
        assert!(!password_path.exists());
        let password = "a valid password";
        assert_eq!(
            setup_response(&state, &uri, &headers, Some("setup-code"), password)
                .await
                .status(),
            StatusCode::OK
        );
        assert!(
            fs::read_to_string(&password_path)
                .unwrap()
                .starts_with("gofro-scrypt-v1$")
        );
        assert_eq!(
            login_response(&state, &uri, &headers, password)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            login_response(&state, &uri, &headers, "wrong password")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        headers.insert("x-csrf-token", HeaderValue::from_static("wrong"));
        assert_eq!(
            login_response(&state, &uri, &headers, password)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn reboot_route_rejects_missing_session_and_csrf() {
        let dir = std::env::temp_dir().join(format!("gofro-reboot-auth-{}", token().unwrap()));
        fs::create_dir(&dir).unwrap();
        let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
        let request = Request::post("/api/reboot")
            .header(header::HOST, "wifi.gofro.net")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            crate::api::secure_router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let (session, csrf) = state.auth.issue().unwrap();
        let request = Request::post("/api/reboot")
            .header(header::HOST, "wifi.gofro.net")
            .header(header::ORIGIN, "https://wifi.gofro.net")
            .header(header::COOKIE, format!("{SESSION}={session}"))
            .header("x-csrf-token", format!("wrong-{csrf}"))
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            crate::api::secure_router(state)
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn managed_routes_reject_missing_sessions_and_bad_csrf_before_ssh() {
        let dir = std::env::temp_dir().join(format!("gofro-managed-auth-{}", token().unwrap()));
        fs::create_dir(&dir).unwrap();
        let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
        for (method, path) in [
            (Method::POST, "/api/servers/management"),
            (Method::POST, "/api/servers/restart"),
            (Method::POST, "/api/servers/friends"),
            (Method::PUT, "/api/servers/friends"),
            (Method::DELETE, "/api/servers/friends"),
            (Method::POST, "/api/servers/friends/profile"),
        ] {
            let request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::HOST, "wifi.gofro.net")
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                crate::api::secure_router(state.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let (session, csrf) = state.auth.issue().unwrap();
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, "wifi.gofro.net")
                .header(header::ORIGIN, "https://wifi.gofro.net")
                .header(header::COOKIE, format!("{SESSION}={session}"))
                .header("x-csrf-token", format!("wrong-{csrf}"))
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                crate::api::secure_router(state.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let (session, csrf) = state.auth.issue().unwrap();
        let request = Request::post("/api/servers/friends")
            .header(header::HOST, "wifi.gofro.net")
            .header(header::ORIGIN, "https://wifi.gofro.net")
            .header(header::COOKIE, format!("{SESSION}={session}"))
            .header("x-csrf-token", csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                r#"{"public_key":"Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=","name":"Friend"}"#,
            ))
            .unwrap();
        // The body is accepted without peer_key; the unmanaged server is rejected before SSH.
        assert_eq!(
            crate::api::secure_router(state.clone())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn incomplete_onboarding_cannot_fall_back_to_a_legacy_code() {
        let dir = std::env::temp_dir().join(format!("gofro-claim-closed-{}", token().unwrap()));
        fs::create_dir(&dir).unwrap();
        let code = dir.join("ap-password");
        fs::write(&code, "old-wifi-password").unwrap();
        let password_path = dir.join("admin-password");
        let state = test_state(&dir, password_path.clone(), code);
        let uri = "https://wifi.gofro.net/api/auth/setup".parse().unwrap();
        let marker = dir.join("onboarding-state");
        for phase in ["wifi\n", "wifi_applying\n", "server\n", "invalid\n"] {
            fs::write(&marker, phase).unwrap();
            fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
            let response = setup_response(
                &state,
                &uri,
                &preauth_headers(),
                Some("old-wifi-password"),
                "a valid password",
            )
            .await;
            assert_ne!(response.status(), StatusCode::OK);
            assert!(!password_path.exists());
        }
        fs::write(&marker, "admin\n").unwrap();
        let response = setup_response(
            &state,
            &uri,
            &preauth_headers(),
            Some("old-wifi-password"),
            "a valid password",
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!password_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn fresh_setup_claims_one_admin_and_resumes_wifi() {
        let dir = std::env::temp_dir().join(format!("gofro-fresh-claim-{}", token().unwrap()));
        fs::create_dir(&dir).unwrap();
        let password_path = dir.join("admin-password");
        let state = test_state(&dir, password_path.clone(), dir.join("no-legacy-code"));
        let marker = dir.join("onboarding-state");
        fs::write(&marker, "admin\n").unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
        let uptime = fs::read_to_string("/proc/uptime").unwrap();
        let now: u64 = uptime.split('.').next().unwrap().parse().unwrap();
        let window = dir.join("onboarding-window");
        fs::write(&window, format!("{} {}\n", boot.trim(), now + 900)).unwrap();
        fs::set_permissions(&window, fs::Permissions::from_mode(0o600)).unwrap();
        let uri = "https://wifi.gofro.net/api/auth/setup".parse().unwrap();
        let headers = preauth_headers();
        let password = "a fresh admin password";
        let (first, second) = tokio::join!(
            setup_response(&state, &uri, &headers, None, password),
            setup_response(&state, &uri, &headers, None, password),
        );
        assert_eq!(
            [first.status(), second.status()]
                .iter()
                .filter(|status| **status == StatusCode::OK)
                .count(),
            1
        );
        assert!(verify(&read_record(&password_path).unwrap(), password).unwrap());
        assert_eq!(onboarding::step(&state).unwrap(), onboarding::Step::Wifi);
        assert_eq!(fs::read_to_string(&marker).unwrap(), "wifi\n");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn verifies_record() {
        let path = std::env::temp_dir().join("gofro-auth-test");
        write_record(&path, "a long enough password").unwrap();
        assert!(verify(&read_record(&path).unwrap(), "a long enough password").unwrap());
        assert!(!verify(&read_record(&path).unwrap(), "wrong").unwrap());
        fs::remove_file(path).unwrap();
    }
}

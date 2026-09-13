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
    if !allowed_host(&state, &uri, &headers) {
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
            let window = setup.then(|| onboarding::setup_window_seconds(&state));
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
                setup.then_some("code"),
                window,
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
    if !preauth(&state, &uri, &headers) {
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
    if onboarding::setup_window_seconds(&state) == 0 {
        return error(StatusCode::FORBIDDEN, "setup_closed");
    }
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
    let auth = state.auth.clone();
    let state_for_write = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _hashing = hashing;
        write_record(&auth.password, &input.password)?;
        onboarding::complete_admin(&state_for_write, &auth.setup_code)
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
    if !preauth(&state, &uri, &headers) {
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
    let state_for_write = state.clone();
    let verified = tokio::task::spawn_blocking(move || {
        let valid = read_record(&auth.password)
            .and_then(|record| verify(&record, &input.password))
            .unwrap_or(false);
        let result = if valid {
            onboarding::complete_admin(&state_for_write, &auth.setup_code).map(|()| true)
        } else {
            Ok(false)
        };
        (result, hashing)
    })
    .await;
    let (valid, hashing) = match verified {
        Ok((Ok(valid), hashing)) => (valid, hashing),
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
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
    if !allowed_host(&state, request.uri(), headers) {
        return error(StatusCode::FORBIDDEN, "request_rejected");
    }
    let Ok((_, csrf)) = session(&state.auth, headers) else {
        return error(StatusCode::UNAUTHORIZED, "session_expired");
    };
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) && (!allowed_origin(&state, headers)
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
    if matches!(step, onboarding::Step::Admin)
        && !matches!(
            (request.method(), request.uri().path()),
            (&Method::GET, "/api/status")
                | (_, "/api/auth/logout")
                | (_, "/api/onboarding")
                | (_, "/api/onboarding/complete")
        )
    {
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
fn preauth(state: &AppState, uri: &Uri, headers: &HeaderMap) -> bool {
    allowed_host(state, uri, headers)
        && allowed_origin(state, headers)
        && cookie(headers, CSRF)
            .zip(headers.get("x-csrf-token").and_then(|v| v.to_str().ok()))
            .is_some_and(|(a, b)| constant_time_eq(&a, b))
}
fn setup_password_error(password: &str) -> Option<&'static str> {
    if password.chars().count() < 8 {
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
fn allowed_host(state: &AppState, uri: &Uri, headers: &HeaderMap) -> bool {
    let authority = uri.authority().map(|value| value.as_str());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if authority.is_some() && host.is_some() && authority != host {
        return false;
    }
    authority.or(host).is_some_and(|value| {
        value == format!("{}:{}", state.lan.address, state.https_listen.port())
            || value == format!("{}:{}", crate::model::AP_DOMAIN, state.https_listen.port())
    })
}
fn allowed_origin(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value
                == format!(
                    "https://{}:{}",
                    state.lan.address,
                    state.https_listen.port()
                )
                || value
                    == format!(
                        "https://{}:{}",
                        crate::model::AP_DOMAIN,
                        state.https_listen.port()
                    )
        })
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
mod tests;

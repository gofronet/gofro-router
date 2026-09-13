use super::*;
use crate::{
    fake_dns::FakeDns,
    geodata::GeoData,
    model::{ControllerConfig, LanContext, RoutingConfig},
    routing::RoutingPolicy,
    stats::StatsTracker,
};
use tower::ServiceExt;

fn test_state(dir: &std::path::Path, password: PathBuf, setup_code: PathBuf) -> AppState {
    let geodata = Arc::new(GeoData::default());
    AppState {
        interface: "eth0".into(),
        lan: LanContext {
            device: "eth0".into(),
            address: "192.168.0.1".parse().unwrap(),
            subnet: "192.168.0.0/24".parse().unwrap(),
        },
        https_listen: "192.168.0.1:8443".parse().unwrap(),
        http_listen: "192.168.0.1:8081".parse().unwrap(),
        dns_listen: "192.168.0.1:5353".parse().unwrap(),
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
    headers.insert(
        header::HOST,
        HeaderValue::from_static("wifi.gofro.net:8443"),
    );
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://wifi.gofro.net:8443"),
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
fn accepts_only_current_lan_and_domain_authorities() {
    let dir = std::env::temp_dir().join(format!("gofro-auth-authority-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
    assert!(allowed_host(
        &state,
        &"https://wifi.gofro.net:8443/api/auth/status"
            .parse()
            .unwrap(),
        &HeaderMap::new()
    ));
    assert!(allowed_host(
        &state,
        &"https://192.168.0.1:8443/".parse().unwrap(),
        &HeaderMap::new()
    ));
    assert!(!allowed_host(
        &state,
        &"https://192.168.0.2:8443/".parse().unwrap(),
        &HeaderMap::new()
    ));
    assert!(!allowed_host(
        &state,
        &"https://192.168.0.1:9443/".parse().unwrap(),
        &HeaderMap::new()
    ));
    assert!(!allowed_host(
        &state,
        &"https://evil.example:8443/".parse().unwrap(),
        &HeaderMap::new()
    ));
}
#[test]
fn rejects_conflicting_authority_and_host_header() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("192.168.0.1:8443"));
    let dir = std::env::temp_dir().join(format!("gofro-auth-authority-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
    assert!(!allowed_host(
        &state,
        &"https://wifi.gofro.net:8443/api/auth/status"
            .parse()
            .unwrap(),
        &headers
    ));
}
#[test]
fn setup_code_must_match_exactly_one_code() {
    assert!(setup_code_matches("setup-code", "setup-code\n"));
    assert!(!setup_code_matches("setup-code", "other-code\n"));
}

#[tokio::test]
async fn canonical_and_recovery_authorities_reach_auth_routes() {
    for authority in [
        "wifi.gofro.net",
        "wifi.gofro.net:443",
        "wifi.gofro.net:8443",
        "192.168.0.1:8443",
    ] {
        let dir = std::env::temp_dir().join(format!("gofro-canonical-{}", token().unwrap()));
        fs::create_dir(&dir).unwrap();
        let password = dir.join("admin-password");
        let state = test_state(&dir, password.clone(), dir.join("setup-code"));
        let app = crate::api::secure_router(state.clone());
        for configured in [false, true] {
            if configured {
                write_record(&password, "test-password").unwrap();
            }
            let response = app
                .clone()
                .oneshot(
                    Request::get(format!("https://{authority}/api/auth/status"))
                        .version(axum::http::Version::HTTP_2)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{authority}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
            let cookie = response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body["state"], if configured { "login" } else { "setup" });
            let csrf = body["csrf_token"].as_str().unwrap();
            assert_eq!(cookie, format!("{CSRF}={csrf}"));
            let path = if configured {
                "/api/auth/login"
            } else {
                "/api/auth/setup"
            };
            let response = app
                .clone()
                .oneshot(
                    Request::post(path)
                        .header(header::HOST, authority)
                        .header(header::ORIGIN, format!("https://{authority}"))
                        .header(header::COOKIE, &cookie)
                        .header("x-csrf-token", csrf)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(axum::body::Body::from(if configured {
                            r#"{"password":"fake-wrong-password"}"#
                        } else {
                            r#"{"password":"short"}"#
                        }))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if configured {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::BAD_REQUEST
                },
                "{authority}"
            );
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                body["error"],
                if configured {
                    "invalid_password"
                } else {
                    "password_too_short"
                }
            );
        }
        let (session, csrf) = state.auth.issue().unwrap();
        for supplied in ["wrong", csrf.as_str()] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/auth/logout")
                        .header(header::HOST, authority)
                        .header(header::ORIGIN, format!("https://{authority}"))
                        .header(header::COOKIE, format!("{SESSION}={session}"))
                        .header("x-csrf-token", supplied)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if supplied == csrf {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "{authority}"
            );
        }
        assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn auth_rejects_untrusted_authorities_origins_and_http2_conflicts() {
    let dir = std::env::temp_dir().join(format!("gofro-origin-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
    let app = crate::api::secure_router(state.clone());
    for authority in [
        "198.18.0.0:8443",
        "198.18.0.0",
        "192.168.0.2:8443",
        "192.168.0.1",
        "192.168.0.1:443",
        "wifi.gofro.net:80",
        "wifi.gofro.net:8081",
        "wifi.gofro.net:9443",
        "wifi.gofro.net.evil.example",
        "evilwifi.gofro.net",
        "wifi.gofro.net.",
        "evil.example@wifi.gofro.net",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/auth/status")
                    .header(header::HOST, authority)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{authority}");
        let mut headers = preauth_headers();
        headers.insert(
            header::ORIGIN,
            format!("https://{authority}").parse().unwrap(),
        );
        assert!(!allowed_origin(&state, &headers), "{authority}");
    }
    for origin in [
        "http://wifi.gofro.net",
        "http://wifi.gofro.net:8443",
        "null",
        "https://evil.example",
        "https://wifi.gofro.net/",
        "https://wifi.gofro.net?x=1",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/auth/setup")
                    .header(header::HOST, "wifi.gofro.net")
                    .header(header::ORIGIN, origin)
                    .header(header::COOKIE, "__Host-gofro-csrf=token")
                    .header("x-csrf-token", "token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(r#"{"password":"short"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
    }
    for host in ["wifi.gofro.net:443", "wifi.gofro.net:8443", "evil.example"] {
        let response = app
            .clone()
            .oneshot(
                Request::get("https://wifi.gofro.net/api/auth/status")
                    .version(axum::http::Version::HTTP_2)
                    .header(header::HOST, host)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host}");
    }
    let uri = "https://wifi.gofro.net/".parse().unwrap();
    let mut headers = HeaderMap::new();
    headers.append(header::HOST, HeaderValue::from_static("wifi.gofro.net"));
    headers.append(header::HOST, HeaderValue::from_static("wifi.gofro.net"));
    assert!(request_authority(&uri, &headers).is_none());
    headers.insert(header::HOST, HeaderValue::from_bytes(b"\xff").unwrap());
    assert!(request_authority(&uri, &headers).is_none());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn redirects_canonicalize_domain_and_preserve_lan_recovery_and_query() {
    let dir = std::env::temp_dir().join(format!("gofro-redirect-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let mut state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
    for (http_port, https_port) in [(8081, 8443), (9081, 9443)] {
        state.http_listen.set_port(http_port);
        state.https_listen.set_port(https_port);
        let app = crate::api::redirect_router(state.clone());
        for (authority, destination) in [
            (
                "wifi.gofro.net".to_owned(),
                "https://wifi.gofro.net".to_owned(),
            ),
            (
                "wifi.gofro.net:80".to_owned(),
                "https://wifi.gofro.net".to_owned(),
            ),
            (
                "wifi.gofro.net:8081".to_owned(),
                "https://wifi.gofro.net".to_owned(),
            ),
            (
                format!("wifi.gofro.net:{http_port}"),
                "https://wifi.gofro.net".to_owned(),
            ),
            (
                format!("192.168.0.1:{http_port}"),
                format!("https://192.168.0.1:{https_port}"),
            ),
        ] {
            for absolute in [false, true] {
                let path = "/api/auth/status?next=%2Fhome&x=1";
                let uri = if absolute {
                    format!("http://{authority}{path}")
                } else {
                    path.to_owned()
                };
                let mut request = Request::get(uri);
                if !absolute {
                    request = request.header(header::HOST, &authority);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(axum::body::Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::TEMPORARY_REDIRECT,
                    "{authority}"
                );
                assert_eq!(
                    response.headers()[header::LOCATION],
                    format!("{destination}{path}")
                );
            }
        }
        for host in [
            "evil.example",
            "wifi.gofro.net.evil.example",
            "wifi.gofro.net:443",
            "wifi.gofro.net:8443",
            "198.18.0.0:80",
            "192.168.0.2:8081",
            "192.168.0.1:80",
            "evil.example@wifi.gofro.net",
        ] {
            for uri in ["/path?q=1", "http://wifi.gofro.net/path?q=1"] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::get(uri)
                            .header(header::HOST, host)
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host} {uri}");
                assert!(!response.headers().contains_key(header::LOCATION));
            }
        }
        if http_port != 8081 {
            let response = app
                .clone()
                .oneshot(
                    Request::get("http://192.168.0.1:8081/?q=1")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(!response.headers().contains_key(header::LOCATION));
        }
        for (uri, host) in [
            ("http://wifi.gofro.net/", Some("wifi.gofro.net:80")),
            ("http://evil.example/", None),
            ("/", None),
        ] {
            let mut request = Request::get(uri).version(axum::http::Version::HTTP_2);
            if let Some(host) = host {
                request = request.header(header::HOST, host);
            }
            let response = app
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(!response.headers().contains_key(header::LOCATION));
        }
    }
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn validates_setup_password_character_and_byte_limits() {
    for character in ["a", "é", "😀"] {
        assert_eq!(
            setup_password_error(&character.repeat(7)),
            Some("password_too_short")
        );
        assert_eq!(setup_password_error(&character.repeat(8)), None);
    }
    assert_eq!(setup_password_error(&"e\u{301}".repeat(4)), None);
    assert_eq!(setup_password_error(&"a".repeat(128)), None);
    assert_eq!(
        setup_password_error(&"a".repeat(129)),
        Some("password_too_long")
    );
    assert_eq!(setup_password_error(&"é".repeat(64)), None);
    assert_eq!(setup_password_error(&"😀".repeat(32)), None);
    assert_eq!(
        setup_password_error(&format!("{}a", "😀".repeat(32))),
        Some("password_too_long")
    );
    assert_eq!(
        setup_password_error(&"😀".repeat(33)),
        Some("password_too_long")
    );
}
#[test]
fn preauth_requires_matching_csrf_token() {
    let dir = std::env::temp_dir().join(format!("gofro-auth-authority-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let state = test_state(&dir, dir.join("admin-password"), dir.join("setup-code"));
    let uri: Uri = "https://wifi.gofro.net:8443/api/auth/setup"
        .parse()
        .unwrap();
    let mut headers = preauth_headers();
    assert!(preauth(&state, &uri, &headers));
    headers.insert("x-csrf-token", HeaderValue::from_static("wrong"));
    assert!(!preauth(&state, &uri, &headers));
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
    let uri: Uri = "https://wifi.gofro.net:8443/api/auth/setup"
        .parse()
        .unwrap();
    let headers = preauth_headers();
    let short = setup_response(&state, &uri, &headers, Some("setup-code"), "1234567").await;
    assert_eq!(short.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        axum::body::to_bytes(short.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        br#"{"error":"password_too_short"}"#
    );
    assert!(!password_path.exists());
    assert_eq!(
        setup_response(
            &state,
            &uri,
            &headers,
            Some("setup-code"),
            "a valid password"
        )
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
        .header(header::HOST, "wifi.gofro.net:8443")
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
        .header(header::HOST, "wifi.gofro.net:8443")
        .header(header::ORIGIN, "https://wifi.gofro.net:8443")
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
    // Missing JSON keeps a guard regression from reaching SSH, while the exact
    // auth errors below prove rejection happens before body extraction.
    for (method, path) in [
        (Method::POST, "/api/servers/probe"),
        (Method::POST, "/api/servers/bootstrap"),
        (Method::POST, "/api/servers/check"),
        (Method::POST, "/api/servers/update-managed"),
        (Method::POST, "/api/servers/create-profile"),
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
            .header(header::HOST, "wifi.gofro.net:8443")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = crate::api::secure_router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap()
                .as_ref(),
            br#"{"error":"session_expired"}"#,
            "{method} {path}"
        );
        let (session, csrf) = state.auth.issue().unwrap();
        let request = Request::builder()
            .method(method.clone())
            .uri(path)
            .header(header::HOST, "wifi.gofro.net:8443")
            .header(header::ORIGIN, "https://wifi.gofro.net:8443")
            .header(header::COOKIE, format!("{SESSION}={session}"))
            .header("x-csrf-token", format!("wrong-{csrf}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = crate::api::secure_router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap()
                .as_ref(),
            br#"{"error":"request_rejected"}"#,
            "{method} {path}"
        );
    }
    let (session, csrf) = state.auth.issue().unwrap();
    let request = Request::post("/api/servers/friends")
        .header(header::HOST, "wifi.gofro.net:8443")
        .header(header::ORIGIN, "https://wifi.gofro.net:8443")
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
    let uri = "https://wifi.gofro.net:8443/api/auth/setup"
        .parse()
        .unwrap();
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
async fn setup_code_claims_one_admin_and_advances_to_server() {
    let dir = std::env::temp_dir().join(format!("gofro-fresh-claim-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let password_path = dir.join("admin-password");
    let code = dir.join("setup-code");
    fs::write(&code, "setup-code\n").unwrap();
    let state = test_state(&dir, password_path.clone(), code);
    let marker = dir.join("onboarding-state");
    fs::write(&marker, "admin\n").unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let uptime = fs::read_to_string("/proc/uptime").unwrap();
    let now: u64 = uptime.split('.').next().unwrap().parse().unwrap();
    let window = dir.join("onboarding-window");
    fs::write(&window, format!("{} {}\n", boot.trim(), now + 900)).unwrap();
    fs::set_permissions(&window, fs::Permissions::from_mode(0o600)).unwrap();
    let uri = "https://wifi.gofro.net:8443/api/auth/setup"
        .parse()
        .unwrap();
    let headers = preauth_headers();
    let password = "12345678";
    let (first, second) = tokio::join!(
        setup_response(&state, &uri, &headers, Some("setup-code"), password),
        setup_response(&state, &uri, &headers, Some("setup-code"), password),
    );
    assert_eq!(
        [first.status(), second.status()]
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert!(verify(&read_record(&password_path).unwrap(), password).unwrap());
    assert_eq!(onboarding::step(&state).unwrap(), onboarding::Step::Server);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "server\n");
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn verifies_record() {
    let path = std::env::temp_dir().join("gofro-auth-test");
    write_record(&path, "12345678").unwrap();
    assert!(verify(&read_record(&path).unwrap(), "12345678").unwrap());
    assert!(!verify(&read_record(&path).unwrap(), "wrong").unwrap());
    fs::remove_file(path).unwrap();
}

async fn login_response(state: &AppState, password: &str) -> Response {
    login(
        State(state.clone()),
        "https://wifi.gofro.net:8443/api/auth/login"
            .parse()
            .unwrap(),
        preauth_headers(),
        Json(PasswordInput {
            setup_code: None,
            password: password.to_owned(),
        }),
    )
    .await
}

#[tokio::test]
async fn verified_login_repairs_interrupted_claim_after_restart() {
    let dir = std::env::temp_dir().join(format!("gofro-claim-repair-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let password = dir.join("admin-password");
    let code = dir.join("setup-code");
    let marker = dir.join("onboarding-state");
    let mut state = test_state(&dir, password.clone(), code.clone());
    write_record(&password, "12345678").unwrap();
    fs::write(&code, "setup-code\n").unwrap();
    fs::write(&marker, "admin\n").unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    let credential = fs::read(&password).unwrap();
    state.auth = Arc::new(Auth::open(password.clone(), code.clone()).unwrap());
    assert!(onboarding::step(&state).is_err());

    let response = login_response(&state, "wrong-password").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(fs::read(&password).unwrap(), credential);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "admin\n");
    assert_eq!(fs::read_to_string(&code).unwrap(), "setup-code\n");
    assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
    state.auth = Arc::new(Auth::open(password.clone(), code.clone()).unwrap());

    // A directory at the temporary file path deterministically fails the marker write.
    fs::create_dir(marker.with_extension("new")).unwrap();
    assert_eq!(
        login_response(&state, "12345678").await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "admin\n");
    assert!(code.exists());
    assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
    fs::remove_dir(marker.with_extension("new")).unwrap();

    let response = login_response(&state, "12345678").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(fs::read(&password).unwrap(), credential);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "server\n");
    assert!(!code.exists());
    assert!(!dir.join("onboarding-window").exists());
    let session_cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(SESSION))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let request = Request::get("/api/onboarding")
        .header(header::HOST, "wifi.gofro.net:8443")
        .header(header::COOKIE, session_cookie)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = crate::api::secure_router(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["step"], "server");
    assert_eq!(
        login_response(&state, "12345678").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        setup_response(
            &state,
            &"/api/auth/setup".parse().unwrap(),
            &preauth_headers(),
            Some("setup-code"),
            "replacement-password"
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(fs::read(&password).unwrap(), credential);
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn login_completion_retries_code_retirement_without_resetting_server_state() {
    let dir = std::env::temp_dir().join(format!("gofro-code-repair-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let password = dir.join("admin-password");
    let code = dir.join("setup-code");
    let marker = dir.join("onboarding-state");
    write_record(&password, "12345678").unwrap();
    let state = test_state(&dir, password, code.clone());
    fs::write(&marker, "admin\n").unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&state.config_path, "existing server configuration").unwrap();
    fs::create_dir(&code).unwrap();
    assert_eq!(
        login_response(&state, "12345678").await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "server\n");
    assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
    fs::remove_dir(&code).unwrap();
    fs::write(&code, "setup-code\n").unwrap();
    // Server retries must not rewrite the marker.
    fs::create_dir(marker.with_extension("new")).unwrap();
    assert_eq!(
        login_response(&state, "12345678").await.status(),
        StatusCode::OK
    );
    assert!(!code.exists());
    assert_eq!(
        fs::read_to_string(&state.config_path).unwrap(),
        "existing server configuration"
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "server\n");
    fs::remove_file(&marker).unwrap();
    assert_eq!(
        login_response(&state, "12345678").await.status(),
        StatusCode::OK
    );
    assert!(!marker.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_login_does_not_repair_malformed_or_legacy_markers() {
    let dir = std::env::temp_dir().join(format!("gofro-invalid-repair-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let password = dir.join("admin-password");
    let code = dir.join("setup-code");
    let marker = dir.join("onboarding-state");
    write_record(&password, "12345678").unwrap();
    let credential = fs::read(&password).unwrap();
    let state = test_state(&dir, password.clone(), code.clone());
    fs::write(&code, "setup-code\n").unwrap();
    for phase in [
        "wifi\n",
        "wifi_applying\n",
        "invalid\n",
        "admin",
        "admin\n\n",
    ] {
        fs::write(&marker, phase).unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            login_response(&state, "12345678").await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(onboarding::step(&state).is_err());
        assert_eq!(fs::read_to_string(&marker).unwrap(), phase);
        assert_eq!(fs::read(&password).unwrap(), credential);
        assert_eq!(fs::read_to_string(&code).unwrap(), "setup-code\n");
        assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
    }
    fs::remove_dir_all(dir).unwrap();
}

fn password_change_state() -> (PathBuf, AppState, HeaderMap) {
    let dir = std::env::temp_dir().join(format!("gofro-password-{}", token().unwrap()));
    fs::create_dir(&dir).unwrap();
    let password = dir.join("admin-password");
    write_record(&password, "old-password").unwrap();
    let state = test_state(&dir, password, dir.join("setup-code"));
    let (session, csrf) = state.auth.issue().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("wifi.gofro.net"));
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://wifi.gofro.net"),
    );
    headers.insert(
        header::COOKIE,
        format!("{SESSION}={session}").parse().unwrap(),
    );
    headers.insert("x-csrf-token", csrf.parse().unwrap());
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (dir, state, headers)
}

async fn password_change_response(
    state: &AppState,
    headers: &HeaderMap,
    current: &str,
    password: &str,
) -> Response {
    let mut request = Request::post("/api/auth/password")
        .body(axum::body::Body::from(
            serde_json::json!({
                "current_password": current,
                "password": password,
            })
            .to_string(),
        ))
        .unwrap();
    *request.headers_mut() = headers.clone();
    crate::api::secure_router(state.clone())
        .oneshot(request)
        .await
        .unwrap()
}

async fn response_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn password_change_rejections_preserve_credentials_and_sessions() {
    let (dir, state, headers) = password_change_state();
    let original = fs::read(&state.auth.password).unwrap();
    for (remove, status, code) in [
        (header::COOKIE, StatusCode::UNAUTHORIZED, "session_expired"),
        (
            header::HeaderName::from_static("x-csrf-token"),
            StatusCode::FORBIDDEN,
            "request_rejected",
        ),
        (header::ORIGIN, StatusCode::FORBIDDEN, "request_rejected"),
    ] {
        let mut invalid = headers.clone();
        invalid.remove(remove);
        let response = password_change_response(&state, &invalid, "old-password", "12345678").await;
        assert_eq!(response.status(), status);
        assert_eq!(response_json(response).await["error"], code);
    }
    for (name, value) in [
        (header::ORIGIN, "http://wifi.gofro.net"),
        (header::ORIGIN, "https://evil.example"),
        (header::HOST, "198.18.0.0"),
        (header::HeaderName::from_static("x-csrf-token"), "wrong"),
    ] {
        let mut invalid = headers.clone();
        invalid.insert(name, value.parse().unwrap());
        let response = password_change_response(&state, &invalid, "old-password", "12345678").await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response_json(response).await["error"], "request_rejected");
    }
    let mut request = Request::post("https://wifi.gofro.net:443/api/auth/password")
        .version(axum::http::Version::HTTP_2)
        .body(axum::body::Body::from(
            r#"{"current_password":"old-password","password":"12345678"}"#,
        ))
        .unwrap();
    *request.headers_mut() = headers.clone();
    let response = crate::api::secure_router(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for (password, code) in [
        ("é".repeat(7), "password_too_short"),
        ("a".repeat(129), "password_too_long"),
        ("😀".repeat(33), "password_too_long"),
    ] {
        let response = password_change_response(&state, &headers, "old-password", &password).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response_json(response).await["error"], code);
    }
    let response = password_change_response(&state, &headers, "wrong", "12345678").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(
        response_json(response).await["error"],
        "invalid_current_password"
    );
    let response = password_change_response(&state, &headers, "old-password", "12345678").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response_json(response).await["error"], "login_throttled");
    assert_eq!(fs::read(&state.auth.password).unwrap(), original);
    assert!(session(&state.auth, &headers).is_ok());
    assert!(!state.auth.password.with_extension("new").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn password_change_rotates_all_sessions_and_survives_auth_reopen() {
    for password in ["12345678".to_owned(), "é".repeat(8), "😀".repeat(32)] {
        let (dir, mut state, headers) = password_change_state();
        let (other_session, _) = state.auth.issue().unwrap();
        let mut other = headers.clone();
        other.insert(
            header::COOKIE,
            format!("{SESSION}={other_session}").parse().unwrap(),
        );
        for (name, contents) in [
            ("config.json", "configuration sentinel"),
            ("onboarding-state", "server\n"),
            ("setup-code", "code sentinel"),
            ("onboarding-window", "window sentinel"),
        ] {
            fs::write(dir.join(name), contents).unwrap();
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let response = password_change_response(&state, &headers, "old-password", &password).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(cookies.len(), 2);
        for cookie in &cookies {
            assert!(cookie.contains("; Path=/; Secure; HttpOnly; SameSite=Strict"));
            assert!(!cookie.contains("Domain="));
        }
        let body = response_json(response).await;
        assert_eq!(body["state"], "authenticated");
        let mut rotated = headers.clone();
        rotated.insert(
            header::COOKIE,
            cookies
                .iter()
                .map(|cookie| cookie.split(';').next().unwrap())
                .collect::<Vec<_>>()
                .join("; ")
                .parse()
                .unwrap(),
        );
        rotated.insert(
            "x-csrf-token",
            body["csrf_token"].as_str().unwrap().parse().unwrap(),
        );
        assert_eq!(cookie(&rotated, CSRF).unwrap(), body["csrf_token"]);
        assert!(session(&state.auth, &rotated).is_ok());
        assert!(session(&state.auth, &headers).is_err());
        assert!(session(&state.auth, &other).is_err());
        assert_eq!(state.auth.inner.lock().unwrap().sessions.len(), 1);
        let mut request = Request::get("/api/auth/status")
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = rotated.clone();
        let response = crate::api::secure_router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, body);
        let record = read_record(&state.auth.password).unwrap();
        assert!(verify(&record, &password).unwrap());
        assert!(!verify(&record, "old-password").unwrap());
        assert_eq!(
            fs::metadata(&state.auth.password)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        for (name, contents) in [
            ("config.json", "configuration sentinel"),
            ("onboarding-state", "server\n"),
            ("setup-code", "code sentinel"),
            ("onboarding-window", "window sentinel"),
        ] {
            assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), contents);
        }
        // Revalidate after middleware: a request admitted before rotation is now stale.
        let response = change_password_record(
            &state.auth,
            &headers,
            PasswordChangeInput {
                current_password: password.clone(),
                password: "replacement".into(),
            },
            |_, _| panic!("stale session must not publish"),
        );
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        state.auth = Arc::new(
            Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap(),
        );
        assert_eq!(
            login_response(&state, "old-password").await.status(),
            StatusCode::UNAUTHORIZED
        );
        state.auth = Arc::new(
            Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap(),
        );
        assert_eq!(
            login_response(&state, &password).await.status(),
            StatusCode::OK
        );
        fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn password_change_write_failure_preserves_old_account() {
    let (dir, state, headers) = password_change_state();
    let original = fs::read(&state.auth.password).unwrap();
    let temporary = state.auth.password.with_extension("new");
    fs::create_dir(&temporary).unwrap();
    let response = password_change_response(&state, &headers, "old-password", "12345678").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response_json(response).await["error"], "internal_error");
    assert_eq!(fs::read(&state.auth.password).unwrap(), original);
    assert!(session(&state.auth, &headers).is_ok());
    let reopened = Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap();
    assert!(verify(&read_record(&reopened.password).unwrap(), "old-password").unwrap());
    assert!(temporary.is_dir());
    fs::remove_dir(&temporary).unwrap();
    let response = password_change_response(&state, &headers, "old-password", "12345678").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!temporary.exists());
    assert!(verify(&read_record(&state.auth.password).unwrap(), "12345678").unwrap());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn password_change_recovers_stale_files_after_reopen_without_following_symlinks() {
    for leftover in ["regular", "symlink", "dangling", "directory-link"] {
        let (dir, mut state, mut headers) = password_change_state();
        let temporary = state.auth.password.with_extension("new");
        let target = dir.join("unrelated-target");
        match leftover {
            "regular" => fs::write(&temporary, "interrupted credential write").unwrap(),
            "symlink" => {
                fs::write(&target, "untouched").unwrap();
                std::os::unix::fs::symlink(&target, &temporary).unwrap();
            }
            "directory-link" => {
                fs::create_dir(&target).unwrap();
                fs::write(target.join("sentinel"), "untouched").unwrap();
                std::os::unix::fs::symlink(&target, &temporary).unwrap();
            }
            "dangling" => std::os::unix::fs::symlink(&target, &temporary).unwrap(),
            _ => unreachable!(),
        }
        state.auth = Arc::new(
            Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap(),
        );
        let response = login_response(&state, "old-password").await;
        assert_eq!(response.status(), StatusCode::OK, "{leftover}");
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().split(';').next().unwrap())
            .collect::<Vec<_>>()
            .join("; ");
        headers.insert(header::COOKIE, cookies.parse().unwrap());
        let body = response_json(response).await;
        headers.insert(
            "x-csrf-token",
            body["csrf_token"].as_str().unwrap().parse().unwrap(),
        );
        let response = password_change_response(&state, &headers, "old-password", "12345678").await;
        assert_eq!(response.status(), StatusCode::OK, "{leftover}");
        assert!(fs::symlink_metadata(&temporary).is_err());
        match leftover {
            "symlink" => assert_eq!(fs::read_to_string(&target).unwrap(), "untouched"),
            "directory-link" => {
                assert_eq!(
                    fs::read_to_string(target.join("sentinel")).unwrap(),
                    "untouched"
                );
            }
            "dangling" => assert!(!target.exists()),
            "regular" => {}
            _ => unreachable!(),
        }
        let reopened =
            Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap();
        let record = read_record(&reopened.password).unwrap();
        assert!(verify(&record, "12345678").unwrap());
        assert!(!verify(&record, "old-password").unwrap());
        assert_eq!(
            fs::metadata(&reopened.password)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn failed_record_publication_cleans_temporary_and_can_retry() {
    let (dir, state, headers) = password_change_state();
    let _permit = state.auth.hashing.clone().try_acquire_owned().unwrap();
    let original = fs::read(&state.auth.password).unwrap();
    let blocked = dir.join("blocked-password");
    fs::create_dir(&blocked).unwrap();
    let failure = write_record(&blocked, "12345678").unwrap_err();
    assert!(!failure.is::<RecordPublished>());
    assert!(blocked.is_dir());
    assert!(!blocked.with_extension("new").exists());
    assert_eq!(fs::read(&state.auth.password).unwrap(), original);
    assert!(session(&state.auth, &headers).is_ok());
    fs::remove_dir(&blocked).unwrap();
    write_record(&blocked, "12345678").unwrap();
    assert!(verify(&read_record(&blocked).unwrap(), "12345678").unwrap());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn custom_ports_keep_domain_aliases_but_lan_requires_configured_ports() {
    let (dir, mut state, mut headers) = password_change_state();
    state.https_listen.set_port(9443);
    state.http_listen.set_port(9081);
    for authority in [
        "wifi.gofro.net",
        "wifi.gofro.net:443",
        "wifi.gofro.net:8443",
        "wifi.gofro.net:9443",
        "192.168.0.1:9443",
    ] {
        headers.insert(header::HOST, authority.parse().unwrap());
        headers.insert(
            header::ORIGIN,
            format!("https://{authority}").parse().unwrap(),
        );
        let uri = format!("https://{authority}/api/auth/password")
            .parse()
            .unwrap();
        assert!(allowed_host(&state, &uri, &headers), "{authority}");
        assert!(allowed_origin(&state, &headers), "{authority}");
    }
    for authority in [
        "192.168.0.1:8443",
        "192.168.0.1:443",
        "198.18.0.0:9443",
        "wifi.gofro.net:8081",
        "wifi.gofro.net:9081",
        "wifi.gofro.net:10443",
        "wifi.gofro.net.evil:8443",
    ] {
        headers.insert(header::HOST, authority.parse().unwrap());
        headers.insert(
            header::ORIGIN,
            format!("https://{authority}").parse().unwrap(),
        );
        assert!(
            !allowed_host(&state, &"/".parse().unwrap(), &headers),
            "{authority}"
        );
        assert!(!allowed_origin(&state, &headers), "{authority}");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn password_change_published_sync_failure_revokes_sessions_and_reports_uncertainty() {
    let (dir, state, headers) = password_change_state();
    state.auth.issue().unwrap();
    let response = change_password_record(
        &state.auth,
        &headers,
        PasswordChangeInput {
            current_password: "old-password".into(),
            password: "12345678".into(),
        },
        |path, password| {
            write_record(path, password)?;
            // Model the post-rename directory-fsync error, after actual publication.
            Err(anyhow::anyhow!("injected directory sync failure").context(RecordPublished))
        },
    );
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(
        response_json(response).await["error"],
        "password_change_uncertain"
    );
    assert!(state.auth.inner.lock().unwrap().sessions.is_empty());
    let reopened = Auth::open(state.auth.password.clone(), state.auth.setup_code.clone()).unwrap();
    let record = read_record(&reopened.password).unwrap();
    assert!(verify(&record, "12345678").unwrap());
    assert!(!verify(&record, "old-password").unwrap());
    fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn concurrent_password_changes_publish_only_once() {
    let (dir, state, headers) = password_change_state();
    let permit = state.auth.hashing.clone().try_acquire_owned().unwrap();
    let response = password_change_response(&state, &headers, "old-password", "12345678").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(response).await["error"], "auth_busy");
    drop(permit);
    let (first, second) = tokio::join!(
        password_change_response(&state, &headers, "old-password", "first-password"),
        password_change_response(&state, &headers, "old-password", "second-password"),
    );
    let statuses = [first.status(), second.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert!(statuses.iter().all(|status| matches!(
        *status,
        StatusCode::OK | StatusCode::CONFLICT | StatusCode::UNAUTHORIZED
    )));
    let record = read_record(&state.auth.password).unwrap();
    assert_ne!(
        verify(&record, "first-password").unwrap(),
        verify(&record, "second-password").unwrap()
    );
    assert!(!verify(&record, "old-password").unwrap());
    assert_eq!(state.auth.inner.lock().unwrap().sessions.len(), 1);
    assert!(session(&state.auth, &headers).is_err());
    fs::remove_dir_all(dir).unwrap();
}

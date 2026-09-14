use super::*;
use crate::{
    auth::Auth,
    fake_dns::FakeDns,
    geodata::GeoData,
    model::{LanContext, RoutingConfig, ServerProfile, ServerUpdate},
    stats::StatsTracker,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize},
    },
};

// Thread-local operation runner: no PATH changes, services, nft or WireGuard.
#[derive(Default)]
struct Runner {
    kernel_guard: Vec<crate::model::MacAddress>,
    kernel_routing: Vec<crate::model::MacAddress>,
    publish_expected: Option<Vec<crate::model::MacAddress>>,
    policy_exclusions: Vec<Vec<crate::model::MacAddress>>,
    calls: Vec<&'static str>,
    failures: VecDeque<&'static str>,
    guarded: bool,
    real_network: bool,
    apply_lock: Option<std::path::PathBuf>,
}
thread_local! { static RUNNER: RefCell<Option<Runner>> = const { RefCell::new(None) }; }

pub(super) fn record_kernel_exclusions(step: &str, exclusions: &[crate::model::MacAddress]) {
    RUNNER.with_borrow_mut(|runner| {
        if let Some(runner) = runner {
            if matches!(step, "guard" | "publish") {
                runner.kernel_guard = exclusions.to_vec();
            }
            if matches!(step, "policy" | "publish") {
                runner.kernel_routing = exclusions.to_vec();
            }
        }
    });
}

#[test]
fn committed_removal_repairs_publication_or_independently_revokes_guard_membership() {
    if run_in_child() {
        return;
    }
    for (postrename, failures) in [
        (false, vec!["publish"]),
        (false, vec!["publish", "publish"]),
        (true, vec![]),
        (true, vec!["publish"]),
        (true, vec!["sync"]),
    ] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let a = "02:aa:bb:cc:dd:01".parse().unwrap();
        let b = "02:aa:bb:cc:dd:02".parse().unwrap();
        update_device_exclusion(state, a, true).unwrap();
        update_device_exclusion(state, b, true).unwrap();
        RUNNER.with_borrow_mut(|runner| {
            let r = runner.as_mut().unwrap();
            assert_eq!(r.kernel_routing, [a, b]);
            r.calls.clear();
            r.failures.extend(failures.clone());
            r.publish_expected = Some(vec![b]);
        });
        if postrename {
            crate::config::FAIL_SYNC.with(|failure| failure.set(Some("parent")));
        }
        let error = update_device_exclusion(state, a, false).unwrap_err();
        assert!(error.is::<crate::managed::CommittedRefreshFailed>());
        assert_eq!(error.is::<crate::config::PublishedSaveError>(), postrename);
        assert_eq!(
            crate::config::load(&state.config_path)
                .unwrap()
                .device_exclusions,
            [b]
        );
        assert_eq!(state.config.lock().unwrap().device_exclusions, [b]);
        RUNNER.with_borrow(|runner| {
            let r = runner.as_ref().unwrap();
            assert!(r.guarded);
            assert_eq!(
                r.kernel_guard,
                [b],
                "removed MAC retains forwarding exemption"
            );
            let repaired = failures.len() < if postrename { 1 } else { 2 };
            assert_eq!(
                r.kernel_routing,
                if repaired { vec![b] } else { vec![a, b] }
            );
            if !repaired {
                assert_eq!(r.calls.last(), Some(&"guard"));
            }
        });
        assert!(state.routing_degraded.load(Ordering::Relaxed));
    }
}

pub(super) fn record_policy_exclusions(exclusions: &[crate::model::MacAddress]) {
    RUNNER.with_borrow_mut(|runner| {
        if let Some(runner) = runner {
            runner.policy_exclusions.push(exclusions.to_vec());
        }
    });
}

#[test]
fn exclusions_commit_before_publication_and_keep_latest_list() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    let a = "02:aa:bb:cc:dd:01".parse().unwrap();
    let b = "02:aa:bb:cc:dd:02".parse().unwrap();
    for (mac, excluded, expected, frozen) in [
        (a, true, vec![a], vec![]),
        (b, true, vec![a, b], vec![a]),
        (a, false, vec![b], vec![a, b]),
        (b, false, vec![], vec![b]),
    ] {
        RUNNER.with_borrow_mut(|runner| {
            let r = runner.as_mut().unwrap();
            r.calls.clear();
            r.policy_exclusions.clear();
            r.publish_expected = Some(expected.clone());
        });
        update_device_exclusion(state, mac, excluded).unwrap();
        assert_eq!(
            crate::config::load(&state.config_path)
                .unwrap()
                .device_exclusions,
            expected
        );
        RUNNER.with_borrow(|runner| {
            let r = runner.as_ref().unwrap();
            assert_eq!(
                r.calls,
                ["guard", "policy", "publish", "dns-cleanup", "clear"]
            );
            assert_eq!(r.policy_exclusions, [frozen]);
        });
    }
    RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().publish_expected = Some(vec![a]));
    update_device_exclusion(state, a, true).unwrap();
    RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().calls.clear());
    mutate(state, "routing").unwrap();
    assert_eq!(
        crate::config::load(&state.config_path)
            .unwrap()
            .device_exclusions,
        [a]
    );
    RUNNER.with_borrow(|runner| {
        assert_eq!(runner.as_ref().unwrap().calls, ["guard", "policy", "clear"])
    });
}

#[test]
fn exclusion_publish_and_cleanup_failures_are_committed_and_full_retry_repairs() {
    if run_in_child() {
        return;
    }
    for step in ["publish", "dns-cleanup"] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let mac = "02:aa:bb:cc:dd:01".parse().unwrap();
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.push_back(step));
        let error = update_device_exclusion(state, mac, true).unwrap_err();
        assert!(error.is::<crate::managed::CommittedRefreshFailed>());
        assert_eq!(
            crate::config::load(&state.config_path)
                .unwrap()
                .device_exclusions,
            [mac]
        );
        assert_eq!(state.config.lock().unwrap().device_exclusions, [mac]);
        assert!(state.routing_degraded.load(Ordering::Relaxed));
        RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().guarded));
        reconcile(state).unwrap();
        assert!(!state.routing_degraded.load(Ordering::Relaxed));
    }
}

#[test]
fn sync_failure_distinguishes_unpublished_and_published_configuration() {
    if run_in_child() {
        return;
    }
    for stage in ["file", "parent"] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let mac = "02:aa:bb:cc:dd:01".parse().unwrap();
        crate::config::FAIL_SYNC.with(|failure| failure.set(Some(stage)));
        let error = update_device_exclusion(state, mac, true).unwrap_err();
        let published = stage == "parent";
        assert_eq!(
            error.is::<crate::managed::CommittedRefreshFailed>(),
            published
        );
        let expected = if published { vec![mac] } else { vec![] };
        assert_eq!(
            crate::config::load(&state.config_path)
                .unwrap()
                .device_exclusions,
            expected
        );
        assert_eq!(state.config.lock().unwrap().device_exclusions, expected);
        assert_eq!(state.routing_degraded.load(Ordering::Relaxed), published);
        RUNNER.with_borrow(|runner| {
            let r = runner.as_ref().unwrap();
            assert_eq!(r.guarded, published);
            assert_eq!(r.calls.contains(&"publish"), published);
            assert_eq!(r.kernel_routing.contains(&mac), published);
            assert_eq!(r.kernel_guard.contains(&mac), published);
            assert_eq!(r.policy_exclusions.len(), if published { 1 } else { 2 });
        });
    }
}

#[test]
fn network_rollback_failure_still_restores_old_policy() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    RUNNER.with_borrow_mut(|runner| {
        runner
            .as_mut()
            .unwrap()
            .failures
            .extend(["network", "network"])
    });
    assert!(mutate(&fixture.0, "edit").is_err());
    RUNNER.with_borrow(|runner| {
        assert_eq!(
            runner.as_ref().unwrap().calls,
            ["snapshot", "guard", "network", "network", "policy"]
        )
    });
    assert!(fixture.0.routing_degraded.load(Ordering::Relaxed));
}

#[test]
fn mutation_limit_rejects_before_any_effect_and_duplicate_add_is_idempotent() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    state.config.lock().unwrap().device_exclusions = (0..256)
        .map(|n| format!("02:00:00:00:00:{n:02x}").parse().unwrap())
        .collect();
    save(&state.config_path, &state.config.lock().unwrap()).unwrap();
    let before = fs::read(&state.config_path).unwrap();
    assert!(update_device_exclusion(state, "02:00:00:00:01:00".parse().unwrap(), true).is_err());
    assert_eq!(fs::read(&state.config_path).unwrap(), before);
    RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().calls.is_empty()));
    update_device_exclusion(state, "02:00:00:00:00:01".parse().unwrap(), true).unwrap();
    assert_eq!(state.config.lock().unwrap().device_exclusions.len(), 256);
    RUNNER.with_borrow(|runner| {
        assert_eq!(runner.as_ref().unwrap().calls, ["guard", "policy", "clear"])
    });
}

fn run_in_child() -> bool {
    let thread = std::thread::current();
    let test = thread.name().unwrap();
    if std::env::var("GOFRO_CONTROLLER_TEST_CHILD").as_deref() == Ok(test) {
        return false;
    }
    // Parallel CLI tests can fork with our locked FDs before CLOEXEC closes
    // them. Keep real lock assertions in a separate process, not a retry loop.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("GOFRO_CONTROLLER_TEST_CHILD", test)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{test}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    true
}

pub(super) fn run_external(step: &'static str) -> Option<Result<()>> {
    RUNNER.with_borrow_mut(|runner| {
        let runner = runner.as_mut()?;
        if step == "publish"
            && let Some(expected) = &runner.publish_expected
        {
            let path = runner
                .apply_lock
                .as_ref()
                .unwrap()
                .parent()
                .unwrap()
                .join("config.json");
            assert_eq!(
                &crate::config::load(&path).unwrap().device_exclusions,
                expected,
                "publish before configuration commit"
            );
        }
        if let Some(path) = &runner.apply_lock {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            assert!(
                matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)),
                "apply lock released before {step}"
            );
        }
        let result = (|| {
            runner.calls.push(step);
            if runner.failures.front() == Some(&step) {
                runner.failures.pop_front();
                bail!("injected {step} failure");
            }
            match step {
                "guard" | "publish" => runner.guarded = true,
                "clear" => {
                    assert!(runner.guarded);
                    runner.guarded = false;
                }
                "network" | "select-peer" | "policy" | "retire" | "dns-cleanup" => {
                    assert!(runner.guarded)
                }
                "snapshot" | "sync" => {}
                _ => panic!("unexpected external operation: {step}"),
            }
            Ok(())
        })();
        if result.is_ok() && step == "network" && runner.real_network {
            None
        } else {
            Some(result)
        }
    })
}

struct Fixture(AppState);
impl Fixture {
    fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gofro-controller-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let geodata = Arc::new(GeoData::default());
        let first = server('A');
        let config = ControllerConfig {
            auto_update_enabled: false,
            device_exclusions: vec![],
            vpn_enabled: true,
            active_server_key: Some(first.public_key.clone()),
            servers: vec![first, server('B')],
            routing: fixture_routing(),
        };
        let state = AppState {
            interface: "gt0".into(),
            lan: LanContext {
                device: "br-lan".into(),
                address: "192.168.8.1".parse().unwrap(),
                subnet: "192.168.8.0/24".parse().unwrap(),
            },
            https_listen: "192.168.8.1:443".parse().unwrap(),
            http_listen: "192.168.8.1:8081".parse().unwrap(),
            dns_listen: "192.168.8.1:5353".parse().unwrap(),
            config_path: dir.join("config.json"),
            mode_command: dir.join("never-execute"),
            management_dir: dir.join("management"),
            routing: Arc::new(RwLock::new(
                RoutingPolicy::compile(config.routing.clone(), geodata.clone()).unwrap(),
            )),
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(StatsTracker::default())),
            geodata,
            routing_degraded: Arc::new(AtomicBool::new(false)),
            fake_dns: Arc::new(FakeDns::open(&dir.join("routing.sqlite")).unwrap()),
            auth: Arc::new(Auth::open(dir.join("password"), dir.join("setup-code")).unwrap()),
            managed_operations: Arc::new(Mutex::new(())),
        };
        save(&state.config_path, &state.config.lock().unwrap()).unwrap();
        RUNNER.with_borrow_mut(|runner| {
            *runner = Some(Runner {
                apply_lock: Some(dir.join("apply.lock")),
                ..Runner::default()
            })
        });
        Self(state)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        RUNNER.with_borrow_mut(|runner| *runner = None);
        fs::remove_dir_all(self.0.config_path.parent().unwrap()).unwrap();
    }
}

fn server(key: char) -> ServerProfile {
    ServerProfile {
        name: key.to_string(),
        emoji: String::new(),
        endpoint: "vpn.example:8443".into(),
        public_key: format!("{}=", key.to_string().repeat(43)),
        client_tunnel_address: Some("10.202.0.2/32".into()),
        client_private_key: None,
        management: None,
    }
}

fn fixture_routing() -> RoutingConfig {
    RoutingConfig {
        domain_rules: vec![],
        ip_rules: vec![],
        default_target: crate::model::RouteTarget::Vpn,
        mode: crate::model::RoutingMode::Rules,
        rule_order: None,
    }
}

fn mutate(state: &AppState, operation: &str) -> Result<()> {
    let first = server('A');
    match operation {
        "mode" => set_mode(state, false),
        "same-mode" => set_mode(state, true),
        "routing" => update_routing(
            state,
            RoutingConfig {
                default_target: crate::model::RouteTarget::Direct,
                ..fixture_routing()
            },
        ),
        "select" => select_server(state, &server('B').public_key),
        "same-server" => select_server(state, &first.public_key),
        "delete" => delete_server(state, &first.public_key),
        "add" => add_server(state, server('C')),
        "edit" => update_server(
            state,
            ServerUpdate {
                previous_public_key: first.public_key.clone(),
                public_key: first.public_key,
                name: first.name,
                endpoint: "changed.example:8443".into(),
                emoji: None,
            },
        ),
        "import" => import_server(
            state,
            "Imported".into(),
            format!(
                "[Interface]\nPrivateKey = {}\nAddress = 10.202.0.5/32\nMTU = 1280\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0\nEndpoint = changed.example:8443\nPersistentKeepalive = 10\n",
                server('C').public_key,
                first.public_key
            ),
        ),
        _ => panic!("unknown operation"),
    }
}

#[test]
fn guard_failure_never_mutates_or_rolls_back_sibling_paths() {
    if run_in_child() {
        return;
    }
    for operation in [
        "mode",
        "same-mode",
        "routing",
        "select",
        "edit",
        "import",
        "delete",
    ] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let before = fs::read(&state.config_path).unwrap();
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.push_back("guard"));
        assert!(mutate(state, operation).is_err(), "{operation}");
        RUNNER.with_borrow(|runner| {
            assert!(
                runner
                    .as_ref()
                    .unwrap()
                    .calls
                    .iter()
                    .all(|step| matches!(*step, "snapshot" | "guard")),
                "{operation}"
            )
        });
        assert_eq!(before, fs::read(&state.config_path).unwrap());
        assert_eq!(
            before,
            serde_json::to_vec_pretty(&*state.config.lock().unwrap()).unwrap()
        );
    }
}

#[test]
fn failed_rollback_blocks_all_partial_requests_until_full_reconcile() {
    if run_in_child() {
        return;
    }
    for (operation, step) in [("edit", "network"), ("routing", "policy")] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.extend([step, step]));
        assert!(
            mutate(state, operation)
                .unwrap_err()
                .to_string()
                .contains("rollback failed")
        );
        assert!(state.routing_degraded.load(Ordering::Relaxed));
        RUNNER.with_borrow_mut(|runner| {
            let runner = runner.as_mut().unwrap();
            assert!(runner.guarded);
            runner.calls.clear();
        });
        for operation in [
            "mode",
            "same-mode",
            "routing",
            "select",
            "same-server",
            "edit",
            "import",
            "delete",
            "add",
        ] {
            assert!(
                mutate(state, operation)
                    .unwrap_err()
                    .to_string()
                    .contains("full reconcile"),
                "{operation}"
            );
        }
        RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().calls.is_empty()));
        let mut stale = state.config.lock().unwrap().routing.clone();
        stale.default_target = crate::model::RouteTarget::Block;
        *state.routing.write().unwrap() =
            RoutingPolicy::compile(stale, state.geodata.clone()).unwrap();
        reconcile(state).unwrap();
        assert_eq!(
            serde_json::to_value(state.routing.read().unwrap().config()).unwrap(),
            serde_json::to_value(&state.config.lock().unwrap().routing).unwrap(),
        );
        RUNNER.with_borrow(|runner| {
            let runner = runner.as_ref().unwrap();
            assert_eq!(
                runner.calls,
                [
                    "guard",
                    "policy",
                    "dns-cleanup",
                    "network",
                    "retire",
                    "clear"
                ]
            );
            assert!(!runner.guarded);
        });
        assert!(!state.routing_degraded.load(Ordering::Relaxed));
        mutate(state, "same-mode").unwrap();
    }
}

#[test]
fn full_reconcile_publishes_saved_policy_before_network_and_stops_on_policy_failure() {
    if run_in_child() {
        return;
    }
    for failure in ["network", "policy", "sqlite"] {
        for populated in [false, true] {
            // An UPDATE trigger needs a lease to exercise a real commit failure.
            if failure == "sqlite" && !populated {
                continue;
            }
            for enabled in [false, true] {
                let mut fixture = Fixture::new();
                let state = &mut fixture.0;
                let directory = state.config_path.parent().unwrap();
                let history = directory.join("routing-legacy.json");
                fs::write(&history, b"legacy history").unwrap();
                let database = directory.join("routing.sqlite");
                let connection = rusqlite::Connection::open(&database).unwrap();
                if populated {
                    connection.execute(
                        "INSERT INTO fake_dns (fake, domain, real, target, expires) VALUES (?1, 'example.com', ?2, 3, ?3)",
                        rusqlite::params![u32::from(std::net::Ipv4Addr::new(198, 18, 0, 1)), u32::from(std::net::Ipv4Addr::new(8, 8, 8, 8)), i64::MAX],
                    ).unwrap();
                    state.fake_dns = Arc::new(FakeDns::open(&database).unwrap());
                }
                if failure == "sqlite" {
                    connection.execute_batch("CREATE TRIGGER refuse_update BEFORE UPDATE ON fake_dns BEGIN SELECT RAISE(FAIL, 'injected SQLite failure'); END;").unwrap();
                } else {
                    RUNNER.with_borrow_mut(|runner| {
                        runner.as_mut().unwrap().failures.push_back(failure)
                    });
                }
                state.config.lock().unwrap().vpn_enabled = enabled;
                save(&state.config_path, &state.config.lock().unwrap()).unwrap();
                let saved = fs::read(&state.config_path).unwrap();
                let mut stale = fixture_routing();
                stale.default_target = crate::model::RouteTarget::Block;
                *state.routing.write().unwrap() =
                    RoutingPolicy::compile(stale, state.geodata.clone()).unwrap();
                state.fake_dns.set_vpn_enabled(!enabled);

                let error = reconcile(state).unwrap_err();
                assert!(error.to_string().contains(if failure == "sqlite" {
                    "SQLite"
                } else {
                    failure
                }));
                RUNNER.with_borrow(|runner| {
                    let runner = runner.as_ref().unwrap();
                    assert_eq!(
                        runner.calls,
                        if failure == "network" {
                            vec!["guard", "policy", "dns-cleanup", "network"]
                        } else {
                            vec!["guard", "policy"]
                        }
                    );
                    assert!(runner.guarded);
                });
                assert!(state.routing_degraded.load(Ordering::Relaxed));
                assert_eq!(fs::read(&history).unwrap(), b"legacy history");
                assert_eq!(fs::read(&state.config_path).unwrap(), saved);
                let published = failure == "network";
                assert_eq!(
                    state.fake_dns.vpn_enabled(),
                    if published { enabled } else { !enabled }
                );
                assert_eq!(
                    state.routing.read().unwrap().config().default_target,
                    if published {
                        crate::model::RouteTarget::Vpn
                    } else {
                        crate::model::RouteTarget::Block
                    }
                );
                assert_eq!(state.fake_dns.count(), usize::from(populated));
                if populated {
                    let target: i64 = connection
                        .query_row("SELECT target FROM fake_dns", [], |row| row.get(0))
                        .unwrap();
                    // Persist intent even with VPN disabled, just like the nft sets.
                    assert_eq!(target, if published { 2 } else { 3 });
                }
            }
        }
    }
}

#[test]
fn persistence_failure_rolls_back_every_live_server_path() {
    if run_in_child() {
        return;
    }
    for operation in ["select", "edit", "import", "delete"] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let before = fs::read(&state.config_path).unwrap();
        fs::create_dir(state.config_path.with_extension("json.tmp")).unwrap();
        assert!(mutate(state, operation).is_err(), "{operation}");
        RUNNER.with_borrow(|runner| {
            let runner = runner.as_ref().unwrap();
            assert_eq!(
                runner.calls,
                [
                    "snapshot", "guard", "network", "policy", "network", "policy", "clear"
                ],
                "{operation}"
            );
            assert!(!runner.guarded);
        });
        assert!(!state.routing_degraded.load(Ordering::Relaxed));
        assert_eq!(before, fs::read(&state.config_path).unwrap());
        assert_eq!(
            before,
            serde_json::to_vec_pretty(&*state.config.lock().unwrap()).unwrap()
        );
    }
}

#[test]
fn last_server_requires_vpn_off_and_import_does_not_enable_it() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    state.config.lock().unwrap().servers.truncate(1);
    assert!(delete_server(state, &server('A').public_key).is_err());
    RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().calls.is_empty()));
    set_mode(state, false).unwrap();
    delete_server(state, &server('A').public_key).unwrap();
    mutate(state, "import").unwrap();
    assert!(!state.config.lock().unwrap().vpn_enabled);
}

#[test]
fn clear_failure_and_failed_startup_reconcile_latch_degraded() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.push_back("clear"));
    assert!(mutate(state, "routing").is_err());
    assert!(state.routing_degraded.load(Ordering::Relaxed));
    // Model a new process encountering the retained kernel guard at startup.
    state.routing_degraded.store(false, Ordering::Relaxed);
    RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.push_back("guard"));
    assert!(reconcile(state).is_err());
    assert!(state.routing_degraded.load(Ordering::Relaxed));
    assert!(mutate(state, "same-mode").is_err());
    reconcile(state).unwrap();
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
}

#[test]
fn only_completed_full_reconcile_retires_legacy_history() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    let history = state
        .config_path
        .parent()
        .unwrap()
        .join("routing-legacy.json");
    fs::write(&history, b"legacy history").unwrap();
    mutate(state, "same-mode").unwrap();
    mutate(state, "routing").unwrap();
    mutate(state, "edit").unwrap();
    assert_eq!(fs::read(&history).unwrap(), b"legacy history");
    for step in ["network", "policy", "retire"] {
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().failures.push_back(step));
        assert!(reconcile(state).is_err());
        assert_eq!(fs::read(&history).unwrap(), b"legacy history");
        assert!(state.routing_degraded.load(Ordering::Relaxed));
        RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().guarded));
    }
    reconcile(state).unwrap();
    assert!(!history.exists());
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
    reconcile(state).unwrap(); // Missing history is already retired.
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
}

#[test]
fn history_removal_error_keeps_guard_and_blocks_partial_requests() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    let history = state
        .config_path
        .parent()
        .unwrap()
        .join("routing-legacy.json");
    fs::create_dir(&history).unwrap();
    let error = reconcile(state).unwrap_err();
    assert!(error.to_string().contains("failed to remove"));
    assert!(history.is_dir());
    assert!(state.routing_degraded.load(Ordering::Relaxed));
    RUNNER.with_borrow(|runner| {
        let runner = runner.as_ref().unwrap();
        assert_eq!(
            runner.calls,
            ["guard", "policy", "dns-cleanup", "network", "retire"]
        );
        assert!(runner.guarded);
    });
    assert!(mutate(state, "same-mode").is_err());
    fs::remove_dir(&history).unwrap();
    reconcile(state).unwrap();
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
}

#[test]
fn synchronous_mode_repairs_missed_hotplug_and_failure_retains_guard() {
    if run_in_child() {
        return;
    }
    for operation in ["reconcile", "edit", "import", "select", "delete"] {
        let fixture = Fixture::new();
        let state = &fixture.0;
        let directory = state.config_path.parent().unwrap();
        let route = directory.join("route");
        // The tunnel/peer boundary models an already-active tunnel: no ifup/hotplug.
        // Execute the real mode-command runner against this isolated shell stub.
        fs::write(
            &state.mode_command,
            r#"#!/bin/sh
set -eu
directory=${0%/*}
[ "$#" = 3 ] && [ "$2" = br-lan ] && [ "$3" = 192.168.8.0/24 ]
printf '%s\n' "$*" >> "$directory/mode-calls"
case "$1" in
    vpn) ;;
    tunnel-up)
        [ ! -e "$directory/fail-route" ] || exit 1
        printf '%s\n' 'default dev gt0 table 100 metric 10 proto 186' > "$directory/route"
        ;;
    *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&state.mode_command, fs::Permissions::from_mode(0o700)).unwrap();
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().real_network = true);
        assert!(!route.exists());
        if operation == "reconcile" {
            reconcile(state).unwrap();
        } else {
            mutate(state, operation).unwrap();
        }
        assert_eq!(
            fs::read_to_string(&route).unwrap(),
            "default dev gt0 table 100 metric 10 proto 186\n",
            "{operation}"
        );
        assert_eq!(
            fs::read_to_string(directory.join("mode-calls")).unwrap(),
            "vpn br-lan 192.168.8.0/24\ntunnel-up br-lan 192.168.8.0/24\n",
            "{operation}"
        );
        assert!(!state.routing_degraded.load(Ordering::Relaxed));
        RUNNER.with_borrow_mut(|runner| {
            let runner = runner.as_mut().unwrap();
            assert!(!runner.guarded);
            assert!(runner.calls.contains(&"select-peer"));
            runner.calls.clear();
        });

        fs::remove_file(&route).unwrap();
        fs::write(directory.join("fail-route"), b"").unwrap();
        let error = reconcile(state).unwrap_err();
        assert!(error.to_string().contains("tunnel-up"));
        assert!(!route.exists());
        assert!(state.routing_degraded.load(Ordering::Relaxed));
        RUNNER.with_borrow(|runner| {
            let runner = runner.as_ref().unwrap();
            assert_eq!(
                runner.calls,
                ["guard", "policy", "dns-cleanup", "network", "select-peer"]
            );
            assert!(runner.guarded);
        });
        assert!(mutate(state, "same-mode").is_err());
    }
}

#[test]
fn lifecycle_owner_rejects_all_writers_before_local_locks_and_release_allows_healing() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    let path = state.config_path.parent().unwrap().join("apply.lock");
    drop(crate::network::lock_apply(state).unwrap());
    // An independent descriptor models init's FD8, not a cloned agent handle.
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    owner.try_lock().unwrap();
    let before = fs::read(&state.config_path).unwrap();
    let _update = state.fake_dns.begin_update().unwrap();
    let config = state.config.lock().unwrap();
    for degraded in [false, true] {
        state.routing_degraded.store(degraded, Ordering::Relaxed);
        RUNNER.with_borrow_mut(|runner| runner.as_mut().unwrap().guarded = degraded);
        for operation in [
            "mode",
            "same-mode",
            "routing",
            "select",
            "same-server",
            "delete",
            "add",
            "edit",
            "import",
            "reconcile",
        ] {
            let result = if operation == "reconcile" {
                reconcile(state)
            } else {
                mutate(state, operation)
            };
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("lifecycle operation is in progress"),
                "{operation}"
            );
            assert_eq!(state.routing_degraded.load(Ordering::Relaxed), degraded);
        }
        RUNNER.with_borrow(|runner| {
            let runner = runner.as_ref().unwrap();
            assert!(runner.calls.is_empty());
            assert_eq!(runner.guarded, degraded);
        });
        assert_eq!(before, fs::read(&state.config_path).unwrap());
        assert_eq!(before, serde_json::to_vec_pretty(&*config).unwrap());
    }
    drop(config);
    drop(_update);
    // A fork copy, like try_clone, shares the locked open file description.
    let inherited = owner.try_clone().unwrap();
    drop(owner);
    assert!(
        reconcile(state)
            .unwrap_err()
            .to_string()
            .contains("lifecycle operation is in progress")
    );
    inherited.unlock().unwrap();
    drop(inherited);
    reconcile(state).unwrap();
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
    assert!(path.is_file()); // Never unlink the shared lock inode.
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let released = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    released.try_lock().unwrap();
}

#[test]
fn lifecycle_fence_rejects_symlinks_hardlinks_and_unsafe_permissions() {
    if run_in_child() {
        return;
    }
    let fixture = Fixture::new();
    let state = &fixture.0;
    let directory = state.config_path.parent().unwrap();
    let path = directory.join("apply.lock");
    let target = directory.join("untouched");
    fs::write(&target, b"untouched").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(reconcile(state).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
    fs::remove_file(&path).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&target, &path).unwrap();
    assert!(reconcile(state).is_err());
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(reconcile(state).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(reconcile(state).is_err());
    fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = directory.join("alias");
    std::os::unix::fs::symlink(directory, &alias).unwrap();
    let mut aliased = state.clone();
    aliased.config_path = alias.join("config.json");
    assert!(reconcile(&aliased).is_err());
    RUNNER.with_borrow(|runner| assert!(runner.as_ref().unwrap().calls.is_empty()));
    assert!(!state.routing_degraded.load(Ordering::Relaxed));
    reconcile(state).unwrap();
}

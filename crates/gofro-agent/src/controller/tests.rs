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
    calls: Vec<&'static str>,
    failures: VecDeque<&'static str>,
    guarded: bool,
    real_network: bool,
    apply_lock: Option<std::path::PathBuf>,
}
thread_local! { static RUNNER: RefCell<Option<Runner>> = const { RefCell::new(None) }; }

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
                "guard" => runner.guarded = true,
                "clear" => {
                    assert!(runner.guarded);
                    runner.guarded = false;
                }
                "network" | "select-peer" | "policy" | "retire" => assert!(runner.guarded),
                "snapshot" => {}
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
                ["guard", "network", "policy", "retire", "clear"]
            );
            assert!(!runner.guarded);
        });
        assert!(!state.routing_degraded.load(Ordering::Relaxed));
        mutate(state, "same-mode").unwrap();
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
        assert_eq!(runner.calls, ["guard", "network", "policy", "retire"]);
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
            assert_eq!(runner.calls, ["guard", "network", "select-peer"]);
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

use super::*;
use std::{
    fs,
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        process::ExitStatusExt,
    },
    process::ExitStatus,
};

pub(super) const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEwCks2omLrfMrS1du13ol2Iwo4CoDhME50jxaLI7Mdq";

pub(crate) struct Fixture {
    pub(crate) state: AppState,
    pub(super) root: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        use std::sync::{Arc, Mutex, RwLock, atomic::AtomicBool};
        let root = fake_ssh_dir();
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let geodata = Arc::new(crate::geodata::GeoData::default());
        let config = ControllerConfig {
            vpn_enabled: false,
            active_server_key: None,
            servers: vec![],
            routing: crate::model::RoutingConfig {
                domain_rules: vec![],
                ip_rules: vec![],
                default_target: crate::model::RouteTarget::Vpn,
                mode: crate::model::RoutingMode::Rules,
                rule_order: None,
            },
        };
        let state = AppState {
            interface: "gt0".into(),
            lan: crate::model::LanContext {
                device: "br-lan".into(),
                address: "192.168.8.1".parse().unwrap(),
                subnet: "192.168.8.0/24".parse().unwrap(),
            },
            https_listen: "192.168.8.1:443".parse().unwrap(),
            dns_listen: "192.168.8.1:5353".parse().unwrap(),
            config_path: root.join("controller.json"),
            mode_command: root.join("never-execute"),
            management_dir: root.join("management"),
            routing: Arc::new(RwLock::new(
                crate::routing::RoutingPolicy::compile(config.routing.clone(), geodata.clone())
                    .unwrap(),
            )),
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(crate::stats::StatsTracker::default())),
            geodata,
            routing_degraded: Arc::new(AtomicBool::new(false)),
            fake_dns: Arc::new(
                crate::fake_dns::FakeDns::open(&root.join("routing.sqlite")).unwrap(),
            ),
            auth: Arc::new(
                crate::auth::Auth::open(root.join("password"), root.join("setup-code")).unwrap(),
            ),
            managed_operations: Arc::new(Mutex::new(())),
        };
        crate::config::save(&state.config_path, &state.config.lock().unwrap()).unwrap();
        Self { state, root }
    }

    pub(super) fn server(&self) -> crate::model::ServerProfile {
        crate::model::ServerProfile {
            name: "VPS".into(),
            emoji: String::new(),
            endpoint: "1.1.1.1:8443".into(),
            public_key: format!("{}=", "A".repeat(43)),
            client_tunnel_address: Some("10.202.0.2/32".into()),
            client_private_key: Some(format!("{}=", "B".repeat(43))),
            management: Some(ManagedServer {
                host: "1.1.1.1".into(),
                port: 22,
                host_key: KEY.into(),
            }),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(super) fn fake_ssh(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

pub(super) fn fake_ssh_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "gofro-ssh-{}-{}",
        std::process::id(),
        TEMPORARY.fetch_add(1, Ordering::Relaxed),
    ))
}

#[test]
fn parses_exact_version() {
    assert_eq!(parse_version("gofro-server 0.5.11\n").unwrap(), "0.5.11");
    assert!(parse_version("0.5.11").is_err());
}

#[test]
fn ordinary_writes_mark_only_confirmed_commits_when_second_ssh_fails() {
    const CHILD: &str = "GOFRO_OUTCOME_TEST_DIR";
    let Ok(directory) = std::env::var(CHILD) else {
        let fixture = Fixture::new();
        fake_ssh(
            &fixture.root,
            "ssh",
            r#"#!/bin/sh
for remote; do :; done
/bin/cat >/dev/null
printf '%s\n' "$remote" >>"$GOFRO_OUTCOME_TEST_DIR/calls"
if [ -f "$GOFRO_OUTCOME_TEST_DIR/reject" ]; then exit 124; fi
case "$remote" in managed-status|version) exit 1;; esac
exit 0
"#,
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "managed::tests::ordinary_writes_mark_only_confirmed_commits_when_second_ssh_fails",
                "--nocapture",
            ])
            .env(CHILD, &fixture.root)
            // Only the fake executable is reachable; no real SSH or network fallback.
            .env("PATH", &fixture.root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let directory = PathBuf::from(directory);
    let fixture = Fixture::new();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&fixture.state.management_dir)
        .unwrap();
    fs::write(fixture.state.management_dir.join("id_ed25519"), "test-only").unwrap();
    let server = fixture.server();
    let public_key = server.public_key.clone();
    fixture.state.config.lock().unwrap().servers.push(server);
    for rejected in [false, true] {
        if rejected {
            fs::write(directory.join("reject"), "").unwrap();
        }
        for operation in ["update", "restart", "create", "rename", "revoke"] {
            fs::write(directory.join("calls"), "").unwrap();
            let result = match operation {
                "update" => update(&fixture.state, &public_key).map(|_| ()),
                "restart" => restart(&fixture.state, &public_key).map(|_| ()),
                "create" => create_friend(&fixture.state, &public_key, "Friend").map(|_| ()),
                "rename" => {
                    rename_friend(&fixture.state, &public_key, &public_key, "Friend").map(|_| ())
                }
                "revoke" => revoke_friend(&fixture.state, &public_key, &public_key).map(|_| ()),
                _ => unreachable!(),
            };
            let error = result.unwrap_err();
            assert_eq!(
                error.is::<CommittedRefreshFailed>(),
                !rejected,
                "{operation}: {error}"
            );
            let calls = fs::read_to_string(directory.join("calls")).unwrap();
            let calls: Vec<_> = calls.lines().collect();
            assert_eq!(
                calls.len(),
                if rejected { 1 } else { 2 },
                "{operation}: {calls:?}"
            );
            if !rejected {
                assert_eq!(
                    calls[1],
                    if operation == "update" {
                        "version"
                    } else {
                        "managed-status"
                    }
                );
            }
        }
    }
}

#[test]
fn validates_typed_managed_status_and_upgrade_guidance() {
    let key = "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
    let status = ManagedServerStatus {
        version: "0.5.14".into(),
        peers: vec![wireguard_status::managed::FriendPeer {
            public_key: key.into(),
            name: "Friend".into(),
            revoked: false,
            can_share: true,
        }],
    };
    assert!(validate_managed_status(&status).is_ok());
    let mut invalid = status.clone();
    invalid.peers[0].revoked = true;
    assert!(validate_managed_status(&invalid).is_err());
    assert!(
        ssh::ssh_command_error(ExitStatus::from_raw(126 << 8), "", "capabilities")
            .to_string()
            .contains("устаревшую")
    );
}

#[test]
fn unmanaged_servers_are_rejected_before_ssh() {
    let key = "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
    let config = ControllerConfig {
        vpn_enabled: false,
        active_server_key: None,
        servers: vec![crate::model::ServerProfile {
            name: "Unmanaged".into(),
            emoji: String::new(),
            endpoint: "vpn.example.com:8443".into(),
            public_key: key.into(),
            client_tunnel_address: None,
            client_private_key: None,
            management: None,
        }],
        routing: crate::model::RoutingConfig::default(),
    };
    assert!(managed_server_config(&config, key).is_err());
}

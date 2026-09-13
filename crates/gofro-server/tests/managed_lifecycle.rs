#![cfg(debug_assertions)]

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

const OWNER: &str = "Ooooooooooooooooooooooooooooooooooooooooooo=";
const FIRST: &str = "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=";
const SECOND: &str = "Bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb=";
const LEGACY: &str = "Lllllllllllllllllllllllllllllllllllllllllll=";
static FIXTURES: AtomicUsize = AtomicUsize::new(0);

#[test]
fn legacy_profile_creation_still_accepts_hostname_endpoints() {
    let fixture = Fixture::new();
    let result = fixture.run("create-profile", ["--endpoint", "vpn.test:51820"], None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Endpoint = vpn.test:51820"));
    assert!(fixture.state().contains(OWNER));
}

#[test]
fn router_owner_metadata_migrates_legacy_and_never_advertises_a_lan() {
    let fixture = Fixture::new();
    let created = fixture.run("create-router-profile", ["198.51.100.1:8443"], None);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let profile = String::from_utf8(created.stdout).unwrap();
    assert!(profile.contains("Address = 10.202.0.3/32"));
    assert!(!profile.contains("10.203"));
    assert!(fixture.state().contains(&format!("{FIRST}\t10.202.0.3/32")));
    let metadata = fs::read_to_string(fixture.path("friends/gt0.json")).unwrap();
    assert!(metadata.contains("\"schema\":2"));
    assert!(metadata.contains(OWNER));
    assert!(metadata.contains(FIRST));
    assert!(!fixture.run("revoke-friend", [OWNER], None).status.success());
    assert!(!fixture.run("revoke-friend", [FIRST], None).status.success());
    assert!(
        fixture
            .run("remove-router-peer", [FIRST], None)
            .status
            .success()
    );
    assert!(fixture.state().contains(OWNER));
    assert!(!fixture.state().contains(FIRST));
}

#[test]
fn schema_two_requires_owner_metadata() {
    let fixture = Fixture::new();
    let friends = fixture.path("friends");
    fs::create_dir(&friends).unwrap();
    fs::set_permissions(&friends, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(friends.join("gt0.json"), r#"{"schema":2,"records":[]}"#).unwrap();
    fs::set_permissions(friends.join("gt0.json"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!fixture.run("managed-status", [], None).status.success());
}

#[test]
fn owner_save_failures_rollback_wireguard_and_metadata() {
    let fixture = Fixture::new();
    fs::write(fixture.path("fail-save"), "1").unwrap();
    assert!(
        !fixture
            .run("create-router-profile", ["198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(!fixture.state().contains(FIRST));
    let metadata = fs::read_to_string(fixture.path("friends/gt0.json")).unwrap();
    assert!(metadata.contains(OWNER));
    // Both the save and its rollback fail, so identity must survive a possible disk restore.
    assert!(metadata.contains(FIRST));
    fs::remove_file(fixture.path("fail-save")).unwrap();
    assert!(
        fixture
            .run("create-router-profile", ["198.51.100.1:8443"], None)
            .status
            .success()
    );
    fs::write(fixture.path("fail-save"), "1").unwrap();
    assert!(
        !fixture
            .run("remove-router-peer", [SECOND], None)
            .status
            .success()
    );
    assert!(fixture.state().contains(SECOND));
    assert!(
        fs::read_to_string(fixture.path("friends/gt0.json"))
            .unwrap()
            .contains(SECOND)
    );
}

#[test]
fn manages_friends_without_touching_other_peers_or_secrets() {
    let fixture = Fixture::new();

    let created = fixture.run(
        "create-friend",
        ["198.51.100.1:8443"],
        Some(r#"{"name":"Alice"}"#),
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created = String::from_utf8(created.stdout).unwrap();
    assert!(created.contains("PrivateKey = Ppppppppppppppppppppppppppppppppppppppppppp="));
    assert_eq!(fixture.mode("friends/gt0.json"), 0o600);

    let exported = fixture.run("friend-profile", [FIRST, "198.51.100.1:8443"], None);
    assert!(exported.status.success());
    assert_eq!(created, String::from_utf8(exported.stdout).unwrap());
    assert!(
        fixture
            .run("friend-profile", [FIRST, "[2001:db8::1]:8443"], None)
            .status
            .success()
    );
    for endpoint in ["[:::]:8443", "[2001:0db8::1]:8443", "198.051.100.1:8443"] {
        assert!(
            !fixture
                .run("friend-profile", [FIRST, endpoint], None)
                .status
                .success(),
            "accepted noncanonical managed endpoint: {endpoint}"
        );
    }
    let metadata = fixture.path("friends/gt0.json");
    let original_metadata = fs::read_to_string(&metadata).unwrap();
    fs::write(
        &metadata,
        original_metadata.replace(
            "Ppppppppppppppppppppppppppppppppppppppppppp=",
            "Qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq=",
        ),
    )
    .unwrap();
    let mismatch = fixture.run("friend-profile", [FIRST, "198.51.100.1:8443"], None);
    assert!(!mismatch.status.success());
    assert!(!String::from_utf8_lossy(&mismatch.stderr).contains("Qqqq"));
    fs::write(&metadata, original_metadata).unwrap();

    assert!(
        fixture
            .run(
                "create-friend",
                ["198.51.100.1:8443"],
                Some(r#"{"name":"Bob"}"#)
            )
            .status
            .success()
    );
    assert!(
        !fixture
            .run("rename-friend", [SECOND], Some(r#"{"name":"Alice"}"#))
            .status
            .success()
    );
    assert!(
        fixture
            .run("rename-friend", [SECOND], Some(r#"{"name":"Bobby"}"#))
            .status
            .success()
    );

    assert!(fixture.run("revoke-friend", [FIRST], None).status.success());
    assert!(fixture.state().contains(SECOND));
    assert!(fixture.state().contains(OWNER));
    assert!(
        !fixture
            .run("friend-profile", [FIRST, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(
        fixture
            .run("friend-profile", [SECOND, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(!fixture.run("revoke-friend", [OWNER], None).status.success());
    assert!(
        !fixture
            .run("friend-profile", [OWNER, "198.51.100.1:8443"], None)
            .status
            .success()
    );

    fixture.append_state(&format!("{LEGACY}\t10.202.0.9/32\n"));
    let listed = fixture.run("managed-status", [], None);
    assert!(
        String::from_utf8(listed.stdout)
            .unwrap()
            .contains("10.202.0.9/32")
    );
    assert!(
        !fixture
            .run(
                "create-friend",
                ["198.51.100.1:8443"],
                Some(r#"{"name":"10.202.0.9/32"}"#),
            )
            .status
            .success()
    );
    assert!(
        !fixture
            .run(
                "rename-friend",
                [SECOND],
                Some(r#"{"name":"10.202.0.9/32"}"#)
            )
            .status
            .success()
    );
    assert!(
        !fixture
            .run("friend-profile", [LEGACY, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(
        fixture
            .run("rename-friend", [LEGACY], Some(r#"{"name":"Old friend"}"#))
            .status
            .success()
    );
    assert!(
        !fixture
            .run("friend-profile", [LEGACY, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(
        fixture
            .run("revoke-friend", [LEGACY], None)
            .status
            .success()
    );

    fs::write(fixture.path("friends/gt0.json"), "broken").unwrap();
    let corrupt = fixture.run("managed-status", [], None);
    assert!(!corrupt.status.success());
    assert!(!String::from_utf8_lossy(&corrupt.stderr).contains("private-"));
}

#[test]
fn save_failure_rolls_back_and_restart_keeps_peers() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .run(
                "create-friend",
                ["198.51.100.1:8443"],
                Some(r#"{"name":"Alice"}"#)
            )
            .status
            .success()
    );
    let before_state = fixture.state();
    let before_metadata = fs::read(fixture.path("friends/gt0.json")).unwrap();
    fs::write(fixture.path("fail-save"), "1").unwrap();
    let failed = fixture.run("revoke-friend", [FIRST], None);
    assert!(!failed.status.success());
    assert_eq!(fixture.state(), before_state);
    assert_eq!(
        fs::read(fixture.path("friends/gt0.json")).unwrap(),
        before_metadata
    );
    fs::remove_file(fixture.path("fail-save")).unwrap();
    assert!(fixture.run("restart-vpn", [], None).status.success());
    assert!(fixture.state().contains(FIRST));
    assert!(fixture.state().contains(OWNER));
}

#[test]
fn revoke_retry_persists_absence_after_save_and_rollback_fail() {
    for known in [true, false] {
        let fixture = Fixture::new();
        assert!(fixture.run("managed-status", [], None).status.success());
        fixture.append_state(&format!(
            "{FIRST}\t10.202.0.3/32\n{SECOND}\t10.202.0.4/32\n{LEGACY}\t192.0.2.0/24\n"
        ));
        if known {
            assert!(
                fixture
                    .run("rename-friend", [FIRST], Some(r#"{"name":"Alice"}"#))
                    .status
                    .success()
            );
        }
        let before = fixture.state();
        fs::write(fixture.path("saved"), &before).unwrap();
        let remaining = before.replace(&format!("{FIRST}\t10.202.0.3/32\n"), "");
        fs::write(fixture.path("fail-save"), "1").unwrap();
        fs::write(fixture.path("fail-setconf"), "1").unwrap();

        let failed = fixture.run("revoke-friend", [FIRST], None);
        assert!(!failed.status.success());
        assert!(String::from_utf8_lossy(&failed.stderr).contains("rollback failed"));
        assert_eq!(fixture.state(), remaining);
        assert_eq!(fs::read_to_string(fixture.path("saved")).unwrap(), before);
        let tombstone = fs::read(fixture.path("friends/gt0.json")).unwrap();
        assert!(String::from_utf8_lossy(&tombstone).contains(FIRST));

        let retry = fixture.run("revoke-friend", [FIRST], None);
        assert!(
            !retry.status.success(),
            "acknowledged revoke without saving"
        );
        assert!(String::from_utf8_lossy(&retry.stderr).contains("wg-quick"));
        assert_eq!(fixture.state(), remaining);
        assert_eq!(fs::read_to_string(fixture.path("saved")).unwrap(), before);
        assert_eq!(
            fs::read(fixture.path("friends/gt0.json")).unwrap(),
            tombstone
        );

        fs::remove_file(fixture.path("fail-save")).unwrap();
        assert!(fixture.run("revoke-friend", [FIRST], None).status.success());
        assert_eq!(
            fs::read_to_string(fixture.path("saved")).unwrap(),
            remaining
        );
        // Reload only persisted state: restart-vpn would save first and mask the bug.
        fs::copy(fixture.path("saved"), fixture.path("state")).unwrap();
        assert_eq!(fixture.state(), remaining);
        assert_eq!(
            fs::read(fixture.path("friends/gt0.json")).unwrap(),
            tombstone
        );
        let status = fixture.run("managed-status", [], None);
        assert!(status.status.success());
        let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        let revoked = status["peers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|peer| peer["public_key"] == FIRST)
            .unwrap();
        assert_eq!(revoked["revoked"], true);
        assert_eq!(revoked["can_share"], false);
        assert!(!fixture.run("revoke-friend", [OWNER], None).status.success());
        assert_eq!(fixture.state(), remaining);
    }
}

#[test]
fn friend_operations_reuse_one_wireguard_snapshot() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .run(
                "create-friend",
                ["198.51.100.1:8443"],
                Some(r#"{"name":"Alice"}"#)
            )
            .status
            .success()
    );
    for command in [
        "rename-friend",
        "friend-profile",
        "revoke-friend",
        "revoke-friend",
    ] {
        fs::write(fixture.path("reads"), "").unwrap();
        let result = match command {
            "rename-friend" => fixture.run(command, [FIRST], Some(r#"{"name":"Renamed"}"#)),
            "friend-profile" => fixture.run(command, [FIRST, "198.51.100.1:8443"], None),
            _ => fixture.run(command, [FIRST], None),
        };
        assert!(
            result.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_to_string(fixture.path("reads")).unwrap(),
            "read\n",
            "{command}"
        );
        if command == "friend-profile" {
            assert!(String::from_utf8_lossy(&result.stdout).contains("Address = 10.202.0.3/32"));
        }
    }
}

#[test]
fn hides_metadata_for_owner_or_other_shapes_and_rejects_unsafe_storage() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .run(
                "create-friend",
                ["198.51.100.1:8443"],
                Some(r#"{"name":"Alice"}"#),
            )
            .status
            .success()
    );
    fs::write(
        fixture.path("state"),
        format!("{OWNER}\t10.202.0.2/32 10.203.1.0/24\n{FIRST}\t10.203.1.0/24 10.202.0.2/32\n"),
    )
    .unwrap();
    let status = fixture.run("managed-status", [], None);
    assert!(status.status.success());
    assert!(!String::from_utf8(status.stdout).unwrap().contains(FIRST));
    assert!(!fixture.run("revoke-friend", [FIRST], None).status.success());
    assert!(
        !fixture
            .run("friend-profile", [FIRST, "198.51.100.1:8443"], None)
            .status
            .success()
    );

    let metadata = fixture.path("friends/gt0.json");
    fs::set_permissions(&metadata, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fixture.run("managed-status", [], None).status.success());
    fs::set_permissions(&metadata, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&metadata, "x".repeat(65 * 1024)).unwrap();
    assert!(!fixture.run("managed-status", [], None).status.success());
    fs::write(
        &metadata,
        format!(
            r#"{{"schema":1,"records":[{{"public_key":"{FIRST}","name":"Alice","private_key":"secret-private-key"}}]}}"#
        ),
    )
    .unwrap();
    let corrupt = fixture.run("managed-status", [], None);
    assert!(!corrupt.status.success());
    assert!(!String::from_utf8_lossy(&corrupt.stderr).contains("secret-private-key"));
}

#[test]
fn rejects_metadata_symlinks_without_leaking_contents() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let friends = fixture.path("friends");
    fs::create_dir(&friends).unwrap();
    fs::set_permissions(&friends, fs::Permissions::from_mode(0o700)).unwrap();
    let target = fixture.path("target.json");
    fs::write(&target, r#"{"schema":1,"records":[]}"#).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, friends.join("gt0.json")).unwrap();
    let result = fixture.run("managed-status", [], None);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("schema"));

    let directory_fixture = Fixture::new();
    let target_directory = directory_fixture.path("target-directory");
    fs::create_dir(&target_directory).unwrap();
    fs::set_permissions(&target_directory, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&target_directory, directory_fixture.path("friends")).unwrap();
    assert!(
        !directory_fixture
            .run("managed-status", [], None)
            .status
            .success()
    );
}

#[test]
fn legacy_subnet_compatibility_preserves_identity_without_lan_routes() {
    let fixture = Fixture::new();
    let created = fixture.run(
        "create-profile",
        [
            "--endpoint",
            "vpn.test:51820",
            "--tunnel-ip",
            "10.202.0.8/32",
            "--subnet",
            "10.203.1.0/24",
        ],
        None,
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(
        fixture
            .state()
            .contains(&format!("{FIRST}\t10.202.0.8/32\n"))
    );
    assert!(!String::from_utf8_lossy(&created.stdout).contains("10.203"));
    assert!(
        fixture
            .run(
                "add-peer",
                [
                    "--public-key",
                    FIRST,
                    "--tunnel-ip",
                    "10.202.0.9/32",
                    "--subnet",
                    "10.203.1.0/24"
                ],
                None
            )
            .status
            .success()
    );
    assert!(
        !fixture
            .run("rename-friend", [FIRST], Some(r#"{"name":"Not a friend"}"#))
            .status
            .success()
    );
    assert!(
        !fixture
            .run("friend-profile", [FIRST, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(!fixture.run("revoke-friend", [FIRST], None).status.success());
    assert!(
        fixture
            .run(
                "remove-peer",
                ["--public-key", FIRST, "--subnet", "10.203.1.0/24"],
                None
            )
            .status
            .success()
    );
    fixture.append_state(&format!("{FIRST}\t10.202.0.9/32\n"));
    let status = fixture.run("managed-status", [], None);
    assert!(status.status.success());
    assert!(!String::from_utf8_lossy(&status.stdout).contains(FIRST));
    assert!(!fixture.run("revoke-friend", [FIRST], None).status.success());
    assert!(fixture.state().contains(OWNER));
    assert!(!fixture.path("forbidden-command").exists());
}

#[test]
fn migration_retains_real_schema_one_friends_and_revoked_history() {
    let fixture = Fixture::new();
    fixture.append_state(&format!("{FIRST}\t10.202.0.3/32\n"));
    fixture.store(&format!(r#"{{"schema":1,"records":[{{"public_key":"{FIRST}","name":"Alice","private_key":"Ppppppppppppppppppppppppppppppppppppppppppp="}},{{"public_key":"{SECOND}","name":"Revoked"}}]}}"#));
    let state = fixture.state();
    let status = fixture.run("managed-status", [], None);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status["peers"],
        serde_json::json!([
            {"public_key": FIRST, "name": "Alice", "revoked": false, "can_share": true},
            {"public_key": SECOND, "name": "Revoked", "revoked": true, "can_share": false}
        ])
    );
    assert_eq!(fixture.state(), state);
    assert!(
        fixture
            .run("friend-profile", [FIRST, "198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(
        fs::read_to_string(fixture.path("friends/gt0.json"))
            .unwrap()
            .contains("\"schema\":2")
    );
}

#[test]
fn missing_or_ambiguous_metadata_never_discloses_or_revokes_owners() {
    let fixture = Fixture::new();
    fixture.append_state(&format!("{FIRST}\t10.202.0.3/32\n"));
    let before = fixture.state();
    for data in [
        None,
        Some("broken".to_owned()),
        Some(format!(
            r#"{{"schema":1,"records":[{{"public_key":"{OWNER}","name":"Ambiguous"}}]}}"#
        )),
    ] {
        if let Some(data) = &data {
            fixture.store(data);
        }
        for command in [
            "managed-status",
            "revoke-friend",
            "rename-friend",
            "friend-profile",
        ] {
            let result = match command {
                "managed-status" => fixture.run(command, [], None),
                "rename-friend" => fixture.run(command, [FIRST], Some(r#"{"name":"Bad"}"#)),
                "friend-profile" => fixture.run(command, [FIRST, "198.51.100.1:8443"], None),
                _ => fixture.run(command, [FIRST], None),
            };
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
        }
        assert_eq!(fixture.state(), before);
        if let Some(data) = data {
            assert_eq!(
                fs::read_to_string(fixture.path("friends/gt0.json")).unwrap(),
                data
            );
        }
    }
}

#[test]
fn failed_owner_rollback_keeps_live_peer_protected() {
    let fixture = Fixture::new();
    fs::write(fixture.path("fail-save"), "1").unwrap();
    fs::write(fixture.path("fail-setconf"), "1").unwrap();
    let result = fixture.run("create-router-profile", ["198.51.100.1:8443"], None);
    assert!(!result.status.success());
    assert!(fixture.state().contains(FIRST));
    let status = fixture.run("managed-status", [], None);
    assert!(status.status.success());
    assert!(!String::from_utf8_lossy(&status.stdout).contains(FIRST));
    assert!(!fixture.run("revoke-friend", [FIRST], None).status.success());
}

#[test]
fn successful_owner_rollback_restores_exact_state_and_metadata() {
    let fixture = Fixture::new();
    assert!(fixture.run("managed-status", [], None).status.success());
    let state = fixture.state();
    let metadata = fs::read(fixture.path("friends/gt0.json")).unwrap();
    fs::write(fixture.path("fail-save"), "once").unwrap();
    let result = fixture.run("create-router-profile", ["198.51.100.1:8443"], None);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(fixture.state(), state);
    assert_eq!(
        fs::read(fixture.path("friends/gt0.json")).unwrap(),
        metadata
    );
}

#[test]
fn owner_metadata_capacity_failure_does_not_mutate_wireguard() {
    let fixture = Fixture::new();
    let owners: Vec<_> = (0..11_154).map(|i| format!("{i:043}=")).collect();
    let metadata =
        serde_json::to_string(&serde_json::json!({"schema": 2, "records": [], "owners": owners}))
            .unwrap();
    assert!(metadata.len() <= 512 * 1024 && metadata.len() + 47 > 512 * 1024);
    fixture.store(&metadata);
    let state = fixture.state();
    let result = fixture.run("create-router-profile", ["198.51.100.1:8443"], None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("metadata is too large"));
    assert_eq!(fixture.state(), state);
    assert_eq!(
        fs::read_to_string(fixture.path("friends/gt0.json")).unwrap(),
        metadata
    );
}

#[test]
fn metadata_preflight_rejects_oversize_hardlinks_and_sensitive_parse_errors() {
    let fixture = Fixture::new();
    fixture.store(&" ".repeat(512 * 1024 + 1));
    let result = fixture.run("managed-status", [], None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsafe friend metadata"));
    fixture.store(r#"{"schema":2,"owners":[],"records":"secret-private-key"}"#);
    let result = fixture.run("managed-status", [], None);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("secret-private-key"));
    fixture.store(r#"{"schema":2,"owners":[],"records":[]}"#);
    fs::hard_link(
        fixture.path("friends/gt0.json"),
        fixture.path("linked.json"),
    )
    .unwrap();
    assert!(!fixture.run("managed-status", [], None).status.success());
}

#[test]
fn rejects_symlink_and_writable_locks_before_mutation() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(fixture.path("missing-target"), fixture.path("lock")).unwrap();
    assert!(
        !fixture
            .run("create-router-profile", ["198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(!fixture.path("missing-target").exists());
    fs::remove_file(fixture.path("lock")).unwrap();
    fs::write(fixture.path("target"), "untouched").unwrap();
    std::os::unix::fs::symlink(fixture.path("target"), fixture.path("lock")).unwrap();
    assert!(
        !fixture
            .run("create-router-profile", ["198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(fixture.path("target")).unwrap(),
        "untouched"
    );
    fs::remove_file(fixture.path("lock")).unwrap();
    fs::write(fixture.path("lock"), "").unwrap();
    fs::set_permissions(fixture.path("lock"), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        !fixture
            .run("create-router-profile", ["198.51.100.1:8443"], None)
            .status
            .success()
    );
    assert!(!fixture.path("friends").exists());
}

struct Fixture {
    root: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gofro-server-process-{}-{}",
            std::process::id(),
            FIXTURES.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            root.join("state"),
            format!("{OWNER}\t10.202.0.2/32 10.203.1.0/24\n"),
        )
        .unwrap();
        fs::write(root.join("counter"), "0\n").unwrap();
        for (name, source) in [
            ("id", "#!/bin/sh\nprintf '0\\n'\n"),
            ("wg-quick", WG_QUICK),
            ("wg", WG),
            ("systemctl", "#!/bin/sh\nexit 0\n"),
            ("ip", "#!/bin/sh\ntouch \"$GOFRO_FAKE_FORBIDDEN\"\nexit 1\n"),
        ] {
            let path = bin.join(name);
            fs::write(&path, source).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { root, bin }
    }

    fn run<const N: usize>(&self, command: &str, args: [&str; N], input: Option<&str>) -> Output {
        let mut process = Command::new(env!("CARGO_BIN_EXE_gofro-server"));
        process
            .arg("--interface")
            .arg("gt0")
            .arg(command)
            .args(args)
            .env("GOFRO_TESTING", "1")
            .env(
                "GOFRO_TEST_UID",
                fs::metadata(&self.root).unwrap().uid().to_string(),
            )
            .env("GOFRO_TEST_FRIENDS_DIR", self.path("friends"))
            .env("GOFRO_TEST_LOCK", self.path("lock"))
            .env("GOFRO_FAKE_WG_STATE", self.path("state"))
            .env("GOFRO_FAKE_SAVE", self.path("saved"))
            .env("GOFRO_FAKE_COUNTER", self.path("counter"))
            .env("GOFRO_FAKE_READS", self.path("reads"))
            .env("GOFRO_FAKE_FAIL_SAVE", self.path("fail-save"))
            .env("GOFRO_FAKE_FAIL_SETCONF", self.path("fail-setconf"))
            .env("GOFRO_FAKE_FORBIDDEN", self.path("forbidden-command"))
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            );
        if let Some(input) = input {
            use std::io::Write;
            process
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = process.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            return child.wait_with_output().unwrap();
        }
        process.output().unwrap()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
    fn store(&self, data: &str) {
        fs::create_dir_all(self.path("friends")).unwrap();
        fs::set_permissions(self.path("friends"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(self.path("friends/gt0.json"), data).unwrap();
        fs::set_permissions(
            self.path("friends/gt0.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    fn state(&self) -> String {
        fs::read_to_string(self.path("state")).unwrap()
    }
    fn append_state(&self, value: &str) {
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(self.path("state"))
            .unwrap()
            .write_all(value.as_bytes())
            .unwrap();
    }
    fn mode(&self, name: &str) -> u32 {
        fs::metadata(self.path(name)).unwrap().permissions().mode() & 0o777
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const WG_QUICK: &str = r#"#!/bin/sh
[ "$1" = save ] || exit 1
if [ -e "$GOFRO_FAKE_FAIL_SAVE" ]; then
  [ "$(cat "$GOFRO_FAKE_FAIL_SAVE")" = once ] && rm "$GOFRO_FAKE_FAIL_SAVE"
  exit 1
fi
cp "$GOFRO_FAKE_WG_STATE" "$GOFRO_FAKE_SAVE"
"#;
const WG: &str = r#"#!/bin/sh
state=$GOFRO_FAKE_WG_STATE
case "$1 $2" in
  "show gt0")
    case "$3" in
      allowed-ips) printf 'read\n' >> "$GOFRO_FAKE_READS"; cat "$state" ;;
      public-key) printf 'Sssssssssssssssssssssssssssssssssssssssssss=\n' ;;
    esac ;;
  "showconf gt0") cat "$state" ;;
  "genkey ") n=$(cat "$GOFRO_FAKE_COUNTER"); n=$((n + 1)); printf '%s\n' "$n" > "$GOFRO_FAKE_COUNTER"; [ "$n" = 1 ] && printf 'Ppppppppppppppppppppppppppppppppppppppppppp=\n' || printf 'Qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq=\n' ;;
  "pubkey ") read -r private; case "$private" in Ppppppppppppppppppppppppppppppppppppppppppp=) printf 'Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=\n' ;; Qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq=) printf 'Bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb=\n' ;; *) exit 1 ;; esac ;;
  "set gt0")
    if [ "$3" = peer ] && [ "$5" = remove ]; then
      key=$4
      while IFS= read -r line; do case "$line" in "$key"*) ;; *) printf '%s\n' "$line";; esac; done < "$state" > "$state.tmp"
    elif [ "$3" = peer ]; then
      key=$4; ip=$6
      while IFS= read -r line; do case "$line" in "$key"*) ;; *) printf '%s\n' "$line";; esac; done < "$state" > "$state.tmp"
      printf '%s\t%s\n' "$key" "$ip" >> "$state.tmp"
    fi
    mv "$state.tmp" "$state" ;;
  "setconf gt0") [ -e "$GOFRO_FAKE_FAIL_SETCONF" ] && exit 1; cat > "$state" ;;
  *) exit 1 ;;
esac
"#;

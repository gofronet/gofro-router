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
            .env("GOFRO_FAKE_FAIL_SAVE", self.path("fail-save"))
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

const WG_QUICK: &str = "#!/bin/sh\n[ \"$1\" = save ] || exit 1\n[ -e \"$GOFRO_FAKE_FAIL_SAVE\" ] && exit 1\ncp \"$GOFRO_FAKE_WG_STATE\" \"$GOFRO_FAKE_SAVE\"\n";
const WG: &str = r#"#!/bin/sh
state=$GOFRO_FAKE_WG_STATE
case "$1 $2" in
  "show gt0")
    case "$3" in
      allowed-ips) cat "$state" ;;
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
  "setconf gt0") cat > "$state" ;;
  *) exit 1 ;;
esac
"#;

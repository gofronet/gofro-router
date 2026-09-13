use std::fs;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use super::{
    UPGRADE_REQUIRED, endpoint,
    pins::{pin_host, preserve_host_pins},
    ssh::{
        key_ssh, known_hosts, management_key, password_ssh, probe, ssh_keygen, validate_password,
        validate_target,
    },
};
use crate::{
    AppState,
    config::{normalize_server_name, parse_server_profile},
    controller,
    model::{BootstrapStage, ControllerConfig, ManagedServer},
};

const SERVER_INSTALLER: &str = include_str!("../../../../deploy/server/gofro-server-install");

pub(crate) fn bootstrap(
    state: &AppState,
    mut name: String,
    host: String,
    port: u16,
    password: String,
    progress: &mut dyn FnMut(BootstrapStage),
) -> Result<()> {
    progress(BootstrapStage::Waiting);
    let _operation = state
        .managed_operations
        .try_lock()
        .map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => anyhow!(
                "A managed VPS operation is already running. This request was not queued; check its outcome before retrying."
            ),
            std::sync::TryLockError::Poisoned(_) => anyhow!("managed operation lock poisoned"),
        })?;
    validate_password(&password)?;
    normalize_server_name(&mut name)?;
    validate_target(&host, port)?;
    progress(BootstrapStage::HostKey);
    let config = state
        .config
        .lock()
        .map_err(|_| anyhow!("configuration lock poisoned"))?;
    preserve_host_pins(&state.management_dir, &config)?;
    if config.servers.iter().any(|server| {
        server
            .management
            .as_ref()
            .is_some_and(|managed| managed.host == host && managed.port == port)
    }) {
        bail!("управляемый сервер с этим host уже существует");
    }
    drop(config);
    let fresh = probe(host.clone(), port)?;
    let host_key = fresh.host_key;
    {
        let config = state
            .config
            .lock()
            .map_err(|_| anyhow!("configuration lock poisoned"))?;
        preserve_host_pins(&state.management_dir, &config)?;
        pin_host(&state.management_dir, &host, port, &host_key)?;
    }
    let private_key = management_key(&state.management_dir)?;
    let public_key = ssh_keygen(&[
        "-y",
        "-f",
        private_key
            .to_str()
            .context("invalid management key path")?,
    ])?;
    let known_hosts = known_hosts(&state.management_dir, &host, port, &host_key)?;
    let authorized = format!(
        "restrict,command=\"/usr/local/sbin/gofro-managed\" {}",
        public_key.trim()
    );
    let remote = bootstrap_script(&authorized);
    progress(BootstrapStage::Connect);
    let result = password_ssh(&host, port, &known_hosts, &password, &remote, progress);
    let _ = fs::remove_file(&known_hosts);
    result?;

    let capabilities = key_ssh(
        &host,
        port,
        &private_key,
        &state.management_dir,
        &host_key,
        "capabilities",
        None,
    )?;
    require_owner_protocol(&capabilities)?;

    progress(BootstrapStage::Profile);
    let profile = key_ssh(
        &host,
        port,
        &private_key,
        &state.management_dir,
        &host_key,
        &format!("create-router-profile {}", endpoint(&host)),
        None,
    )?;
    let client_public_key = profile_client_public_key(&profile).map_err(|_| anyhow!(
        "VPS returned an invalid router profile. Enrollment outcome is unknown; existing remote access was retained."
    ))?;
    let server = parse_server_profile(name, &profile).map_err(|_| anyhow!(
        "VPS returned an invalid router profile. Remote access was retained; check the server before retrying."
    ))?;
    let client_private_key = server
        .client_private_key
        .clone()
        .context("router profile has no private key")?;
    progress(BootstrapStage::Save);
    let result = {
        let mut server = server;
        server.management = Some(ManagedServer {
            host: host.clone(),
            port,
            host_key: host_key.clone(),
        });
        controller::add_server(state, server)
    };
    if let Err(error) = result {
        return Err(rollback_enrollment(
            state,
            &client_private_key,
            error,
            || {
                key_ssh(
                    &host,
                    port,
                    &private_key,
                    &state.management_dir,
                    &host_key,
                    &format!("remove-router-peer {client_public_key}"),
                    None,
                )
            },
        ));
    }
    Ok(())
}

fn rollback_enrollment(
    state: &AppState,
    private_key: &str,
    error: anyhow::Error,
    rollback: impl FnOnce() -> Result<String>,
) -> anyhow::Error {
    if !enrollment_uncommitted(state, private_key) {
        return anyhow!(
            "Local enrollment may already be saved, but activation failed. Remote access was retained. Refresh the server list and repair local routing before retrying."
        );
    }
    match rollback() {
        Ok(_) => error,
        Err(rollback) => anyhow!(
            "local server enrollment failed: {error:#}; remote peer rollback failed: {rollback:#}"
        ),
    }
}

fn enrollment_uncommitted(state: &AppState, private_key: &str) -> bool {
    let Ok(config) = state.config.lock() else {
        return false;
    };
    let Ok(saved) = fs::read(&state.config_path) else {
        return false;
    };
    let Ok(saved) = serde_json::from_slice::<ControllerConfig>(&saved) else {
        return false;
    };
    [&*config, &saved].iter().all(|config| {
        !config
            .servers
            .iter()
            .any(|server| server.client_private_key.as_deref() == Some(private_key))
    })
}

fn bootstrap_script(authorized: &str) -> String {
    format!(
        r#"set -eu
printf 'GOFRO_STAGE inspect\n'
compatible() {{
  [ -x /usr/local/bin/gofro-router-server ] &&
  [ -x /usr/local/sbin/gofro-managed ] &&
  [ "$(SSH_ORIGINAL_COMMAND=capabilities /usr/local/sbin/gofro-managed 2>/dev/null)" = '{{"owner_protocol":2}}' ]
}}
if ! compatible; then
  printf 'GOFRO_STAGE install\n'
  installer=$(mktemp /tmp/gofro-server-install.XXXXXX)
  trap 'rm -f "$installer"' EXIT
  chmod 700 "$installer"
  printf %s {} > "$installer"
  if [ -s /etc/gofro/version ]; then bash "$installer" >&2; else bash "$installer" --install >&2; fi
  compatible || exit 40
fi
[ -s /etc/gofro/version ] && [ -s /etc/wireguard/gt0.conf ] &&
  wg show gt0 public-key >/dev/null 2>&1 &&
  systemctl is-active --quiet wg-quick@gt0.service &&
  systemctl is-active --quiet gofro-relay.service || exit 41
printf 'GOFRO_STAGE authorize\n'
install -d -m 700 /root/.ssh
touch /root/.ssh/authorized_keys
chmod 600 /root/.ssh/authorized_keys
grep -qxF {} /root/.ssh/authorized_keys || printf '\n%s\n' {} >> /root/.ssh/authorized_keys
"#,
        shell_quote(SERVER_INSTALLER),
        shell_quote(authorized),
        shell_quote(authorized)
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    owner_protocol: u8,
}

fn require_owner_protocol(value: &str) -> Result<()> {
    let capabilities: Capabilities =
        serde_json::from_str(value).map_err(|_| anyhow!(UPGRADE_REQUIRED))?;
    if capabilities.owner_protocol != 2 {
        bail!(UPGRADE_REQUIRED);
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn profile_client_public_key(profile: &str) -> Result<&str> {
    let key = profile
        .lines()
        .find_map(|line| line.strip_prefix("# ClientPublicKey = "))
        .context("server profile has no client public key")?;
    let bytes = key.as_bytes();
    if bytes.len() != 44
        || bytes[43] != b'='
        || !bytes[..43]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        bail!("server profile has an invalid client public key");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::super::{
        ssh::run_password_ssh,
        tests::{Fixture, fake_ssh},
    };
    use super::*;
    use std::{process::Command, sync::mpsc, thread, time::Duration};

    #[test]
    fn bootstrap_rejects_busy_without_queuing_or_advancing_past_waiting() {
        let fixture = Fixture::new();
        let state = fixture.state.clone();
        let lock = fixture.state.managed_operations.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        let task = thread::spawn(move || {
            let mut stages = Vec::new();
            let result = bootstrap(
                &state,
                "VPS".into(),
                "1.1.1.1".into(),
                22,
                String::new(),
                &mut |stage| stages.push(stage),
            );
            let _ = sender.send((result, stages));
        });
        let result = receiver.recv_timeout(Duration::from_secs(30));
        drop(lock);
        task.join().unwrap();
        let (result, stages) = result.expect("bootstrap waited for the busy lock");
        assert!(result.unwrap_err().to_string().contains("not queued"));
        assert_eq!(stages, [BootstrapStage::Waiting]);
        assert!(!fixture.state.management_dir.exists());
    }

    #[test]
    fn rollback_preserves_peer_after_commit_or_when_commit_is_uncertain() {
        let fixture = Fixture::new();
        let server = fixture.server();
        let private = server.client_private_key.clone().unwrap();
        controller::add_server(&fixture.state, server).unwrap();
        // update_config commits before its final fallible guard release.
        let error = rollback_enrollment(
            &fixture.state,
            &private,
            anyhow!("forwarding guard remains installed"),
            || panic!("must not revoke committed peer"),
        );
        assert!(error.to_string().contains("Remote access was retained"));
        fixture.state.config.lock().unwrap().servers.clear();
        assert!(!enrollment_uncommitted(&fixture.state, &private)); // Disk still committed.
        fs::write(&fixture.state.config_path, "broken").unwrap();
        assert!(!enrollment_uncommitted(&fixture.state, &private));
        crate::config::save(
            &fixture.state.config_path,
            &fixture.state.config.lock().unwrap(),
        )
        .unwrap();
        let mut rollbacks = 0;
        rollback_enrollment(&fixture.state, &private, anyhow!("save failed"), || {
            rollbacks += 1;
            Ok(String::new())
        });
        assert_eq!(rollbacks, 1);
    }

    #[test]
    fn preflight_reuses_compatible_vps_and_rechecks_incompatible_release() {
        let fixture = Fixture::new();
        let root = &fixture.root;
        for path in [
            "usr/local/bin",
            "usr/local/sbin",
            "etc/gofro",
            "etc/wireguard",
            "root",
            "tmp",
            "bin",
        ] {
            fs::create_dir_all(root.join(path)).unwrap();
        }
        fake_ssh(
            &root.join("usr/local/bin"),
            "gofro-router-server",
            "#!/bin/sh\nexit 0\n",
        );
        fake_ssh(
            &root.join("usr/local/sbin"),
            "gofro-managed",
            "#!/bin/sh\n[ -f \"$FIXTURE/compatible\" ] || exit 126\nprintf '{\"owner_protocol\":2}\\n'\n",
        );
        fake_ssh(&root.join("bin"), "wg", "#!/bin/sh\nexit 0\n");
        fake_ssh(
            &root.join("bin"),
            "systemctl",
            "#!/bin/sh\n[ \"$1\" = is-active ] && [ \"$2\" = --quiet ] || exit 2\nshift 2\nfor unit do\n  [ ! -f \"$FIXTURE/inactive-$unit\" ] && exit 0\ndone\nexit 3\n",
        );
        fs::write(root.join("etc/gofro/version"), "0.5.15\n").unwrap();
        fs::write(
            root.join("etc/wireguard/gt0.conf"),
            "existing-private-config",
        )
        .unwrap();
        fs::write(root.join("compatible"), "").unwrap();
        let installer = "printf '%s\\n' \"$*\" >> \"$FIXTURE/installs\"; printf 'secret installer output\\n'; [ ! -f \"$FIXTURE/upgrade\" ] || touch \"$FIXTURE/compatible\"; exit 0";
        let script = bootstrap_script("restricted public key")
            .replace(&shell_quote(SERVER_INSTALLER), &shell_quote(installer))
            // Expand the fixture root in the shell so /tmp/gofro cannot rewrite it again.
            .replace("/usr/local/", "\"$FIXTURE\"/usr/local/")
            .replace("/etc/", "\"$FIXTURE\"/etc/")
            .replace("/root/", "\"$FIXTURE\"/root/")
            .replace("/tmp/gofro", "\"$FIXTURE\"/tmp/gofro");
        let run = || {
            let mut command = Command::new("bash");
            command
                .arg("-c")
                .arg(format!("IFS= read -r password\n{script}"))
                .env("FIXTURE", root)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        root.join("bin").display(),
                        std::env::var("PATH").unwrap()
                    ),
                );
            let mut stages = Vec::new();
            let result = run_password_ssh(
                command,
                "secret-password",
                &mut |stage| stages.push(stage),
                Duration::from_secs(5),
            );
            (result, stages)
        };
        let (result, stages) = run();
        result.unwrap();
        assert_eq!(stages, [BootstrapStage::Inspect, BootstrapStage::Authorize]);
        assert!(!root.join("installs").exists());
        for inactive in [
            vec!["wg-quick@gt0.service"],
            vec!["gofro-relay.service"],
            vec!["wg-quick@gt0.service", "gofro-relay.service"],
        ] {
            for unit in &inactive {
                fs::write(root.join(format!("inactive-{unit}")), "").unwrap();
            }
            let (result, stages) = run();
            assert!(
                result.unwrap_err().to_string().contains("not ready"),
                "{inactive:?}"
            );
            assert_eq!(stages, [BootstrapStage::Inspect]);
            assert!(!root.join("installs").exists());
            for unit in inactive {
                fs::remove_file(root.join(format!("inactive-{unit}"))).unwrap();
            }
        }
        run().0.unwrap();
        assert_eq!(
            fs::read_to_string(root.join("root/.ssh/authorized_keys"))
                .unwrap()
                .matches("restricted public key")
                .count(),
            1
        );
        fs::remove_file(root.join("compatible")).unwrap();
        let (result, stages) = run();
        assert!(result.unwrap_err().to_string().contains("upgrade required"));
        assert_eq!(stages, [BootstrapStage::Inspect, BootstrapStage::Install]);
        fs::write(root.join("upgrade"), "").unwrap();
        let (result, stages) = run();
        result.unwrap();
        assert_eq!(
            stages,
            [
                BootstrapStage::Inspect,
                BootstrapStage::Install,
                BootstrapStage::Authorize
            ]
        );
        fs::remove_file(root.join("etc/wireguard/gt0.conf")).unwrap();
        assert!(run().0.unwrap_err().to_string().contains("not ready"));
        assert_eq!(
            fs::read_to_string(root.join("installs"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn requires_exact_owner_protocol_two() {
        assert!(require_owner_protocol(r#"{"owner_protocol":2}"#).is_ok());
        assert!(require_owner_protocol(r#"{"owner_protocol":1}"#).is_err());
        assert!(require_owner_protocol(r#"{"owner_protocol":2,"extra":true}"#).is_err());
        assert!(require_owner_protocol("gofro-server 0.5.15").is_err());
    }

    #[test]
    fn quotes_shell_data_without_leaving_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{AppState, config::validate_server, model::ServerProfile};

const RELAY_ENDPOINT_PATH: &str = "/etc/gofro/relay-endpoint";
const RELAY_SERVICE: &str = "gofro-relay";
const RELAY_LOCAL_ENDPOINT: &str = "127.0.0.1:51822";
const SERVICE_COMMAND: &str = "/usr/libexec/gofro/service";
const TUNNEL_COMMAND: &str = "/usr/libexec/gofro/tunnel";
const TUNNEL_MTU: &str = "1280";
const LEGACY_TUNNEL_ADDRESS: &str = "10.202.0.2/32";
const DEVICE_PRIVATE_KEY: &str = "/etc/wireguard/client.key";

pub(crate) fn lock_apply(state: &AppState) -> Result<File> {
    lock_apply_path(&state.config_path)
}

pub(crate) fn lock_apply_path(config_path: &Path) -> Result<File> {
    let parent = config_path
        .parent()
        .context("configuration path has no parent")?;
    let directory =
        fs::symlink_metadata(parent).context("failed to inspect apply lock directory")?;
    if !directory.is_dir() || directory.mode() & 0o022 != 0 {
        bail!("unsafe apply lock directory");
    }
    // Production namespace is root-owned and not writable by other users. This
    // makes checking the existing leaf before opening safe from symlink swaps.
    // Unit fixtures instead live in the unprivileged test user's temporary tree.
    #[cfg(not(test))]
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            bail!("unsafe apply lock ancestor: {}", ancestor.display());
        }
    }
    let path = parent.join("apply.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).mode(0o600);
    let file = match options.create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.mode() & 0o7777 != 0o600
                || metadata.uid() != directory.uid()
            {
                bail!("unsafe apply lock file: {}", path.display());
            }
            options.create_new(false).open(&path)?
        }
        Err(error) => return Err(error).context("failed to create apply lock"),
    };
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!("network lifecycle operation is in progress"),
        Err(TryLockError::Error(error)) => Err(error).context("failed to acquire apply lock"),
    }
}

pub(crate) fn cleanup_dns_flows(state: &AppState) -> Result<()> {
    cleanup_dns_flows_on(&state.lan.device, state.dns_listen.port())
}

pub(crate) fn cleanup_dns_flows_on(lan_device: &str, dns_port: u16) -> Result<()> {
    cleanup_dns_flows_with(
        Path::new("/usr/libexec/gofro/dns-flows"),
        lan_device,
        dns_port,
    )
}

fn cleanup_dns_flows_with(command: &Path, lan_device: &str, dns_port: u16) -> Result<()> {
    run(Command::new(command).args(["cleanup", lan_device, &dns_port.to_string()]))?;
    Ok(())
}

pub(crate) struct Snapshot {
    endpoint: Option<Vec<u8>>,
    relay_active: bool,
    saved_tunnel: Option<Vec<u8>>,
    addresses: Option<Vec<String>>,
}

impl Snapshot {
    pub(crate) fn capture(state: &AppState) -> Result<Self> {
        Ok(Self {
            endpoint: read_optional(Path::new(RELAY_ENDPOINT_PATH))?,
            relay_active: relay_active()?,
            saved_tunnel: read_optional(Path::new(&format!(
                "/etc/wireguard/{}.conf",
                state.interface
            )))?,
            addresses: if service_active(&state.interface)? {
                Some(tunnel_addresses(&state.interface)?)
            } else {
                None
            },
        })
    }

    // Called under the controller guard after replaying the previous desired mode.
    pub(crate) fn restore(&self, state: &AppState) -> Result<()> {
        if let Some(addresses) = &self.addresses {
            replace_tunnel_addresses(&state.interface, addresses)?;
        }
        restore_file(
            Path::new(&format!("/etc/wireguard/{}.conf", state.interface)),
            self.saved_tunnel.as_deref(),
        )?;
        restore_file(Path::new(RELAY_ENDPOINT_PATH), self.endpoint.as_deref())?;
        run(Command::new(SERVICE_COMMAND).args([
            if self.relay_active { "restart" } else { "stop" },
            RELAY_SERVICE,
        ]))?;
        Ok(())
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn restore_file(path: &Path, contents: Option<&[u8]>) -> Result<()> {
    if let Some(contents) = contents {
        let temporary = path.with_extension("restore.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("failed to open {}", temporary.display()))?;
        file.write_all(contents)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
    } else {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to remove {}", path.display()));
            }
        }
    }
    Ok(())
}

fn run(command: &mut Command) -> Result<String> {
    let description = format!("{command:?}");
    let output = command
        .output()
        .with_context(|| format!("failed to run {description}"))?;
    if !output.status.success() {
        bail!(
            "{description} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("command returned non-UTF-8 output")
}

fn set_private_key(interface: &str, private_key: Option<&str>) -> Result<()> {
    if let Some(private_key) = private_key {
        let mut command = Command::new("wg");
        command.args(["set", interface, "private-key", "/dev/stdin"]);
        let description = format!("{command:?}");
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to run {description}"))?;
        let mut stdin = child
            .stdin
            .take()
            .context("failed to open WireGuard stdin")?;
        stdin
            .write_all(private_key.as_bytes())
            .context("failed to pass private key to WireGuard")?;
        stdin
            .write_all(b"\n")
            .context("failed to finish WireGuard private key")?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .context("failed to wait for WireGuard")?;
        if !output.status.success() {
            bail!(
                "{description} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    } else {
        run(Command::new("wg").args(["set", interface, "private-key", DEVICE_PRIVATE_KEY]))?;
    }
    Ok(())
}

fn tunnel_addresses(interface: &str) -> Result<Vec<String>> {
    let output = run(Command::new("ip").args([
        "-o", "-4", "address", "show", "dev", interface, "scope", "global",
    ]))?;
    output
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            fields
                .find(|field| *field == "inet")
                .and_then(|_| fields.next())
                .map(str::to_owned)
                .context("invalid ip address output")
        })
        .collect()
}

fn replace_tunnel_addresses(interface: &str, addresses: &[String]) -> Result<()> {
    run(Command::new("ip").args([
        "-4", "address", "flush", "dev", interface, "scope", "global",
    ]))?;
    for address in addresses {
        run(Command::new("ip").args(["-4", "address", "add", address, "dev", interface]))?;
    }
    Ok(())
}

pub(crate) fn stop_tunnel(interface: &str) -> Result<()> {
    set_tunnel(interface, "stop")
}

fn set_tunnel(interface: &str, action: &str) -> Result<()> {
    run(Command::new(TUNNEL_COMMAND).args([action, interface]))?;
    Ok(())
}

pub(crate) fn service_active(interface: &str) -> Result<bool> {
    Ok(Command::new(TUNNEL_COMMAND)
        .args(["status", interface])
        .status()
        .context("failed to query WireGuard service")?
        .success())
}

pub(crate) fn start_and_select(state: &AppState, server: &ServerProfile) -> Result<()> {
    let previous_endpoint = read_optional(Path::new(RELAY_ENDPOINT_PATH))?;
    let tunnel_was_active = service_active(&state.interface)?;
    let result = (|| {
        prepare_relay(server, !tunnel_was_active)?;
        if !tunnel_was_active {
            set_tunnel(&state.interface, "start")?;
        }
        run(Command::new("ip").args(["link", "set", "mtu", TUNNEL_MTU, "dev", &state.interface]))?;
        set_peer(&state.interface, server)
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        if !tunnel_was_active && let Err(rollback) = stop_tunnel(&state.interface) {
            rollback_errors.push(format!("tunnel stop: {rollback:#}"));
        }
        if let Err(rollback) = restore_relay(previous_endpoint) {
            rollback_errors.push(format!("relay: {rollback:#}"));
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(anyhow!(
            "server update failed: {error:#}; rollback failed: {}",
            rollback_errors.join("; ")
        ));
    }
    Ok(())
}

fn prepare_relay(server: &ServerProfile, force_restart: bool) -> Result<()> {
    validate_server(server)?;
    let endpoint = format!("{}\n", server.endpoint);
    let endpoint_changed =
        !fs::read(RELAY_ENDPOINT_PATH).is_ok_and(|current| current == endpoint.as_bytes());
    let relay_active = relay_active()?;
    if !endpoint_changed && !force_restart && relay_active {
        return Ok(());
    }
    if endpoint_changed {
        write_relay_endpoint(endpoint.as_bytes())?;
    }
    run(Command::new(SERVICE_COMMAND).args(["restart", RELAY_SERVICE]))?;
    Ok(())
}

fn relay_active() -> Result<bool> {
    Ok(Command::new(SERVICE_COMMAND)
        .args(["status", RELAY_SERVICE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to query relay service")?
        .success())
}

fn restore_relay(previous_endpoint: Option<Vec<u8>>) -> Result<()> {
    if let Some(endpoint) = previous_endpoint {
        write_relay_endpoint(&endpoint)?;
        run(Command::new(SERVICE_COMMAND).args(["restart", RELAY_SERVICE]))?;
    } else {
        match fs::remove_file(RELAY_ENDPOINT_PATH) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to remove relay endpoint"),
        }
        run(Command::new(SERVICE_COMMAND).args(["stop", RELAY_SERVICE]))?;
    }
    Ok(())
}

fn write_relay_endpoint(endpoint: &[u8]) -> Result<()> {
    restore_file(Path::new(RELAY_ENDPOINT_PATH), Some(endpoint))
}

fn set_peer(interface: &str, server: &ServerProfile) -> Result<()> {
    validate_server(server)?;
    let config = format!("/etc/wireguard/{interface}.conf");
    let peers = run(Command::new("wg").args(["show", interface, "peers"]))?;
    let previous_addresses = tunnel_addresses(interface)?;
    let next_addresses =
        next_tunnel_addresses(server.client_tunnel_address.as_deref(), &previous_addresses);
    let address_changed = previous_addresses != next_addresses;
    let result = (|| {
        set_private_key(interface, server.client_private_key.as_deref())?;
        run(Command::new("wg").args([
            "set",
            interface,
            "peer",
            &server.public_key,
            "endpoint",
            RELAY_LOCAL_ENDPOINT,
            "allowed-ips",
            "0.0.0.0/0",
            "persistent-keepalive",
            "10",
        ]))?;
        for peer in peers
            .split_whitespace()
            .filter(|peer| *peer != server.public_key)
        {
            run(Command::new("wg").args(["set", interface, "peer", peer, "remove"]))?;
        }
        if address_changed {
            replace_tunnel_addresses(interface, &next_addresses)?;
        }
        run(Command::new(TUNNEL_COMMAND).args(["save", interface]))?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = run(Command::new("wg").args(["setconf", interface, &config])) {
            rollback_errors.push(format!("WireGuard config: {rollback:#}"));
        }
        if address_changed
            && let Err(rollback) = replace_tunnel_addresses(interface, &previous_addresses)
        {
            rollback_errors.push(format!("tunnel address: {rollback:#}"));
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(anyhow!(
            "WireGuard peer update failed: {error:#}; rollback failed: {}",
            rollback_errors.join("; ")
        ));
    }
    Ok(())
}

fn next_tunnel_addresses(configured: Option<&str>, previous: &[String]) -> Vec<String> {
    configured.map_or_else(
        || {
            if previous.is_empty() {
                vec![LEGACY_TUNNEL_ADDRESS.to_owned()]
            } else {
                previous.to_vec()
            }
        },
        |address| vec![address.to_owned()],
    )
}

pub(crate) fn apply_mode(state: &AppState, mode: &str) -> Result<()> {
    let subnet = state.lan.subnet.to_string();
    run(Command::new(&state.mode_command).args([mode, &state.lan.device, &subnet]))?;
    Ok(())
}

pub(crate) fn retire_legacy_routing(state: &AppState) -> Result<()> {
    let parent = state
        .config_path
        .parent()
        .context("configuration path has no parent")?;
    restore_file(&parent.join("routing-legacy.json"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_flow_helper_receives_only_cleanup_lan_and_port_and_propagates_failure() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("gofro-dns-helper-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let helper = directory.join("dns-flows");
        fs::write(&helper, "#!/bin/sh\n[ \"$#\" = 3 ] && [ \"$1\" = cleanup ] && [ \"$2\" = br-home ] && [ \"$3\" = 5353 ]\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        cleanup_dns_flows_with(&helper, "br-home", 5353).unwrap();
        assert!(cleanup_dns_flows_with(&helper, "br-home", 53).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restores_saved_credentials_atomically_including_absent_files() {
        let directory = std::env::temp_dir().join(format!("gofro-network-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tunnel.conf");
        fs::write(&path, b"original").unwrap();
        fs::create_dir(path.with_extension("restore.tmp")).unwrap();
        assert!(restore_file(&path, Some(b"replacement")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::remove_dir(path.with_extension("restore.tmp")).unwrap();
        restore_file(&path, Some(b"replacement")).unwrap();
        assert_eq!(read_optional(&path).unwrap().unwrap(), b"replacement");
        restore_file(&path, None).unwrap();
        restore_file(&path, None).unwrap();
        assert!(read_optional(&path).unwrap().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preserves_legacy_tunnel_address() {
        let previous = vec!["10.202.0.4/32".to_owned()];
        assert_eq!(next_tunnel_addresses(None, &previous), previous);
        assert_eq!(
            next_tunnel_addresses(None, &[]),
            vec![LEGACY_TUNNEL_ADDRESS]
        );
        assert_eq!(
            next_tunnel_addresses(Some("10.202.0.5/32"), &previous),
            vec!["10.202.0.5/32"]
        );
    }
}

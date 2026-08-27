use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    AppState,
    config::validate_server,
    model::{ApNetwork, ServerProfile, WifiBand},
};

const RELAY_ENDPOINT_PATH: &str = "/etc/gofro/relay-endpoint";
const RELAY_SERVICE: &str = "gofro-relay";
const RELAY_LOCAL_ENDPOINT: &str = "127.0.0.1:51822";
const SERVICE_COMMAND: &str = "/usr/libexec/gofro/service";
const TUNNEL_COMMAND: &str = "/usr/libexec/gofro/tunnel";
const TUNNEL_MTU: &str = "1280";
const WIFI_COMMAND: &str = "/usr/libexec/gofro/wifi";
const DEVICE_PRIVATE_KEY: &str = "/etc/wireguard/client.key";

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

pub(crate) fn start_and_select(state: &AppState, server: &ServerProfile) -> Result<()> {
    configure_server(state, server, true)
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

pub(crate) fn select_server_peer(state: &AppState, server: &ServerProfile) -> Result<()> {
    configure_server(state, server, false)
}

fn configure_server(state: &AppState, server: &ServerProfile, start_tunnel: bool) -> Result<()> {
    let previous_endpoint = fs::read(RELAY_ENDPOINT_PATH).ok();
    let tunnel_was_active = service_active(&state.interface)?;
    let result = (|| {
        prepare_relay(server, start_tunnel && !tunnel_was_active)?;
        if start_tunnel {
            if !tunnel_was_active {
                set_tunnel(&state.interface, "start")?;
            }
            run(Command::new("ip").args([
                "link",
                "set",
                "mtu",
                TUNNEL_MTU,
                "dev",
                &state.interface,
            ]))?;
        }
        set_peer(&state.interface, server)
    })();
    if let Err(error) = result {
        if start_tunnel && !tunnel_was_active {
            let _ = stop_tunnel(&state.interface);
        }
        return match restore_relay(previous_endpoint) {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow!(
                "server update failed: {error:#}; relay rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

fn prepare_relay(server: &ServerProfile, force_restart: bool) -> Result<()> {
    validate_server(server)?;
    let endpoint = format!("{}\n", server.endpoint);
    let endpoint_changed =
        !fs::read(RELAY_ENDPOINT_PATH).is_ok_and(|current| current == endpoint.as_bytes());
    let relay_active = Command::new(SERVICE_COMMAND)
        .args(["status", RELAY_SERVICE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to query relay service")?
        .success();
    if !endpoint_changed && !force_restart && relay_active {
        return Ok(());
    }
    if endpoint_changed {
        fs::write(RELAY_ENDPOINT_PATH, endpoint).context("failed to update relay endpoint")?;
    }
    run(Command::new(SERVICE_COMMAND).args(["restart", RELAY_SERVICE]))?;
    Ok(())
}

fn restore_relay(previous_endpoint: Option<Vec<u8>>) -> Result<()> {
    if let Some(endpoint) = previous_endpoint {
        fs::write(RELAY_ENDPOINT_PATH, endpoint).context("failed to restore relay endpoint")?;
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

fn set_peer(interface: &str, server: &ServerProfile) -> Result<()> {
    validate_server(server)?;
    let config = format!("/etc/wireguard/{interface}.conf");
    let peers = run(Command::new("wg").args(["show", interface, "peers"]))?;
    let previous_addresses = tunnel_addresses(interface)?;
    let next_addresses = vec![server.client_tunnel_address.clone()];
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

pub(crate) fn apply_mode(state: &AppState, mode: &str) -> Result<()> {
    run(Command::new(&state.mode_command).arg(mode))?;
    Ok(())
}

pub(crate) fn access_points() -> Result<Vec<ApNetwork>> {
    parse_access_points(&run(Command::new(WIFI_COMMAND).arg("list"))?)
}

fn parse_access_points(output: &str) -> Result<Vec<ApNetwork>> {
    let mut networks = Vec::new();
    for line in output.lines() {
        let (band, ssid) = line
            .split_once('\t')
            .context("invalid Wi-Fi helper output")?;
        let band = match band {
            "2g" => WifiBand::TwoGhz,
            "5g" => WifiBand::FiveGhz,
            _ => bail!("unsupported Wi-Fi band: {band}"),
        };
        if ssid.is_empty() {
            bail!("Wi-Fi helper returned an empty SSID");
        }
        if networks
            .iter()
            .any(|network: &ApNetwork| network.band == band)
        {
            continue;
        }
        networks.push(ApNetwork {
            band,
            ssid: ssid.to_owned(),
        });
    }
    if networks.is_empty() {
        bail!("Wi-Fi helper returned no access points");
    }
    Ok(networks)
}

pub(crate) fn update_ap(band: Option<WifiBand>, ssid: &str, password: Option<&str>) -> Result<()> {
    let mut command = Command::new(WIFI_COMMAND);
    command.args(["set", band.map_or("all", WifiBand::as_str), ssid]);
    if let Some(password) = password {
        command.arg(password);
    }
    run(&mut command)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_access_point_bands() {
        let networks =
            parse_access_points("2g\tGofroWIFI 2\n2g\tGuest Wi-Fi\n5g\tGofroWIFI 5\n").unwrap();
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].band, WifiBand::TwoGhz);
        assert_eq!(networks[0].ssid, "GofroWIFI 2");
        assert_eq!(networks[1].ssid, "GofroWIFI 5");
        assert!(parse_access_points("6g\tUnsupported\n").is_err());
    }
}

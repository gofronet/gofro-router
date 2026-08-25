use std::{fs, process::Command};

use anyhow::{Context, Result, anyhow, bail};

use crate::{AppState, config::validate_server, model::ServerProfile};

const RELAY_ENDPOINT_PATH: &str = "/etc/gofro/relay-endpoint";
const RELAY_SERVICE: &str = "gofro-relay";
const RELAY_LOCAL_ENDPOINT: &str = "127.0.0.1:51822";
const SERVICE_COMMAND: &str = "/usr/libexec/gofro/service";
const TUNNEL_COMMAND: &str = "/usr/libexec/gofro/tunnel";
const WIFI_COMMAND: &str = "/usr/libexec/gofro/wifi";

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
        prepare_relay(server)?;
        if start_tunnel && !tunnel_was_active {
            set_tunnel(&state.interface, "start")?;
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

fn prepare_relay(server: &ServerProfile) -> Result<()> {
    validate_server(server)?;
    let endpoint = format!("{}\n", server.endpoint);
    if fs::read(RELAY_ENDPOINT_PATH).is_ok_and(|current| current == endpoint.as_bytes()) {
        return Ok(());
    }
    fs::write(RELAY_ENDPOINT_PATH, endpoint).context("failed to update relay endpoint")?;
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
    let result = (|| {
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
        run(Command::new(TUNNEL_COMMAND).args(["save", interface]))?;
        Ok(())
    })();
    if let Err(error) = result {
        return match run(Command::new("wg").args(["setconf", interface, &config])) {
            Ok(_) => Err(error),
            Err(rollback) => Err(anyhow!(
                "WireGuard peer update failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

pub(crate) fn apply_mode(state: &AppState, mode: &str) -> Result<()> {
    run(Command::new(&state.mode_command).arg(mode))?;
    Ok(())
}

pub(crate) fn update_ap(ssid: &str, password: Option<&str>) -> Result<()> {
    let mut command = Command::new(WIFI_COMMAND);
    command.arg(ssid);
    if let Some(password) = password {
        command.arg(password);
    }
    run(&mut command)?;
    Ok(())
}

use std::{fs, process::Command, thread, time::Duration};

use anyhow::{Context, Result};
use tracing::error;
use tunnel_core::run;

use crate::{AppState, config::validate_server, model::ServerProfile};

const RELAY_ENDPOINT_PATH: &str = "/etc/maxos-game-tunnel/relay-endpoint";
const RELAY_SERVICE: &str = "maxos-wg-relay-client.service";
const RELAY_LOCAL_ENDPOINT: &str = "127.0.0.1:51822";
const AP_CONNECTION: &str = "maxos-game-ap";

pub(crate) fn start_and_select(state: &AppState, server: &ServerProfile) -> Result<()> {
    prepare_relay(server)?;
    set_tunnel(&state.interface, "start")?;
    set_peer(&state.interface, server)
}

pub(crate) fn stop_tunnel(interface: &str) -> Result<()> {
    set_tunnel(interface, "stop")
}

fn set_tunnel(interface: &str, action: &str) -> Result<()> {
    let unit = format!("wg-quick@{interface}.service");
    run(Command::new("systemctl").args([action, &unit]))?;
    Ok(())
}

pub(crate) fn service_active(interface: &str) -> Result<bool> {
    let unit = format!("wg-quick@{interface}.service");
    Ok(Command::new("systemctl")
        .args(["is-active", "--quiet", &unit])
        .status()
        .context("failed to query WireGuard service")?
        .success())
}

pub(crate) fn select_server_peer(state: &AppState, server: &ServerProfile) -> Result<()> {
    prepare_relay(server)?;
    set_peer(&state.interface, server)
}

fn prepare_relay(server: &ServerProfile) -> Result<()> {
    validate_server(server)?;
    fs::write(RELAY_ENDPOINT_PATH, format!("{}\n", server.endpoint))
        .context("failed to update relay endpoint")?;
    run(Command::new("systemctl").args(["restart", RELAY_SERVICE]))?;
    Ok(())
}

fn set_peer(interface: &str, server: &ServerProfile) -> Result<()> {
    validate_server(server)?;
    let peers = run(Command::new("wg").args(["show", interface, "peers"]))?;
    for peer in peers.split_whitespace() {
        run(Command::new("wg").args(["set", interface, "peer", peer, "remove"]))?;
    }
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
    run(Command::new("wg-quick").args(["save", interface]))?;
    Ok(())
}

pub(crate) fn apply_mode(state: &AppState, mode: &str) -> Result<()> {
    run(Command::new(&state.mode_command).arg(mode))?;
    Ok(())
}

pub(crate) fn update_ap(ssid: &str, password: Option<&str>) -> Result<()> {
    let mut command = Command::new("nmcli");
    command.args([
        "connection",
        "modify",
        AP_CONNECTION,
        "802-11-wireless.ssid",
        ssid,
    ]);
    if let Some(password) = password {
        command.args(["wifi-sec.psk", password]);
    }
    run(&mut command)?;
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(2));
        if let Err(error) = run(Command::new("nmcli").args(["connection", "down", AP_CONNECTION])) {
            error!(%error, "failed to stop Wi-Fi access point");
        }
        if let Err(error) = run(Command::new("nmcli").args(["connection", "up", AP_CONNECTION])) {
            error!(%error, "failed to start Wi-Fi access point");
        }
    });
    Ok(())
}

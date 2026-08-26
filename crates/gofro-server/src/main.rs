#![forbid(unsafe_code)]

use std::{
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use wireguard_status::wireguard_peers;

const CLIENT_TUNNEL_ADDRESS: &str = "10.202.0.2/32";
const CLIENT_SUBNET: &str = "10.203.1.0/24";

#[derive(Debug, Parser)]
#[command(about = "Manage WireGuard peers on a Gofro server", version)]
struct Args {
    #[arg(long, default_value = "gt0", global = true)]
    interface: String,

    #[command(subcommand)]
    command: ServerCommand,
}

#[derive(Debug, Subcommand)]
enum ServerCommand {
    AddPeer {
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        tunnel_ip: String,
        #[arg(long)]
        subnet: String,
    },
    CreateProfile {
        #[arg(long)]
        endpoint: String,
    },
    RemovePeer {
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        subnet: String,
    },
    Status,
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

fn run_with_input(command: &mut Command, input: &str) -> Result<String> {
    let description = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {description}"))?;
    let mut stdin = child.stdin.take().context("failed to open command stdin")?;
    stdin
        .write_all(input.as_bytes())
        .context("failed to write command input")?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {description}"))?;
    if !output.status.success() {
        bail!(
            "{description} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("command returned non-UTF-8 output")
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        ServerCommand::AddPeer {
            public_key,
            tunnel_ip,
            subnet,
        } => {
            require_root()?;
            add_peer(&args.interface, &public_key, &tunnel_ip, &subnet)?;
            println!("peer added: {subnet} via {}", args.interface);
        }
        ServerCommand::CreateProfile { endpoint } => {
            require_root()?;
            validate_endpoint(&endpoint)?;
            let private_key = run(Command::new("wg").arg("genkey"))?;
            let public_key = run_with_input(Command::new("wg").arg("pubkey"), &private_key)?;
            let server_public_key =
                run(Command::new("wg").args(["show", &args.interface, "public-key"]))?;
            add_peer(
                &args.interface,
                public_key.trim(),
                CLIENT_TUNNEL_ADDRESS,
                CLIENT_SUBNET,
            )?;
            print!(
                "{}",
                format_profile(
                    private_key.trim(),
                    public_key.trim(),
                    server_public_key.trim(),
                    &endpoint,
                )
            );
        }
        ServerCommand::RemovePeer { public_key, subnet } => {
            require_root()?;
            run(Command::new("wg").args(["set", &args.interface, "peer", &public_key, "remove"]))?;
            let _ = Command::new("ip")
                .args(["route", "del", &subnet, "dev", &args.interface])
                .status();
            save(&args.interface)?;
            println!("peer removed: {public_key}");
        }
        ServerCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&wireguard_peers(&args.interface)?)?
            );
        }
    }

    Ok(())
}

fn add_peer(interface: &str, public_key: &str, tunnel_ip: &str, subnet: &str) -> Result<()> {
    let previous_config = run(Command::new("wg").args(["showconf", interface]))?;
    let route_existed = !run(Command::new("ip").args(["route", "show", subnet, "dev", interface]))?
        .trim()
        .is_empty();
    let allowed_ips = format!("{tunnel_ip},{subnet}");
    let result = (|| {
        run(Command::new("wg").args([
            "set",
            interface,
            "peer",
            public_key,
            "allowed-ips",
            &allowed_ips,
        ]))?;
        run(Command::new("ip").args(["route", "replace", subnet, "dev", interface]))?;
        save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            if route_existed {
                run(Command::new("ip").args(["route", "replace", subnet, "dev", interface]))?;
            } else {
                let _ = Command::new("ip")
                    .args(["route", "del", subnet, "dev", interface])
                    .status();
            }
            save(interface)
        })();
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback) => Err(anyhow::anyhow!(
                "peer setup failed: {error:#}; rollback failed: {rollback:#}"
            )),
        };
    }
    Ok(())
}

fn format_profile(
    private_key: &str,
    client_public_key: &str,
    server_public_key: &str,
    endpoint: &str,
) -> String {
    format!(
        "# ClientPublicKey = {client_public_key}\n[Interface]\nPrivateKey = {private_key}\nAddress = {CLIENT_TUNNEL_ADDRESS}\nMTU = 1360\n\n[Peer]\nPublicKey = {server_public_key}\nAllowedIPs = 0.0.0.0/0\nEndpoint = {endpoint}\nPersistentKeepalive = 10\n"
    )
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .context("endpoint must have host:port format")?;
    if endpoint.len() > 255
        || host.is_empty()
        || endpoint.chars().any(char::is_whitespace)
        || port.parse::<u16>().ok().filter(|port| *port > 0).is_none()
    {
        bail!("endpoint must have host:port format");
    }
    Ok(())
}

fn save(interface: &str) -> Result<()> {
    run(Command::new("wg-quick").args(["save", interface]))?;
    Ok(())
}

fn require_root() -> Result<()> {
    let uid = run(Command::new("id").arg("-u"))?;
    if uid.trim() != "0" {
        bail!("this command must run as root");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_importable_profile() {
        let profile = format_profile("private", "client", "server", "vpn.test:8443");
        assert!(profile.starts_with("# ClientPublicKey = client"));
        assert!(profile.contains("[Interface]\nPrivateKey = private"));
        assert!(profile.contains("Address = 10.202.0.2/32"));
        assert!(profile.contains("[Peer]\nPublicKey = server"));
        assert!(profile.contains("MTU = 1360"));
        assert!(profile.contains("AllowedIPs = 0.0.0.0/0"));
        assert!(profile.contains("PersistentKeepalive = 10"));
        assert!(profile.contains("Endpoint = vpn.test:8443"));
        assert!(validate_endpoint("vpn.test:8443").is_ok());
        assert!(validate_endpoint("vpn.test").is_err());
        assert!(validate_endpoint(&format!("{}:8443", "a".repeat(251))).is_err());
    }
}

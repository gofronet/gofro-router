#![forbid(unsafe_code)]

use std::{
    io::Write,
    net::Ipv4Addr,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ipnet::Ipv4Net;
use wireguard_status::wireguard_peers;

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
// Deprecated compatibility only; remove all `subnet` fields in the next breaking release.
enum ServerCommand {
    AddPeer {
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        tunnel_ip: String,
        #[arg(long, hide = true)]
        subnet: Option<String>,
    },
    CreateProfile {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        tunnel_ip: String,
        #[arg(long, hide = true)]
        subnet: Option<String>,
    },
    RemovePeer {
        #[arg(long)]
        public_key: String,
        #[arg(long, hide = true)]
        subnet: Option<String>,
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
            add_peer(&args.interface, &public_key, &tunnel_ip, subnet.as_deref())?;
            println!("peer added: {tunnel_ip} via {}", args.interface);
        }
        ServerCommand::CreateProfile {
            endpoint,
            tunnel_ip,
            subnet,
        } => {
            require_root()?;
            validate_endpoint(&endpoint)?;
            let private_key = run(Command::new("wg").arg("genkey"))?;
            let public_key = run_with_input(Command::new("wg").arg("pubkey"), &private_key)?;
            let server_public_key =
                run(Command::new("wg").args(["show", &args.interface, "public-key"]))?;
            add_peer(
                &args.interface,
                public_key.trim(),
                &tunnel_ip,
                subnet.as_deref(),
            )?;
            print!(
                "{}",
                format_profile(
                    private_key.trim(),
                    public_key.trim(),
                    server_public_key.trim(),
                    &endpoint,
                    &tunnel_ip,
                )
            );
        }
        ServerCommand::RemovePeer { public_key, subnet } => {
            require_root()?;
            let subnet = subnet
                .as_deref()
                .map(validate_subnet)
                .transpose()?
                .map(|subnet| subnet.to_string());
            run(Command::new("wg").args(["set", &args.interface, "peer", &public_key, "remove"]))?;
            if let Some(subnet) = subnet {
                let _ = Command::new("ip")
                    .args(["route", "del", &subnet, "dev", &args.interface])
                    .status();
            }
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

fn add_peer(
    interface: &str,
    public_key: &str,
    tunnel_ip: &str,
    subnet: Option<&str>,
) -> Result<()> {
    let tunnel_ip = validate_tunnel_ip(tunnel_ip)?;
    let subnet = subnet.map(validate_subnet).transpose()?;
    let routes = subnet.map_or_else(|| vec![tunnel_ip], |subnet| vec![tunnel_ip, subnet]);
    // ponytail: This root-only admin CLI assumes one operator; add a file lock if automated.
    ensure_routes_available(interface, public_key, &routes)?;
    let tunnel_ip = tunnel_ip.to_string();
    let subnet = subnet.map(|subnet| subnet.to_string());
    let previous_config = run(Command::new("wg").args(["showconf", interface]))?;
    let route_existed = match subnet.as_deref() {
        Some(subnet) => !run(Command::new("ip").args(["route", "show", subnet, "dev", interface]))?
            .trim()
            .is_empty(),
        None => false,
    };
    let allowed_ips = allowed_ips(&tunnel_ip, subnet.as_deref());
    let result = (|| {
        run(Command::new("wg").args([
            "set",
            interface,
            "peer",
            public_key,
            "allowed-ips",
            &allowed_ips,
        ]))?;
        if let Some(subnet) = subnet.as_deref() {
            run(Command::new("ip").args(["route", "replace", subnet, "dev", interface]))?;
        }
        save(interface)
    })();
    if let Err(error) = result {
        let rollback = (|| {
            run_with_input(
                Command::new("wg").args(["setconf", interface, "/dev/stdin"]),
                &previous_config,
            )?;
            if let Some(subnet) = subnet.as_deref() {
                if route_existed {
                    run(Command::new("ip").args(["route", "replace", subnet, "dev", interface]))?;
                } else {
                    let _ = Command::new("ip")
                        .args(["route", "del", subnet, "dev", interface])
                        .status();
                }
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

fn validate_tunnel_ip(value: &str) -> Result<Ipv4Net> {
    let network = value
        .parse::<Ipv4Net>()
        .context("tunnel IP must be an IPv4 network")?;
    let octets = network.addr().octets();
    if network.prefix_len() != 32 || octets[..3] != [10, 202, 0] || !(2..=254).contains(&octets[3])
    {
        bail!("tunnel IP must be in 10.202.0.2-10.202.0.254/32");
    }
    Ok(network)
}

fn validate_subnet(value: &str) -> Result<Ipv4Net> {
    let network = value
        .parse::<Ipv4Net>()
        .context("subnet must be an IPv4 network")?;
    if network.prefix_len() != 24 || network.addr() != Ipv4Addr::new(10, 203, 1, 0) {
        bail!("subnet must be {CLIENT_SUBNET}");
    }
    Ok(network)
}

fn ensure_routes_available(interface: &str, public_key: &str, routes: &[Ipv4Net]) -> Result<()> {
    let assigned = run(Command::new("wg").args(["show", interface, "allowed-ips"]))?;
    if routes_conflict(&assigned, public_key, routes) {
        bail!("tunnel IP or subnet is already assigned to another peer");
    }
    Ok(())
}

fn routes_conflict(assigned: &str, public_key: &str, routes: &[Ipv4Net]) -> bool {
    for line in assigned.lines() {
        let mut fields = line.split_whitespace();
        if fields.next().is_some_and(|key| key != public_key)
            && fields
                .filter_map(|value| value.parse::<Ipv4Net>().ok())
                .any(|existing| {
                    routes.iter().any(|route| {
                        existing.contains(&route.network()) || route.contains(&existing.network())
                    })
                })
        {
            return true;
        }
    }
    false
}

fn allowed_ips(tunnel_ip: &str, subnet: Option<&str>) -> String {
    subnet.map_or_else(
        || tunnel_ip.to_owned(),
        |subnet| format!("{tunnel_ip},{subnet}"),
    )
}

fn format_profile(
    private_key: &str,
    client_public_key: &str,
    server_public_key: &str,
    endpoint: &str,
    tunnel_ip: &str,
) -> String {
    format!(
        "# ClientPublicKey = {client_public_key}\n[Interface]\nPrivateKey = {private_key}\nAddress = {tunnel_ip}\nMTU = 1280\n\n[Peer]\nPublicKey = {server_public_key}\nAllowedIPs = 0.0.0.0/0\nEndpoint = {endpoint}\nPersistentKeepalive = 10\n"
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
        let profile = format_profile(
            "private",
            "client",
            "server",
            "vpn.test:8443",
            "10.202.0.5/32",
        );
        assert!(profile.starts_with("# ClientPublicKey = client"));
        assert!(profile.contains("[Interface]\nPrivateKey = private"));
        assert!(profile.contains("Address = 10.202.0.5/32"));
        assert!(profile.contains("[Peer]\nPublicKey = server"));
        assert!(profile.contains("MTU = 1280"));
        assert!(profile.contains("AllowedIPs = 0.0.0.0/0"));
        assert!(profile.contains("PersistentKeepalive = 10"));
        assert!(profile.contains("Endpoint = vpn.test:8443"));
        assert!(validate_endpoint("vpn.test:8443").is_ok());
        assert!(validate_endpoint("vpn.test").is_err());
        assert!(validate_endpoint(&format!("{}:8443", "a".repeat(251))).is_err());
        assert_eq!(allowed_ips("10.202.0.5/32", None), "10.202.0.5/32");
        assert_eq!(
            allowed_ips("10.202.0.2/32", Some("10.203.1.0/24")),
            "10.202.0.2/32,10.203.1.0/24"
        );
        assert!(validate_tunnel_ip("10.202.0.254/32").is_ok());
        assert!(validate_tunnel_ip("10.202.0.1/32").is_err());
        assert!(validate_subnet(CLIENT_SUBNET).is_ok());
        assert!(validate_subnet("0.0.0.0/0").is_err());
        assert!(
            Args::try_parse_from([
                "gofro-router-server",
                "create-profile",
                "--endpoint",
                "vpn.test:8443",
                "--tunnel-ip",
                "10.202.0.5/32",
                "--subnet",
                CLIENT_SUBNET,
            ])
            .is_ok()
        );
        let routes = [validate_tunnel_ip("10.202.0.5/32").unwrap()];
        assert!(routes_conflict(
            "other-key\t10.202.0.5/32\n",
            "new-key",
            &routes
        ));
        assert!(!routes_conflict(
            "same-key\t10.202.0.5/32\n",
            "same-key",
            &routes
        ));
    }
}

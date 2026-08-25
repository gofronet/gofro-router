use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use wireguard_status::wireguard_peers;

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

fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        ServerCommand::AddPeer {
            public_key,
            tunnel_ip,
            subnet,
        } => {
            require_root()?;
            let allowed_ips = format!("{tunnel_ip},{subnet}");
            run(Command::new("wg").args([
                "set",
                &args.interface,
                "peer",
                &public_key,
                "allowed-ips",
                &allowed_ips,
            ]))?;
            run(Command::new("ip").args(["route", "replace", &subnet, "dev", &args.interface]))?;
            save(&args.interface)?;
            println!("peer added: {subnet} via {}", args.interface);
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

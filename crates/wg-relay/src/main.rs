mod client;
mod codec;
mod server;

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(about = "Obfuscated UDP transport for WireGuard datagrams", version)]
struct Args {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Debug, Subcommand)]
enum Mode {
    Client {
        #[arg(long, default_value = "127.0.0.1:51822")]
        listen: SocketAddr,

        #[arg(long, default_value = "/etc/maxos-game-tunnel/relay-endpoint")]
        server_file: PathBuf,
    },
    Server {
        #[arg(long, default_value = "0.0.0.0:8443")]
        listen: SocketAddr,

        #[arg(long, default_value = "127.0.0.1:51820")]
        wireguard: SocketAddr,
    },
}

fn main() -> Result<()> {
    match Args::parse().mode {
        Mode::Client {
            listen,
            server_file,
        } => client::run(listen, server_file),
        Mode::Server { listen, wireguard } => server::run(listen, wireguard),
    }
}

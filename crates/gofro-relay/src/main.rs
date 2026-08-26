#![forbid(unsafe_code)]

mod batch;
mod client;
mod codec;
mod server;

use std::{
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
};

use anyhow::Result;
use clap::{Parser, Subcommand};
use socket2::SockRef;

const SOCKET_BUFFER_SIZE: usize = 1024 * 1024;

fn configure_socket(socket: &UdpSocket) -> std::io::Result<()> {
    let socket = SockRef::from(socket);
    socket.set_recv_buffer_size(SOCKET_BUFFER_SIZE)?;
    socket.set_send_buffer_size(SOCKET_BUFFER_SIZE)
}

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

        #[arg(long, default_value = "/etc/gofro/relay-endpoint")]
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

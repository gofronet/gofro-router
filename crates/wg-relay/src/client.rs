use std::{fs, net::SocketAddr, net::UdpSocket, path::PathBuf, thread};

use anyhow::{Context, Result, bail};

use crate::codec::{BUFFER_SIZE, decode, send_encoded};

pub(crate) fn run(listen: SocketAddr, server_file: PathBuf) -> Result<()> {
    let endpoint = fs::read_to_string(&server_file)
        .with_context(|| format!("failed to read {}", server_file.display()))?;
    let local = UdpSocket::bind(listen).with_context(|| format!("failed to bind {listen}"))?;
    let remote = UdpSocket::bind("0.0.0.0:0").context("failed to bind relay client socket")?;
    remote
        .connect(endpoint.trim())
        .with_context(|| format!("failed to connect to {}", endpoint.trim()))?;

    let mut plain = [0_u8; BUFFER_SIZE];
    let (size, wireguard) = local
        .recv_from(&mut plain)
        .context("failed to receive initial WireGuard datagram")?;
    local
        .connect(wireguard)
        .context("failed to connect local WireGuard socket")?;
    send_encoded(&remote, &plain[..size])?;
    eprintln!("relay client: {wireguard} -> {}", endpoint.trim());

    let send_local = local.try_clone()?;
    let send_remote = remote.try_clone()?;
    thread::spawn(move || {
        if let Err(error) = forward_encoded(&send_local, &send_remote) {
            eprintln!("relay forwarding failed: {error:#}");
            std::process::exit(1);
        }
    });
    forward_decoded(&remote, &local)
}

fn forward_encoded(input: &UdpSocket, output: &UdpSocket) -> Result<()> {
    let mut plain = [0_u8; BUFFER_SIZE];
    loop {
        let size = input.recv(&mut plain).context("UDP receive failed")?;
        send_encoded(output, &plain[..size])?;
    }
}

fn forward_decoded(input: &UdpSocket, output: &UdpSocket) -> Result<()> {
    let mut encoded = [0_u8; BUFFER_SIZE];
    let mut plain = [0_u8; BUFFER_SIZE];
    loop {
        let size = input.recv(&mut encoded).context("UDP receive failed")?;
        let Some(decoded) = decode(&encoded[..size], &mut plain) else {
            continue;
        };
        if output.send(decoded).context("UDP send failed")? != decoded.len() {
            bail!("partial UDP send");
        }
    }
}

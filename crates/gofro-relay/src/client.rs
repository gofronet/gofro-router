use std::{fs, net::SocketAddr, net::UdpSocket, path::PathBuf, thread};

use anyhow::{Context, Result};

use crate::{
    batch::{BATCH_SIZE, PacketBatch, PacketLengths, recv_many, send_many},
    codec::{BUFFER_SIZE, decode, encode, send_encoded},
    configure_socket,
};

pub(crate) fn run(listen: SocketAddr, server_file: PathBuf) -> Result<()> {
    let endpoint = fs::read_to_string(&server_file)
        .with_context(|| format!("failed to read {}", server_file.display()))?;

    let local = UdpSocket::bind(listen).with_context(|| format!("failed to bind {listen}"))?;
    let remote = UdpSocket::bind("0.0.0.0:0").context("failed to bind relay client socket")?;
    configure_socket(&local).context("failed to configure local relay socket")?;
    configure_socket(&remote).context("failed to configure remote relay socket")?;

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
    println!("relay client: {wireguard} -> {}", endpoint.trim());

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
    let mut plain: PacketBatch = [[0; BUFFER_SIZE]; BATCH_SIZE];
    let mut plain_lengths: PacketLengths = [0; BATCH_SIZE];
    let mut encoded: PacketBatch = [[0; BUFFER_SIZE]; BATCH_SIZE];
    let mut encoded_lengths: PacketLengths = [0; BATCH_SIZE];
    loop {
        let count =
            recv_many(input, &mut plain, &mut plain_lengths).context("UDP batch receive failed")?;
        for index in 0..count {
            encoded_lengths[index] =
                encode(&plain[index][..plain_lengths[index]], &mut encoded[index])?.len();
        }
        send_many(output, &encoded, &encoded_lengths, count).context("UDP batch send failed")?;
    }
}

fn forward_decoded(input: &UdpSocket, output: &UdpSocket) -> Result<()> {
    let mut encoded: PacketBatch = [[0; BUFFER_SIZE]; BATCH_SIZE];
    let mut encoded_lengths: PacketLengths = [0; BATCH_SIZE];
    let mut plain: PacketBatch = [[0; BUFFER_SIZE]; BATCH_SIZE];
    let mut plain_lengths: PacketLengths = [0; BATCH_SIZE];
    loop {
        let count = recv_many(input, &mut encoded, &mut encoded_lengths)
            .context("UDP batch receive failed")?;
        let mut decoded_count = 0;
        for index in 0..count {
            if let Some(decoded) = decode(
                &encoded[index][..encoded_lengths[index]],
                &mut plain[decoded_count],
            ) {
                plain_lengths[decoded_count] = decoded.len();
                decoded_count += 1;
            }
        }
        send_many(output, &plain, &plain_lengths, decoded_count)
            .context("UDP batch send failed")?;
    }
}

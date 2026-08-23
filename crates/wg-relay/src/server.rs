use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::codec::{BUFFER_SIZE, decode, encode};

const MAX_CLIENTS: usize = 256;
const CLIENT_IDLE: Duration = Duration::from_secs(180);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10);

struct Session {
    socket: UdpSocket,
    last_seen: Arc<AtomicU64>,
}

pub(crate) fn run(listen: SocketAddr, wireguard: SocketAddr) -> Result<()> {
    let public = UdpSocket::bind(listen).with_context(|| format!("failed to bind {listen}"))?;
    serve(public, wireguard, MAX_CLIENTS, CLIENT_IDLE)
}

fn serve(
    public: UdpSocket,
    wireguard: SocketAddr,
    max_clients: usize,
    idle: Duration,
) -> Result<()> {
    public
        .set_read_timeout(Some(RECEIVE_TIMEOUT))
        .context("failed to configure relay socket")?;
    let mut sessions = HashMap::<SocketAddr, Session>::new();
    let mut encoded = [0_u8; BUFFER_SIZE];
    let mut plain = [0_u8; BUFFER_SIZE];

    loop {
        let (size, source) = match public.recv_from(&mut encoded) {
            Ok(packet) => packet,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                remove_idle(&mut sessions, idle);
                continue;
            }
            Err(error) => return Err(error).context("UDP receive failed"),
        };
        let Some(decoded) = decode(&encoded[..size], &mut plain) else {
            continue;
        };

        remove_idle(&mut sessions, idle);
        if !sessions.contains_key(&source) {
            if sessions.len() >= max_clients {
                eprintln!("relay client limit reached; dropping {source}");
                continue;
            }
            sessions.insert(source, Session::new(&public, wireguard, source, idle)?);
            eprintln!("relay server: {source} -> {wireguard}");
        }
        let session = &sessions[&source];
        session.last_seen.store(unix_time(), Ordering::Relaxed);
        if session.socket.send(decoded).context("UDP send failed")? != decoded.len() {
            bail!("partial UDP send");
        }
    }
}

impl Session {
    fn new(
        public: &UdpSocket,
        wireguard: SocketAddr,
        source: SocketAddr,
        idle: Duration,
    ) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").context("failed to bind relay session")?;
        socket
            .connect(wireguard)
            .with_context(|| format!("failed to connect to WireGuard at {wireguard}"))?;
        socket
            .set_read_timeout(Some(RECEIVE_TIMEOUT))
            .context("failed to configure relay session")?;
        let last_seen = Arc::new(AtomicU64::new(unix_time()));
        let worker_socket = socket.try_clone()?;
        let worker_public = public.try_clone()?;
        let worker_seen = Arc::clone(&last_seen);
        thread::Builder::new()
            .name(format!("relay-{source}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                if let Err(error) =
                    return_packets(worker_socket, worker_public, source, worker_seen, idle)
                {
                    eprintln!("relay session {source} failed: {error:#}");
                }
            })
            .context("failed to start relay session")?;
        Ok(Self { socket, last_seen })
    }
}

fn return_packets(
    local: UdpSocket,
    public: UdpSocket,
    destination: SocketAddr,
    last_seen: Arc<AtomicU64>,
    idle: Duration,
) -> Result<()> {
    let mut plain = [0_u8; BUFFER_SIZE];
    let mut encoded = [0_u8; BUFFER_SIZE];
    loop {
        let size = match local.recv(&mut plain) {
            Ok(size) => size,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if is_idle(last_seen.load(Ordering::Relaxed), idle) {
                    return Ok(());
                }
                continue;
            }
            Err(error) => return Err(error).context("UDP receive failed"),
        };
        let packet = encode(&plain[..size], &mut encoded)?;
        if public
            .send_to(packet, destination)
            .context("UDP send failed")?
            != packet.len()
        {
            bail!("partial UDP send");
        }
    }
}

fn remove_idle(sessions: &mut HashMap<SocketAddr, Session>, idle: Duration) {
    sessions.retain(|source, session| {
        let active = !is_idle(session.last_seen.load(Ordering::Relaxed), idle);
        if !active {
            eprintln!("relay session expired: {source}");
        }
        active
    });
}

fn is_idle(last_seen: u64, idle: Duration) -> bool {
    unix_time().saturating_sub(last_seen) >= idle.as_secs()
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_two_clients_independently() {
        let wireguard = UdpSocket::bind("127.0.0.1:0").unwrap();
        wireguard
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let public = UdpSocket::bind("127.0.0.1:0").unwrap();
        let relay = public.local_addr().unwrap();
        let wireguard_addr = wireguard.local_addr().unwrap();
        thread::spawn(move || serve(public, wireguard_addr, 2, Duration::from_secs(30)).unwrap());

        let first = test_client(relay);
        let second = test_client(relay);
        send_test_packet(&first, b"first");
        send_test_packet(&second, b"second");

        let mut routes = HashMap::new();
        let mut buffer = [0_u8; 128];
        for _ in 0..2 {
            let (size, source) = wireguard.recv_from(&mut buffer).unwrap();
            routes.insert(buffer[..size].to_vec(), source);
        }
        wireguard
            .send_to(b"reply-first", routes[b"first".as_slice()])
            .unwrap();
        wireguard
            .send_to(b"reply-second", routes[b"second".as_slice()])
            .unwrap();

        assert_eq!(receive_test_packet(&first), b"reply-first");
        assert_eq!(receive_test_packet(&second), b"reply-second");
    }

    fn test_client(relay: SocketAddr) -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(relay).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
    }

    fn send_test_packet(socket: &UdpSocket, plain: &[u8]) {
        let mut encoded = [0_u8; 128];
        socket.send(encode(plain, &mut encoded).unwrap()).unwrap();
    }

    fn receive_test_packet(socket: &UdpSocket) -> Vec<u8> {
        let mut encoded = [0_u8; 128];
        let size = socket.recv(&mut encoded).unwrap();
        let mut plain = [0_u8; 128];
        decode(&encoded[..size], &mut plain).unwrap().to_vec()
    }
}

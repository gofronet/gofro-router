use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::{
    batch::{BATCH_SIZE, PacketBatch, PacketLengths, PacketSources, recv_from_many},
    codec::{BUFFER_SIZE, decode, encode},
    configure_socket,
};

const MAX_CLIENTS: usize = 256;
const CLIENT_IDLE: Duration = Duration::from_secs(180);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10);

struct Session {
    socket: UdpSocket,
    last_seen: Arc<AtomicU64>,
    worker_alive: Arc<AtomicBool>,
}

pub(crate) fn run(listen: SocketAddr, wireguard: SocketAddr) -> Result<()> {
    let public = UdpSocket::bind(listen).with_context(|| format!("failed to bind {listen}"))?;
    configure_socket(&public).context("failed to configure public relay socket")?;
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
    let mut encoded: PacketBatch = [[0; BUFFER_SIZE]; BATCH_SIZE];
    let mut encoded_lengths: PacketLengths = [0; BATCH_SIZE];
    let mut sources: PacketSources = [None; BATCH_SIZE];
    let mut plain = [0_u8; BUFFER_SIZE];
    let mut last_cleanup = Instant::now();

    loop {
        let count = match recv_from_many(&public, &mut encoded, &mut encoded_lengths, &mut sources)
        {
            Ok(count) => count,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                remove_idle(&mut sessions, idle);
                last_cleanup = Instant::now();
                continue;
            }
            Err(error) => return Err(error).context("UDP receive failed"),
        };
        if last_cleanup.elapsed() >= RECEIVE_TIMEOUT {
            remove_idle(&mut sessions, idle);
            last_cleanup = Instant::now();
        }
        for index in 0..count {
            let Some(decoded) = decode(&encoded[index][..encoded_lengths[index]], &mut plain)
            else {
                continue;
            };
            let source = sources[index].context("UDP source address missing")?;
            let Some(session) =
                get_or_create_session(&mut sessions, &public, wireguard, source, max_clients)?
            else {
                continue;
            };
            session.last_seen.store(unix_time(), Ordering::Relaxed);
            if session.socket.send(decoded).context("UDP send failed")? != decoded.len() {
                bail!("partial UDP send");
            }
        }
    }
}

impl Session {
    fn new(public: &UdpSocket, wireguard: SocketAddr, source: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").context("failed to bind relay session")?;
        configure_socket(&socket).context("failed to configure relay session socket")?;
        socket
            .connect(wireguard)
            .with_context(|| format!("failed to connect to WireGuard at {wireguard}"))?;
        socket
            .set_read_timeout(Some(RECEIVE_TIMEOUT))
            .context("failed to configure relay session")?;
        let last_seen = Arc::new(AtomicU64::new(unix_time()));
        let worker_alive = Arc::new(AtomicBool::new(true));
        let worker_socket = socket.try_clone()?;
        let worker_public = public.try_clone()?;
        let worker_state = Arc::clone(&worker_alive);
        thread::Builder::new()
            .name(format!("relay-{source}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                let result = return_packets(
                    worker_socket,
                    worker_public,
                    source,
                    Arc::clone(&worker_state),
                );
                worker_state.store(false, Ordering::Release);
                if let Err(error) = result {
                    eprintln!("relay session {source} failed: {error:#}");
                }
            })
            .context("failed to start relay session")?;
        Ok(Self {
            socket,
            last_seen,
            worker_alive,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.worker_alive.store(false, Ordering::Release);
    }
}

fn return_packets(
    local: UdpSocket,
    public: UdpSocket,
    destination: SocketAddr,
    worker_alive: Arc<AtomicBool>,
) -> Result<()> {
    let mut plain = [0_u8; BUFFER_SIZE];
    let mut encoded = [0_u8; BUFFER_SIZE];
    loop {
        let size = match local.recv(&mut plain) {
            Ok(size) => size,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if !worker_alive.load(Ordering::Acquire) {
                    return Ok(());
                }
                continue;
            }
            Err(error) => return Err(error).context("UDP receive failed"),
        };
        if !worker_alive.load(Ordering::Acquire) {
            return Ok(());
        }
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

fn get_or_create_session<'a>(
    sessions: &'a mut HashMap<SocketAddr, Session>,
    public: &UdpSocket,
    wireguard: SocketAddr,
    source: SocketAddr,
    max_clients: usize,
) -> Result<Option<&'a Session>> {
    if sessions
        .get(&source)
        .is_some_and(|session| !session.worker_alive.load(Ordering::Acquire))
    {
        sessions.remove(&source);
        eprintln!("relay session worker restarted: {source}");
    }
    if !sessions.contains_key(&source) {
        if sessions.len() >= max_clients {
            eprintln!("relay client limit reached; dropping {source}");
            return Ok(None);
        }
        sessions.insert(source, Session::new(public, wireguard, source)?);
        eprintln!("relay server: {source} -> {wireguard}");
    }
    Ok(sessions.get(&source))
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

    #[test]
    fn replaces_failed_session_worker() {
        let public = UdpSocket::bind("127.0.0.1:0").unwrap();
        let wireguard = UdpSocket::bind("127.0.0.1:0").unwrap();
        let source = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let failed = Session::new(&public, wireguard.local_addr().unwrap(), source).unwrap();
        let failed_port = failed.socket.local_addr().unwrap();
        failed.worker_alive.store(false, Ordering::Release);
        let mut sessions = HashMap::from([(source, failed)]);

        let recovered = get_or_create_session(
            &mut sessions,
            &public,
            wireguard.local_addr().unwrap(),
            source,
            1,
        )
        .unwrap()
        .unwrap();

        assert!(recovered.worker_alive.load(Ordering::Acquire));
        assert_ne!(recovered.socket.local_addr().unwrap(), failed_port);
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

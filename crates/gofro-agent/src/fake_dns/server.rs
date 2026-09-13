use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, RwLock, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    sync::Semaphore,
};
use tracing::{error, info, warn};

use super::resolver::{FakeDns, MAX_DNS_PACKET, failure_response};
use crate::{model::LanContext, routing::RoutingPolicy};

const MAX_CONCURRENT_REQUESTS: usize = 16;
const MAX_TCP_CONNECTIONS: usize = 32;
const TCP_READ_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

struct UdpReply {
    peer: SocketAddr,
    #[cfg(target_os = "linux")]
    source: PacketInfo,
}

#[cfg(target_os = "linux")]
enum PacketInfo {
    V4(nix::libc::in_pktinfo),
    V6(nix::libc::in6_pktinfo),
}

pub(crate) struct Server {
    dns: Arc<FakeDns>,
    policy: Arc<RwLock<RoutingPolicy>>,
    upstream: SocketAddr,
    lan: LanContext,
    udp: UdpSocket,
    tcp: TcpListener,
    workers: Arc<Semaphore>,
    tcp_connections: Arc<Semaphore>,
}

impl Server {
    pub(crate) async fn bind(
        listen: SocketAddr,
        upstream: SocketAddr,
        dns: Arc<FakeDns>,
        policy: Arc<RwLock<RoutingPolicy>>,
        lan: &LanContext,
    ) -> Result<Self> {
        let (udp, tcp) = bind_lan_sockets(listen, lan).await?;
        Ok(Self {
            dns,
            policy,
            upstream,
            lan: lan.clone(),
            udp,
            tcp,
            workers: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            tcp_connections: Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS)),
        })
    }

    pub(crate) async fn run(self) {
        self.dns.active.store(true, Ordering::Relaxed);
        let server = Arc::new(self);
        let cleanup = tokio::spawn(clean_expired(Arc::clone(&server.dns)));
        tokio::select! {
            result = serve_udp(Arc::clone(&server)) => {
                if let Err(error) = result { error!(%error, "FakeDNS UDP listener failed"); }
            }
            result = serve_tcp(Arc::clone(&server)) => {
                if let Err(error) = result { error!(%error, "FakeDNS TCP listener failed"); }
            }
        }
        cleanup.abort();
        server.dns.active.store(false, Ordering::Relaxed);
    }

    fn process(&self, packet: Vec<u8>) -> Vec<u8> {
        let _update = match self.dns.updates.read() {
            Ok(update) => update,
            Err(_) => return failure_response(&packet),
        };
        let policy = match self.policy.read() {
            Ok(policy) => policy.clone(),
            Err(_) => return failure_response(&packet),
        };
        self.dns
            .process(&packet, &policy, self.upstream, &self.lan)
            .unwrap_or_else(|error| {
                warn!(%error, "FakeDNS request failed");
                failure_response(&packet)
            })
    }
}

async fn bind_lan_sockets(
    listen: SocketAddr,
    lan: &LanContext,
) -> Result<(UdpSocket, TcpListener)> {
    crate::validate_lan(lan)?;
    anyhow::ensure!(
        listen.ip() == IpAddr::V4(lan.address),
        "FakeDNS must listen on the LAN IPv4 address"
    );
    #[cfg(target_os = "linux")]
    {
        use socket2::{Domain, Protocol, Socket, Type};

        let bind = |kind, protocol| -> Result<Socket> {
            let socket = Socket::new(Domain::IPV6, kind, Some(protocol))
                .context("FakeDNS requires Linux IPv6 dual-stack sockets")?;
            socket
                .set_only_v6(false)
                .context("failed to enable FakeDNS dual-stack")?;
            // Never expose the wildcard listener unless device isolation succeeds first.
            socket
                .bind_device(Some(lan.device.as_bytes()))
                .with_context(|| {
                    format!("failed to restrict FakeDNS to LAN device {}", lan.device)
                })?;
            if kind == Type::STREAM {
                socket.set_reuse_address(true)?;
            }
            socket.set_nonblocking(true)?;
            let address = SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, listen.port()));
            socket.bind(&address.into()).with_context(|| {
                format!(
                    "failed to bind FakeDNS {kind:?} {address} on {}",
                    lan.device
                )
            })?;
            Ok(socket)
        };
        let udp = bind(Type::DGRAM, Protocol::UDP)?;
        nix::sys::socket::setsockopt(&udp, nix::sys::socket::sockopt::Ipv4PacketInfo, &true)
            .context("failed to enable FakeDNS IPv4 packet info")?;
        nix::sys::socket::setsockopt(&udp, nix::sys::socket::sockopt::Ipv6RecvPacketInfo, &true)
            .context("failed to enable FakeDNS IPv6 packet info")?;
        let tcp = bind(Type::STREAM, Protocol::TCP)?;
        tcp.listen(128)
            .context("failed to listen on FakeDNS TCP socket")?;
        Ok((
            UdpSocket::from_std(udp.into())?,
            TcpListener::from_std(tcp.into())?,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok((
            UdpSocket::bind(listen)
                .await
                .with_context(|| format!("failed to bind FakeDNS UDP {listen}"))?,
            TcpListener::bind(listen)
                .await
                .with_context(|| format!("failed to bind FakeDNS TCP {listen}"))?,
        ))
    }
}

async fn clean_expired(dns: Arc<FakeDns>) {
    loop {
        tokio::time::sleep(CLEANUP_INTERVAL).await;
        let dns = Arc::clone(&dns);
        match tokio::task::spawn_blocking(move || dns.purge_expired()).await {
            Ok(Ok(0)) => {}
            Ok(Ok(count)) => info!(count, "expired FakeDNS leases removed"),
            Ok(Err(error)) => warn!(%error, "failed to remove expired FakeDNS leases"),
            Err(error) => warn!(%error, "FakeDNS cleanup worker failed"),
        }
    }
}

async fn serve_udp(server: Arc<Server>) -> Result<()> {
    let mut buffer = vec![0; MAX_DNS_PACKET];
    loop {
        let (length, reply) = recv_datagram(&server.udp, &mut buffer).await?;
        let peer = reply.peer;
        let packet = buffer[..length].to_vec();
        let server = Arc::clone(&server);
        let Ok(permit) = Arc::clone(&server.workers).try_acquire_owned() else {
            if let Err(error) = send_datagram(&server.udp, &failure_response(&packet), &reply).await
            {
                warn!(%error, %peer, "failed to send FakeDNS overload response");
            }
            continue;
        };
        tokio::spawn(async move {
            let worker = Arc::clone(&server);
            match tokio::task::spawn_blocking(move || {
                let _permit = permit;
                worker.process(packet)
            })
            .await
            {
                Ok(response) => {
                    if let Err(error) = send_datagram(&server.udp, &response, &reply).await {
                        warn!(%error, %peer, "failed to send FakeDNS UDP response");
                    }
                }
                Err(error) => warn!(%error, "FakeDNS UDP worker failed"),
            }
        });
    }
}

async fn recv_datagram(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> std::io::Result<(usize, UdpReply)> {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::socket::{ControlMessageOwned, MsgFlags, SockaddrIn6, recvmsg};
        use std::{
            io::{Error, ErrorKind, IoSliceMut},
            os::fd::AsRawFd,
        };
        let mut control = nix::cmsg_space!(nix::libc::in_pktinfo, nix::libc::in6_pktinfo);
        loop {
            socket.readable().await?;
            match socket.try_io(tokio::io::Interest::READABLE, || {
                let mut buffers = [IoSliceMut::new(buffer)];
                let message = recvmsg::<SockaddrIn6>(
                    socket.as_raw_fd(),
                    &mut buffers,
                    Some(&mut control),
                    MsgFlags::empty(),
                )?;
                if message
                    .flags
                    .intersects(MsgFlags::MSG_TRUNC | MsgFlags::MSG_CTRUNC)
                {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "truncated DNS datagram or packet info",
                    ));
                }
                let peer = message.address.ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "DNS datagram has no peer")
                })?;
                let mapped = peer.ip().to_ipv4_mapped().is_some();
                let mut source = None;
                for control in message.cmsgs()? {
                    match control {
                        ControlMessageOwned::Ipv4PacketInfo(mut info) if mapped => {
                            info.ipi_spec_dst = info.ipi_addr;
                            source = Some(PacketInfo::V4(info));
                        }
                        ControlMessageOwned::Ipv6PacketInfo(info) if !mapped => {
                            source = Some(PacketInfo::V6(info))
                        }
                        _ => {}
                    }
                }
                let source = source.ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "DNS datagram has no local packet info",
                    )
                })?;
                Ok((
                    message.bytes,
                    UdpReply {
                        peer: peer.into(),
                        source,
                    },
                ))
            }) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                result => return result,
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let (length, peer) = socket.recv_from(buffer).await?;
        Ok((length, UdpReply { peer }))
    }
}

async fn send_datagram(
    socket: &UdpSocket,
    packet: &[u8],
    reply: &UdpReply,
) -> std::io::Result<usize> {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::socket::{ControlMessage, MsgFlags, SockaddrStorage, sendmsg};
        use std::{
            io::{ErrorKind, IoSlice},
            os::fd::AsRawFd,
        };
        // Reply from the destination/interface of this request, not wildcard source selection.
        let control = match &reply.source {
            PacketInfo::V4(info) => ControlMessage::Ipv4PacketInfo(info),
            PacketInfo::V6(info) => ControlMessage::Ipv6PacketInfo(info),
        };
        let peer = SockaddrStorage::from(reply.peer);
        loop {
            socket.writable().await?;
            match socket.try_io(tokio::io::Interest::WRITABLE, || {
                sendmsg(
                    socket.as_raw_fd(),
                    &[IoSlice::new(packet)],
                    &[control],
                    MsgFlags::empty(),
                    Some(&peer),
                )
                .map_err(std::io::Error::from)
            }) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                result => return result,
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        socket.send_to(packet, reply.peer).await
    }
}

async fn serve_tcp(server: Arc<Server>) -> Result<()> {
    loop {
        let (mut stream, peer) = server.tcp.accept().await?;
        // Reject rather than queue: idle clients must not consume request workers.
        let Ok(connection) = Arc::clone(&server.tcp_connections).try_acquire_owned() else {
            continue;
        };
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let _connection = connection;
            let result: Result<()> = async {
                loop {
                    // One deadline for the entire frame, not a fresh timeout per byte.
                    let packet = match tokio::time::timeout(TCP_READ_TIMEOUT, async {
                        let length = usize::from(stream.read_u16().await?);
                        let mut packet = vec![0; length];
                        stream.read_exact(&mut packet).await?;
                        Ok::<_, std::io::Error>(packet)
                    })
                    .await
                    .context("FakeDNS TCP frame read timed out")?
                    {
                        Ok(packet) => packet,
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                            return Ok(());
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let response = match Arc::clone(&server.workers).try_acquire_owned() {
                        Ok(permit) => {
                            let worker = Arc::clone(&server);
                            tokio::task::spawn_blocking(move || {
                                let _permit = permit;
                                worker.process(packet)
                            })
                            .await
                            .context("FakeDNS TCP worker failed")?
                        }
                        Err(_) => failure_response(&packet),
                    };
                    let length =
                        u16::try_from(response.len()).context("DNS response is too large")?;
                    tokio::time::timeout(TCP_WRITE_TIMEOUT, async {
                        stream.write_u16(length).await?;
                        stream.write_all(&response).await
                    })
                    .await
                    .context("FakeDNS TCP response write timed out")??;
                }
            }
            .await;
            if let Err(error) = result {
                warn!(%error, %peer, "FakeDNS TCP connection failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::{Message, Query, ResponseCode},
        rr::{Name, RecordType},
    };
    use tokio::net::TcpStream;

    async fn loopback_server() -> Arc<Server> {
        let udp = UdpSocket::bind("[::1]:0").await.unwrap();
        #[cfg(target_os = "linux")]
        nix::sys::socket::setsockopt(&udp, nix::sys::socket::sockopt::Ipv6RecvPacketInfo, &true)
            .unwrap();
        Arc::new(Server {
            dns: Arc::new(FakeDns::open(std::path::Path::new(":memory:")).unwrap()),
            policy: Arc::new(RwLock::new(
                RoutingPolicy::compile(
                    crate::model::RoutingConfig {
                        domain_rules: vec![],
                        ip_rules: vec![],
                        ..crate::model::RoutingConfig::default()
                    },
                    Arc::new(crate::geodata::GeoData::default()),
                )
                .unwrap(),
            )),
            upstream: "127.0.0.1:9".parse().unwrap(),
            lan: LanContext {
                device: "lo".into(),
                address: "127.0.0.1".parse().unwrap(),
                subnet: "127.0.0.0/8".parse().unwrap(),
            },
            udp,
            tcp: TcpListener::bind("[::1]:0").await.unwrap(),
            workers: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            tcp_connections: Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS)),
        })
    }

    fn panel_query(id: u16) -> Vec<u8> {
        let mut request = Message::new();
        request.set_id(id).add_query(Query::query(
            Name::from_ascii("wifi.gofro.net.").unwrap(),
            RecordType::A,
        ));
        request.to_vec().unwrap()
    }

    async fn wait_for_connections(server: &Server, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while server.tcp_connections.available_permits() != MAX_TCP_CONNECTIONS - count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("TCP connection permits did not reach expected count");
    }

    #[tokio::test]
    async fn tcp_idle_and_partial_frames_are_capped_expire_and_leave_udp_serviceable() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let server = loopback_server().await;
            let tcp = tokio::spawn(serve_tcp(Arc::clone(&server)));
            let udp = tokio::spawn(serve_udp(Arc::clone(&server)));
            let address = server.tcp.local_addr().unwrap();
            let mut clients = Vec::new();
            for index in 0..MAX_TCP_CONNECTIONS {
                let mut client = TcpStream::connect(address).await.unwrap();
                match index % 3 {
                    0 => {} // Idle, with no header.
                    1 => client.write_all(&[255]).await.unwrap(),
                    _ => client.write_all(&[255, 255, 0]).await.unwrap(),
                }
                clients.push(client);
            }
            wait_for_connections(&server, MAX_TCP_CONNECTIONS).await;
            assert_eq!(server.workers.available_permits(), MAX_CONCURRENT_REQUESTS);
            tokio::time::timeout(Duration::from_secs(3), async {
                let mut excess = TcpStream::connect(address).await.unwrap();
                let result = excess.read(&mut [0]).await;
                assert!(matches!(result, Ok(0)) || result.is_err(), "{result:?}");

                let client = UdpSocket::bind("[::1]:0").await.unwrap();
                let destination = server.udp.local_addr().unwrap();
                client.send_to(&panel_query(44), destination).await.unwrap();
                let mut buffer = [0; 512];
                let (length, source) = client.recv_from(&mut buffer).await.unwrap();
                assert_eq!(source, destination);
                let response = Message::from_vec(&buffer[..length]).unwrap();
                assert_eq!(response.id(), 44);
                assert_eq!(response.response_code(), ResponseCode::NoError);
                assert_eq!(response.answers().len(), 1);
            })
            .await
            .expect("excess TCP rejection or UDP response stalled");
            tokio::time::timeout(TCP_READ_TIMEOUT + Duration::from_secs(3), async {
                for client in &mut clients {
                    let result = client.read(&mut [0]).await;
                    assert!(matches!(result, Ok(0)) || result.is_err(), "{result:?}");
                }
            })
            .await
            .expect("idle or partial TCP frame did not expire");
            wait_for_connections(&server, 0).await;
            let client = TcpStream::connect(address).await.unwrap();
            wait_for_connections(&server, 1).await;
            drop(client);
            wait_for_connections(&server, 0).await;
            tcp.abort();
            udp.abort();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn tcp_active_connection_handles_multiple_queries_and_renews_frame_deadline() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let server = loopback_server().await;
            let tcp = tokio::spawn(serve_tcp(Arc::clone(&server)));
            let mut client = TcpStream::connect(server.tcp.local_addr().unwrap())
                .await
                .unwrap();
            for id in 0..3 {
                if id > 0 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                let packet = panel_query(id);
                client
                    .write_u16(u16::try_from(packet.len()).unwrap())
                    .await
                    .unwrap();
                client.write_all(&packet).await.unwrap();
                let length = usize::from(client.read_u16().await.unwrap());
                let mut response = vec![0; length];
                client.read_exact(&mut response).await.unwrap();
                let response = Message::from_vec(&response).unwrap();
                assert_eq!(response.id(), id);
                assert_eq!(response.response_code(), ResponseCode::NoError);
                assert_eq!(response.answers().len(), 1);
                wait_for_connections(&server, 1).await;
            }
            drop(client);
            wait_for_connections(&server, 0).await;
            tcp.abort();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_lan_and_non_lan_listen_before_binding() {
        let mut lan = LanContext {
            device: "br-lan".into(),
            address: "192.168.1.1".parse().unwrap(),
            subnet: "192.168.1.0/24".parse().unwrap(),
        };
        for address in ["0.0.0.0:5353", "192.0.2.1:5353", "[::]:5353"] {
            assert!(
                bind_lan_sockets(address.parse().unwrap(), &lan)
                    .await
                    .is_err()
            );
        }
        let listen = SocketAddr::from((lan.address, 5353));
        for device in ["", "br-lan\0wan", "interface-name-too-long"] {
            lan.device = device.into();
            let error = bind_lan_sockets(listen, &lan).await.unwrap_err();
            assert!(error.to_string().contains("invalid interface name"));
        }
        lan.device = "br-lan".into();
        lan.subnet = "192.168.2.0/24".parse().unwrap();
        assert!(bind_lan_sockets(listen, &lan).await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires Linux network namespace with CAP_NET_RAW and IPv6"]
    async fn linux_sockets_are_dual_stack_and_device_bound_or_fail_closed() {
        let mut lan = LanContext {
            device: "lo".into(),
            address: "192.168.1.1".parse().unwrap(),
            subnet: "192.168.1.0/24".parse().unwrap(),
        };
        let listen = SocketAddr::from((lan.address, 0));
        let (udp, tcp) = bind_lan_sockets(listen, &lan).await.unwrap();
        assert!(!socket2::SockRef::from(&udp).reuse_address().unwrap());
        assert!(!socket2::SockRef::from(&udp).reuse_port().unwrap());
        for socket in [socket2::SockRef::from(&udp), socket2::SockRef::from(&tcp)] {
            assert_eq!(socket.device().unwrap().as_deref(), Some(b"lo".as_slice()));
            assert!(!socket.only_v6().unwrap());
            assert!(
                socket
                    .local_addr()
                    .unwrap()
                    .as_socket_ipv6()
                    .unwrap()
                    .ip()
                    .is_unspecified()
            );
        }
        let port = udp.local_addr().unwrap().port();
        for (bind, destination) in [
            ("127.0.0.1:0", SocketAddr::from(([127, 0, 0, 2], port))),
            (
                "[::1]:0",
                SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
            ),
        ] {
            tokio::time::timeout(Duration::from_secs(2), async {
                let client = UdpSocket::bind(bind).await.unwrap();
                client.connect(destination).await.unwrap();
                let mut request = hickory_proto::op::Message::new();
                request.set_id(42);
                let request = request.to_vec().unwrap();
                client.send(&request).await.unwrap();
                let mut buffer = [0; 512];
                let (length, reply) = recv_datagram(&udp, &mut buffer).await.unwrap();
                let source = match &reply.source {
                    PacketInfo::V4(info) => IpAddr::V4(std::net::Ipv4Addr::from(
                        info.ipi_spec_dst.s_addr.to_ne_bytes(),
                    )),
                    PacketInfo::V6(info) => {
                        IpAddr::V6(std::net::Ipv6Addr::from(info.ipi6_addr.s6_addr))
                    }
                };
                assert_eq!(source, destination.ip());
                // Overload responses must use the same source-preserving send path.
                send_datagram(&udp, &failure_response(&buffer[..length]), &reply)
                    .await
                    .unwrap();
                let (length, source) = client.recv_from(&mut buffer).await.unwrap();
                assert_eq!(source, destination);
                let response = hickory_proto::op::Message::from_vec(&buffer[..length]).unwrap();
                assert_eq!(response.id(), 42);
                assert_eq!(
                    response.response_code(),
                    hickory_proto::op::ResponseCode::ServFail
                );
            })
            .await
            .unwrap();
        }
        lan.device = "gofro-no-device".into();
        let error = bind_lan_sockets(listen, &lan).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to restrict FakeDNS to LAN device")
        );
    }
}

use std::{
    net::SocketAddr,
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
use crate::routing::RoutingPolicy;

const MAX_CONCURRENT_REQUESTS: usize = 16;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) struct Server {
    dns: Arc<FakeDns>,
    policy: Arc<RwLock<RoutingPolicy>>,
    upstream: SocketAddr,
    udp: UdpSocket,
    tcp: TcpListener,
    workers: Arc<Semaphore>,
}

impl Server {
    pub(crate) async fn bind(
        listen: SocketAddr,
        upstream: SocketAddr,
        dns: Arc<FakeDns>,
        policy: Arc<RwLock<RoutingPolicy>>,
    ) -> Result<Self> {
        Ok(Self {
            dns,
            policy,
            upstream,
            udp: UdpSocket::bind(listen)
                .await
                .with_context(|| format!("failed to bind FakeDNS UDP {listen}"))?,
            tcp: TcpListener::bind(listen)
                .await
                .with_context(|| format!("failed to bind FakeDNS TCP {listen}"))?,
            workers: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
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
            .process(&packet, &policy, self.upstream)
            .unwrap_or_else(|error| {
                warn!(%error, "FakeDNS request failed");
                failure_response(&packet)
            })
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
        let (length, peer) = server.udp.recv_from(&mut buffer).await?;
        let packet = buffer[..length].to_vec();
        let server = Arc::clone(&server);
        let Ok(permit) = Arc::clone(&server.workers).try_acquire_owned() else {
            if let Err(error) = server.udp.send_to(&failure_response(&packet), peer).await {
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
                    if let Err(error) = server.udp.send_to(&response, peer).await {
                        warn!(%error, %peer, "failed to send FakeDNS UDP response");
                    }
                }
                Err(error) => warn!(%error, "FakeDNS UDP worker failed"),
            }
        });
    }
}

async fn serve_tcp(server: Arc<Server>) -> Result<()> {
    loop {
        let (mut stream, peer) = server.tcp.accept().await?;
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let result: Result<()> = async {
                loop {
                    let length = match stream.read_u16().await {
                        Ok(length) => usize::from(length),
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                            return Ok(());
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let mut packet = vec![0; length];
                    stream.read_exact(&mut packet).await?;
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
                    stream.write_u16(length).await?;
                    stream.write_all(&response).await?;
                }
            }
            .await;
            if let Err(error) = result {
                warn!(%error, %peer, "FakeDNS TCP connection failed");
            }
        });
    }
}

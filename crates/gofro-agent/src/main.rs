#![forbid(unsafe_code)]

mod api;
mod auth;
mod config;
mod controller;
mod dataplane;
mod fake_dns;
mod geodata;
mod managed;
mod model;
mod network;
mod onboarding;
mod routing;
mod stats;
mod tls;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use fake_dns::FakeDns;
use geodata::GeoData;
use model::{ControllerConfig, LanContext};
use routing::RoutingPolicy;
use stats::StatsTracker;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Local controller for Gofro Router", version)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    #[arg(long, default_value = "127.0.0.1:8081")]
    http_listen: SocketAddr,

    #[arg(long, default_value = "127.0.0.1:8443")]
    https_listen: SocketAddr,

    #[arg(long, default_value = "/etc/gofro/tls-cert.pem")]
    tls_cert: PathBuf,

    #[arg(long, default_value = "/etc/gofro/tls-key.pem")]
    tls_key: PathBuf,

    #[arg(long)]
    init_security: bool,

    #[arg(long, default_value = "/etc/gofro/admin-password")]
    admin_password: PathBuf,

    #[arg(long, default_value = "/etc/gofro/setup-code")]
    setup_code: PathBuf,

    #[arg(long, default_value = "gt0")]
    interface: String,

    #[arg(long)]
    lan_interface: Option<String>,

    #[arg(long)]
    lan_subnet: Option<ipnet::Ipv4Net>,

    #[arg(long, default_value = "/etc/gofro/controller.json")]
    config: PathBuf,

    #[arg(long, default_value = "/etc/gofro/managed-vps")]
    management_dir: PathBuf,

    #[arg(long, default_value = "/usr/libexec/gofro/mode")]
    mode_command: PathBuf,

    #[arg(long, default_value = "/usr/share/gofro/geosite.dat")]
    geosite: PathBuf,

    #[arg(long, default_value = "/usr/share/gofro/geoip.dat")]
    geoip: PathBuf,

    #[arg(long)]
    dns_listen: Option<SocketAddr>,

    #[arg(long)]
    dns_upstream: Option<SocketAddr>,

    #[arg(long, default_value = "/tmp/gofro/routing.sqlite")]
    routing_state: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) interface: String,
    pub(crate) lan: LanContext,
    pub(crate) http_listen: SocketAddr,
    pub(crate) https_listen: SocketAddr,
    pub(crate) dns_listen: SocketAddr,
    pub(crate) config_path: PathBuf,
    pub(crate) mode_command: PathBuf,
    pub(crate) management_dir: PathBuf,
    pub(crate) config: Arc<Mutex<ControllerConfig>>,
    pub(crate) stats: Arc<Mutex<StatsTracker>>,
    pub(crate) geodata: Arc<GeoData>,
    pub(crate) routing: Arc<RwLock<RoutingPolicy>>,
    pub(crate) routing_degraded: Arc<AtomicBool>,
    pub(crate) fake_dns: Arc<FakeDns>,
    pub(crate) auth: Arc<auth::Auth>,
    pub(crate) managed_operations: Arc<Mutex<()>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("gofro_agent=info".parse()?))
        .init();

    let args = Args::parse();
    if args.init_security {
        println!("{}", init_security(&args)?);
        return Ok(());
    }
    if !args.listen.ip().is_loopback() {
        bail!("--listen must be loopback-only");
    }
    model::PanelPorts {
        http: args.http_listen.port(),
        https: args.https_listen.port(),
    }
    .validate()?;
    ipv4(args.http_listen)?;
    let lan_address = ipv4(args.https_listen)?;
    if args.http_listen.ip() != IpAddr::V4(lan_address) {
        bail!("--http-listen and --https-listen must use the LAN address");
    }
    let lan = LanContext {
        device: args.lan_interface.context("--lan-interface is required")?,
        address: lan_address,
        subnet: args.lan_subnet.context("--lan-subnet is required")?,
    };
    validate_lan(&lan)?;
    validate_interface(&args.interface)?;
    let dns_listen = args
        .dns_listen
        .unwrap_or(SocketAddr::new(IpAddr::V4(lan_address), 5353));
    let dns_upstream = args
        .dns_upstream
        .unwrap_or(SocketAddr::new(IpAddr::V4(lan_address), 53));
    if dns_listen.ip() != IpAddr::V4(lan_address)
        || dns_upstream.port() != 53
        || !(dns_upstream.ip().is_loopback() || dns_upstream.ip() == IpAddr::V4(lan_address))
        || dns_upstream == dns_listen
    {
        bail!("DNS must listen on the LAN address and use local DNS port 53");
    }
    // Keep forwarding closed even if configuration, TLS or listener setup fails.
    dataplane::install_guard(&lan).context("failed to protect LAN forwarding")?;
    let config = config::load(&args.config)?;
    let geodata = Arc::new(GeoData::load(&args.geosite, &args.geoip)?);
    let routing = RoutingPolicy::compile(config.routing.clone(), Arc::clone(&geodata))?;
    let fake_dns = Arc::new(FakeDns::open(&args.routing_state)?);
    fake_dns.set_vpn_enabled(config.vpn_enabled);
    let tls_fingerprint = tls::ensure(&args.tls_cert, &args.tls_key, lan_address)?;
    let auth = Arc::new(auth::Auth::open(args.admin_password, args.setup_code)?);
    let state = AppState {
        interface: args.interface,
        lan,
        http_listen: args.http_listen,
        https_listen: args.https_listen,
        dns_listen,
        config_path: args.config,
        mode_command: args.mode_command,
        management_dir: args.management_dir,
        config: Arc::new(Mutex::new(config)),
        stats: Arc::new(Mutex::new(StatsTracker::default())),
        geodata,
        routing: Arc::new(RwLock::new(routing)),
        routing_degraded: Arc::new(AtomicBool::new(true)),
        fake_dns,
        auth,
        managed_operations: Arc::new(Mutex::new(())),
    };

    let dns = fake_dns::Server::bind(
        dns_listen,
        dns_upstream,
        Arc::clone(&state.fake_dns),
        Arc::clone(&state.routing),
        &state.lan,
    )
    .await?;
    let mut dns_task = tokio::spawn(dns.run());

    let health_listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to bind {}", args.listen))?;
    let http_listener = tokio::net::TcpListener::bind(args.http_listen)
        .await
        .with_context(|| format!("failed to bind {}", args.http_listen))?;
    let https_listener = tokio::net::TcpListener::bind(args.https_listen)
        .await
        .with_context(|| format!("failed to bind {}", args.https_listen))?;
    let tls_config =
        axum_server::tls_openssl::OpenSSLConfig::from_pem_file(&args.tls_cert, &args.tls_key)?;
    if let Err(error) = controller::reconcile(&state) {
        error!(%error, "network recovery failed; forwarding remains guarded, panel available for diagnostics");
    }
    info!(health = %args.listen, https = %args.https_listen, tls_fingerprint, "Gofro agent started");

    let http = async {
        let secure = api::secure_router(state.clone());
        tokio::select! {
            result = axum::serve(health_listener, api::health_router(state.clone())).with_graceful_shutdown(shutdown_signal()) => result.map_err(anyhow::Error::from),
            result = axum::serve(http_listener, api::redirect_router(state.clone())).with_graceful_shutdown(shutdown_signal()) => result.map_err(anyhow::Error::from),
            result = axum_server::from_tcp(https_listener.into_std()?)?.acceptor(axum_server::tls_openssl::OpenSSLAcceptor::new(tls_config)).serve(secure.into_make_service()) => result.map_err(anyhow::Error::from),
        }
    };
    tokio::pin!(http);
    tokio::select! {
        result = &mut http => {
            dns_task.abort();
            result.context("HTTP server failed")
        }
        result = &mut dns_task => {
            result.context("FakeDNS task failed")?;
            bail!("FakeDNS server stopped")
        }
    }
}

fn ipv4(address: SocketAddr) -> Result<Ipv4Addr> {
    match address.ip() {
        IpAddr::V4(value) => Ok(value),
        IpAddr::V6(_) => bail!("panel listen address must be IPv4"),
    }
}
fn validate_interface(interface: &str) -> Result<()> {
    if interface.is_empty()
        || interface.len() > 15
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("invalid interface name")
    }
    Ok(())
}
fn validate_lan(lan: &LanContext) -> Result<()> {
    validate_interface(&lan.device)?;
    if lan.address.is_unspecified()
        || lan.address.is_loopback()
        || lan.address.is_multicast()
        || !lan.subnet.contains(&lan.address)
        || lan.subnet.prefix_len() > 30
        || lan.address == lan.subnet.network()
        || lan.address == lan.subnet.broadcast()
    {
        bail!("LAN address must be a usable host in --lan-subnet");
    }
    for reserved in ["10.202.0.0/24", "198.18.0.0/15"] {
        let reserved = reserved.parse::<ipnet::Ipv4Net>()?;
        if lan.subnet.contains(&reserved.network()) || reserved.contains(&lan.subnet.network()) {
            bail!("LAN subnet overlaps reserved {reserved}");
        }
    }
    Ok(())
}

fn init_security(args: &Args) -> Result<String> {
    tls::ensure(&args.tls_cert, &args.tls_key, ipv4(args.https_listen)?)
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "failed to install shutdown signal handler");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_requires_a_usable_non_reserved_host() {
        for (address, subnet) in [
            ("0.0.0.0", "0.0.0.0/32"),
            ("127.0.0.1", "127.0.0.0/8"),
            ("224.0.0.1", "224.0.0.0/24"),
            ("192.168.4.0", "192.168.4.0/24"),
            ("192.168.4.255", "192.168.4.0/24"),
            ("10.202.0.1", "10.202.0.0/24"),
        ] {
            assert!(
                validate_lan(&LanContext {
                    device: "br-home".into(),
                    address: address.parse().unwrap(),
                    subnet: subnet.parse().unwrap(),
                })
                .is_err(),
                "accepted {address}/{subnet}"
            );
        }
        assert!(
            validate_lan(&LanContext {
                device: "br-home".into(),
                address: "192.168.4.1".parse().unwrap(),
                subnet: "192.168.4.0/24".parse().unwrap(),
            })
            .is_ok()
        );
    }

    #[test]
    fn init_security_only_writes_tls_files() {
        let dir = std::env::temp_dir().join(format!(
            "gofro-init-security-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let cert = dir.join("cert");
        let key = dir.join("key");
        let args = Args::try_parse_from([
            "gofro-agent",
            "--init-security",
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ])
        .unwrap();

        let first = init_security(&args).unwrap();
        assert_eq!(first, init_security(&args).unwrap());
        assert!(cert.exists() && key.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

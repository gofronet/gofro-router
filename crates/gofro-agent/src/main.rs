#![forbid(unsafe_code)]

mod api;
mod config;
mod controller;
mod dataplane;
mod fake_dns;
mod geodata;
mod model;
mod network;
mod routing;
mod stats;
mod wifi;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use fake_dns::FakeDns;
use geodata::GeoData;
use model::ControllerConfig;
use routing::RoutingPolicy;
use stats::StatsTracker;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Local controller for Gofro Router", version)]
struct Args {
    #[arg(long, default_value = "10.203.1.1:8080")]
    listen: SocketAddr,

    #[arg(long, default_value = "10.203.1.1:80")]
    panel_listen: SocketAddr,

    #[arg(long, default_value = "gt0")]
    interface: String,

    #[arg(long, default_value = "br-lan")]
    lan_interface: String,

    #[arg(long, default_value = "phy0-ap0")]
    wifi_interface: String,

    #[arg(long, default_value = "/etc/gofro/controller.json")]
    config: PathBuf,

    #[arg(long, default_value = "/usr/libexec/gofro/mode")]
    mode_command: PathBuf,

    #[arg(long, default_value = "/usr/share/gofro/geosite.dat")]
    geosite: PathBuf,

    #[arg(long, default_value = "/usr/share/gofro/geoip.dat")]
    geoip: PathBuf,

    #[arg(long, default_value = "127.0.0.1:5353")]
    dns_listen: SocketAddr,

    #[arg(long, default_value = "1.1.1.1:53")]
    dns_upstream: SocketAddr,

    #[arg(long, default_value = "/tmp/gofro/routing.sqlite")]
    routing_state: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) interface: String,
    pub(crate) lan_interface: String,
    pub(crate) wifi_interface: String,
    pub(crate) config_path: PathBuf,
    pub(crate) mode_command: PathBuf,
    pub(crate) config: Arc<Mutex<ControllerConfig>>,
    pub(crate) stats: Arc<Mutex<StatsTracker>>,
    pub(crate) geodata: Arc<GeoData>,
    pub(crate) routing: Arc<RwLock<RoutingPolicy>>,
    pub(crate) fake_dns: Arc<FakeDns>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("gofro_agent=info".parse()?))
        .init();

    let args = Args::parse();
    let config = config::load(&args.config)?;
    let geodata = Arc::new(GeoData::load(&args.geosite, &args.geoip)?);
    let routing = RoutingPolicy::compile(config.routing.clone(), Arc::clone(&geodata))?;
    let fake_dns = Arc::new(FakeDns::open(&args.routing_state)?);
    let state = AppState {
        interface: args.interface,
        lan_interface: args.lan_interface,
        wifi_interface: args.wifi_interface,
        config_path: args.config,
        mode_command: args.mode_command,
        config: Arc::new(Mutex::new(config)),
        stats: Arc::new(Mutex::new(StatsTracker::default())),
        geodata,
        routing: Arc::new(RwLock::new(routing)),
        fake_dns,
    };

    controller::reconcile(&state).context("failed to restore configured network mode")?;

    let dns = fake_dns::Server::bind(
        args.dns_listen,
        args.dns_upstream,
        Arc::clone(&state.fake_dns),
        Arc::clone(&state.routing),
    )
    .await?;
    let mut dns_task = tokio::spawn(dns.run());

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to bind {}", args.listen))?;
    let panel_listener = tokio::net::TcpListener::bind(args.panel_listen)
        .await
        .with_context(|| format!("failed to bind {}", args.panel_listen))?;
    info!(listen = %args.listen, "Gofro agent started");

    let http = async {
        let router = api::router(state);
        tokio::select! {
            result = axum::serve(listener, router.clone()).with_graceful_shutdown(shutdown_signal()) => result,
            result = axum::serve(panel_listener, router).with_graceful_shutdown(shutdown_signal()) => result,
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

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "failed to install shutdown signal handler");
    }
}

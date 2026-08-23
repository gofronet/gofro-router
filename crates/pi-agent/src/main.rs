mod api;
mod config;
mod controller;
mod model;
mod network;
mod stats;
mod wifi;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use clap::Parser;
use model::ControllerConfig;
use stats::StatsTracker;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Local controller for the Raspberry Pi gaming tunnel", version)]
struct Args {
    #[arg(long, default_value = "10.203.1.1:80")]
    listen: SocketAddr,

    #[arg(long, default_value = "gt0")]
    interface: String,

    #[arg(long, default_value = "/etc/maxos-game-tunnel/controller.json")]
    config: PathBuf,

    #[arg(long, default_value = "/usr/local/lib/maxos-game-mode")]
    mode_command: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) interface: String,
    pub(crate) config_path: PathBuf,
    pub(crate) mode_command: PathBuf,
    pub(crate) config: Arc<Mutex<ControllerConfig>>,
    pub(crate) stats: Arc<Mutex<StatsTracker>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("pi_agent=info".parse()?))
        .init();

    let args = Args::parse();
    let config = config::load(&args.config)?;
    let state = AppState {
        interface: args.interface,
        config_path: args.config,
        mode_command: args.mode_command,
        config: Arc::new(Mutex::new(config)),
        stats: Arc::new(Mutex::new(StatsTracker::default())),
    };

    if let Err(error) = controller::reconcile(&state) {
        error!(%error, "failed to restore configured network mode");
    }

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to bind {}", args.listen))?;
    info!(listen = %args.listen, "Pi agent started");

    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "failed to install shutdown signal handler");
    }
}

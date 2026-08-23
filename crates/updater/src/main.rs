mod bundle;
mod fsops;
mod manifest;
mod paths;
mod process;
mod state;
mod status;
mod transaction;
mod update;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Signed updater for the Gofro router", version)]
struct Args {
    #[arg(long)]
    force: bool,

    #[arg(long, hide = true)]
    self_check: bool,

    #[arg(long, hide = true)]
    recover_only: bool,

    #[arg(long, hide = true)]
    recover_runtime: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.self_check {
        process::self_check()?;
        println!("gofro-updater {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if args.recover_only {
        transaction::recover_pending(true)?;
        return Ok(());
    }
    if args.recover_runtime {
        transaction::recover_pending(false)?;
        return Ok(());
    }

    if transaction::recover_pending(false)? {
        return Ok(());
    }
    transaction::reconcile_updater()?;
    update::run(args.force)
}

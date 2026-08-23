mod api;
mod bundle;
mod fsops;
mod manifest;
mod paths;
mod process;
mod progress;
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
    check: bool,

    #[arg(long, hide = true)]
    self_check: bool,

    #[arg(long, hide = true)]
    recover_only: bool,

    #[arg(long, hide = true)]
    recover_runtime: bool,

    #[arg(long, hide = true)]
    serve: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let record_error = !args.self_check && !args.serve;
    let result = run(args);
    if record_error {
        if let Err(error) = &result {
            let _ = progress::write_error(error);
        }
    }
    result
}

fn run(args: Args) -> Result<()> {
    if args.self_check {
        process::self_check()?;
        println!("gofro-updater {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.serve {
        return api::serve();
    }

    if args.recover_only {
        return recover(true);
    }
    if args.recover_runtime {
        return recover(false);
    }

    if transaction::recover_pending(false)? {
        return Ok(());
    }
    transaction::reconcile_updater()?;
    if args.check {
        update::check()
    } else {
        update::run(args.force)
    }
}

fn recover(boot: bool) -> Result<()> {
    if !transaction::recover_pending(boot)? && progress::read()?.active() {
        progress::write(&progress::UpdateProgress::error(
            "update operation was interrupted",
        ))?;
    }
    Ok(())
}

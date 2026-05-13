//! `open-interceptor` binary entrypoint.
//!
//! Parses CLI args, initializes structured logging, and dispatches to the
//! requested subcommand. The async runtime is multi-thread Tokio (proxy
//! workloads benefit from work-stealing when streaming responses concurrent
//! to other clients).

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod cli;
mod config;
mod providers;
mod translate;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = cli::Cli::parse();
    if let Err(err) = cli::dispatch(args.command).await {
        // Print the full chain so root causes don't hide behind a generic
        // wrapper message. anyhow's Debug formatter already does this.
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
    Ok(())
}

/// Initialize `tracing-subscriber` with a default filter that shows our
/// own logs at INFO and silences chatty dependencies. Respects
/// `RUST_LOG` if set, so users can opt into verbose mode.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("open_interceptor=info,tower_http=warn,reqwest=warn,hyper=warn")
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(true)
        .compact()
        .init();
}

//! `open-interceptor` binary entrypoint.
//!
//! Parses CLI args, initializes structured logging, and dispatches to the
//! requested subcommand. The async runtime is multi-thread Tokio (proxy
//! workloads benefit from work-stealing when streaming responses concurrent
//! to other clients).

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod cli;
mod daemon;
mod domain;
mod router;
mod services;
mod proxy;
mod providers;
mod translate;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_tracing();
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
///
/// Logs go to stderr (visible in the terminal for `run`, captured by
/// launchd for the daemon) AND to a daily-rotating file in
/// `~/Library/Logs/open-interceptor/` (macOS standard location for
/// user-space daemon logs) with 7-day retention.
fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("open_interceptor=info,tower_http=warn,reqwest=warn,hyper=warn")
    });

    // ---- stderr layer: always on (terminal visibility) ----------------
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true)
        .compact()
        .with_writer(std::io::stderr);

    // ---- rolling file layer: daily rotation, retain 7 days -----------
    // macOS standard location for user-space daemon logs
    let log_dir = dirs_home().join("Library").join("Logs").join("open-interceptor");
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(7)
        .filename_prefix("open-interceptor")
        .filename_suffix("log")
        .build(log_dir)
        .expect("failed to create rolling log appender");

    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true)
        .compact()
        .with_ansi(false)
        .with_writer(non_blocking_file);

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    guard
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

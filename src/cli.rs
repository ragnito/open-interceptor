//! Command-line interface for `open-interceptor`.
//!
//! Subcommands:
//!   run     — foreground server (used by launchd and for development)
//!   start   — install + launch the daemon
//!   stop    — stop the daemon
//!   status  — check daemon health
//!   logs    — tail daemon logs
//!   config  — validate a config file
//!   config edit — interactive TUI config editor (Phase 5)

use std::path::{Path, PathBuf};

use crate::daemon;
use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "open-interceptor",
    version,
    about = "Local proxy that auto-routes Claude Code traffic to the right provider"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the proxy in the foreground. This is what launchd executes in
    /// production; for local development just invoke it directly.
    Run {
        /// Path to the YAML config. `~` is expanded.
        #[arg(short, long, default_value = "~/.config/open-interceptor/config.yaml")]
        config: PathBuf,
    },

    /// Register and start the proxy as a launchd background agent.
    /// Use --install on first run to create the plist.
    Start {
        /// Install the launchd plist before starting.
        #[arg(long)]
        install: bool,

        /// Path to the open-interceptor binary (only needed with --install).
        /// Defaults to the current executable.
        #[arg(long)]
        binary: Option<String>,
    },

    /// Stop the launchd background agent.
    Stop,

    /// Show whether the daemon is running and on which port.
    Status,

    /// Tail the daemon logs.
    Logs {
        /// Follow the log file (`tail -f`).
        #[arg(long)]
        follow: bool,
    },

    /// Validate a config file without starting the proxy.
    Config {
        #[arg(short, long, default_value = "~/.open-interceptor/config.yaml")]
        config: PathBuf,
    },

    /// Open the interactive config editor (TUI).
    ConfigEdit {
        /// Path to the YAML config. `~` is expanded.
        #[arg(short, long, default_value = "~/.config/open-interceptor/config.yaml")]
        config: PathBuf,
    },
}

/// Entrypoint called from `main` after parsing args.
pub async fn dispatch(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run { config } => run(&config).await,
        Command::Start { install, binary } => do_start(install, binary),
        Command::Stop => daemon::stop(),
        Command::Status => daemon::status(),
        Command::Logs { follow } => do_logs(follow),
        Command::Config { config } => validate(&config),
        Command::ConfigEdit { config } => config_edit(&config),
    }
}

fn do_start(install_first: bool, binary_path: Option<String>) -> anyhow::Result<()> {
    if install_first {
        let bin = binary_path.unwrap_or_else(|| {
            // When install flag is given without an explicit binary path,
            // assume `open-interceptor` is on PATH.
            "open-interceptor".to_string()
        });
        daemon::install(&bin)?;
    }
    daemon::start()
}

fn do_logs(follow: bool) -> anyhow::Result<()> {
    let log_dir = dirs_home().join("Library").join("Logs").join("open-interceptor");
    if !log_dir.exists() {
        anyhow::bail!("log directory not found: {}", log_dir.display());
    }

    let stderr_log = log_dir.join("stderr.log");

    if follow {
        // tail -f on the log file
        let mut child = std::process::Command::new("tail")
            .args(["-f", stderr_log.to_str().unwrap()])
            .spawn()
            .context("spawning tail")?;
        child.wait().context("waiting for tail")?;
    } else {
        // Print the last ~20 lines
        if stderr_log.exists() {
            let content = std::fs::read_to_string(&stderr_log).context("reading log")?;
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(20);
            for line in &lines[start..] {
                println!("{line}");
            }
        } else {
            println!("(no logs yet)");
        }
    }
    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Foreground run: load config, build router, start the Axum server.
async fn run(config_path: &Path) -> anyhow::Result<()> {
    let path = expand_tilde(config_path);
    let config = crate::services::config::ConfigService::load(&path)
        .map_err(|e| anyhow::anyhow!("failed to load config from {}: {e}", path.display()))?;

    tracing::info!(
        port = config.port,
        providers = config.providers.len(),
        routes = config.routes.len(),
        "config loaded",
    );

    let router = std::sync::Arc::new(
        crate::router::Router::build(config)
            .map_err(|e| anyhow::anyhow!("router build failed: {e}"))?,
    );

    crate::proxy::serve(router).await
}

/// Validate a config file without starting anything.
fn validate(config_path: &Path) -> anyhow::Result<()> {
    let path = expand_tilde(config_path);
    let config = crate::services::config::ConfigService::load(&path)
        .map_err(|e| anyhow::anyhow!("invalid config at {}: {e}", path.display()))?;
    println!(
        "ok: {} providers, {} routes, port {}",
        config.providers.len(),
        config.routes.len(),
        config.port
    );
    Ok(())
}

/// Placeholder for the interactive TUI config editor.
fn config_edit(_config_path: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "interactive config editor not implemented yet — use a text editor to edit the config file"
    );
}

/// Resolve `~` in a `PathBuf`.
fn expand_tilde(p: &Path) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p.to_string_lossy().as_ref()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn run_subcommand_parses() {
        let cli =
            Cli::try_parse_from(["open-interceptor", "run", "--config", "/tmp/test.yaml"]).unwrap();
        match cli.command {
            Command::Run { config } => assert_eq!(config, PathBuf::from("/tmp/test.yaml")),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn start_subcommand_parses() {
        let cli = Cli::try_parse_from(["open-interceptor", "start"]).unwrap();
        assert!(matches!(cli.command, Command::Start { .. }));
    }

    #[test]
    fn stop_status_logs_config_parse() {
        for sub in &["stop", "status"] {
            let cli = Cli::try_parse_from(["open-interceptor", sub]).unwrap();
            assert!(matches!(cli.command, Command::Stop | Command::Status));
        }
    }

    #[test]
    fn config_edit_subcommand_parses() {
        let cli = Cli::try_parse_from(["open-interceptor", "config-edit"]).unwrap();
        assert!(matches!(cli.command, Command::ConfigEdit { .. }));
    }
}

//! Command-line interface for `open-interceptor`.
//!
//! Phase 1 only implements `run --config <path>`. The daemon-management
//! subcommands (`start`, `stop`, `status`, `logs`, `config validate`,
//! `config edit`) are declared here but return a "not implemented" error
//! until Phase 4 wires them to launchd.

use std::path::{Path, PathBuf};

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
        #[arg(short, long, default_value = "~/.open-interceptor/config.yaml")]
        config: PathBuf,
    },

    /// Register and start the proxy as a launchd background agent.
    /// (Phase 4 — not implemented yet.)
    Start,

    /// Stop the launchd background agent.
    /// (Phase 4 — not implemented yet.)
    Stop,

    /// Show whether the daemon is running and on which port.
    /// (Phase 4 — not implemented yet.)
    Status,

    /// Tail the daemon logs.
    /// (Phase 4 — not implemented yet.)
    Logs {
        /// Follow the log file (`tail -f`).
        #[arg(long)]
        follow: bool,
    },

    /// Validate a config file without starting the proxy.
    /// (Phase 4 — not implemented yet.)
    ConfigValidate {
        #[arg(short, long, default_value = "~/.open-interceptor/config.yaml")]
        config: PathBuf,
    },
}

/// Entrypoint called from `main` after parsing args. Dispatches to the
/// matching handler and propagates errors via anyhow so the caller can
/// print a friendly message.
pub async fn dispatch(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Run { config } => run(&config).await,
        Command::Start | Command::Stop | Command::Status | Command::Logs { .. } => {
            anyhow::bail!(
                "this subcommand is part of Phase 4 (launchd integration) and is not implemented yet"
            )
        }
        Command::ConfigValidate { config } => validate(&config),
    }
}

/// Foreground run: load config, log a summary, then (Phase 1 WIP) hand off
/// to the Axum proxy server.
async fn run(config_path: &Path) -> anyhow::Result<()> {
    let path = expand_tilde(config_path);
    let config = crate::config::Config::load(&path).map_err(|e| {
        anyhow::anyhow!("failed to load config from {}: {e}", path.display())
    })?;

    tracing::info!(
        port = config.port,
        providers = config.providers.len(),
        routes = config.routes.len(),
        "config loaded",
    );

    // TODO(T1.5): proxy::serve(config).await — for now this just verifies
    // that the config parses and exits.
    tracing::warn!(
        "proxy server not implemented yet (T1.5). Config validated, exiting."
    );

    Ok(())
}

/// Validate a config file without starting anything. Phase 4 stub that
/// also works today since the loader does the validation.
fn validate(config_path: &Path) -> anyhow::Result<()> {
    let path = expand_tilde(config_path);
    let config = crate::config::Config::load(&path).map_err(|e| {
        anyhow::anyhow!("invalid config at {}: {e}", path.display())
    })?;
    println!(
        "ok: {} providers, {} routes, port {}",
        config.providers.len(),
        config.routes.len(),
        config.port
    );
    Ok(())
}

/// Resolve `~` in a `PathBuf`. Done here so the rest of the codebase
/// always sees absolute paths.
fn expand_tilde(p: &Path) -> PathBuf {
    PathBuf::from(shellexpand::tilde(p.to_string_lossy().as_ref()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        // Compile-time sanity: panics if the derive macros generated an
        // invalid command graph.
        Cli::command().debug_assert();
    }

    #[test]
    fn run_subcommand_parses() {
        let cli = Cli::try_parse_from([
            "open-interceptor",
            "run",
            "--config",
            "/tmp/test.yaml",
        ])
        .unwrap();
        match cli.command {
            Command::Run { config } => assert_eq!(config, PathBuf::from("/tmp/test.yaml")),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn unimplemented_subcommands_fail_clearly() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(dispatch(Command::Start)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Phase 4"), "got: {msg}");
    }
}

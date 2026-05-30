//! Command-line interface for `open-interceptor`.
//!
//! Subcommands:
//!   run     — foreground server (used by launchd and for development)
//!   start   — install + launch the daemon
//!   stop    — stop the daemon
//!   status  — check daemon health
//!   logs    — tail daemon logs
//!   config  — validate a config file
//!   claude      — run `claude` with proxy auto-started and env vars injected
//!   claude-app  — launch Claude.app desktop with proxy env vars
//!   config-edit — interactive TUI config editor (Phase 5)

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

    /// Run `claude`, auto-starting the proxy if needed and injecting
    /// ANTHROPIC_BASE_URL + CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY.
    /// All trailing arguments are forwarded verbatim to `claude`.
    Claude {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Launch the Claude desktop app with ANTHROPIC_BASE_URL injected
    /// so it routes through the local proxy.
    ClaudeApp,
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
        Command::Claude { args } => do_claude(args).await,
        Command::ClaudeApp => do_claude_app().await,
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
    let log_dir = dirs_home()
        .join("Library")
        .join("Logs")
        .join("open-interceptor");
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

/// Ensure the proxy daemon is running, auto-starting it if needed.
async fn ensure_daemon_running() -> anyhow::Result<()> {
    if daemon::probe() {
        return Ok(());
    }

    eprintln!("open-interceptor: proxy not running — starting...");

    if !daemon::is_installed() {
        let exe =
            std::env::current_exe().context("could not determine current executable path")?;
        let exe = exe
            .canonicalize()
            .context("could not canonicalize current executable path")?;
        let exe_str = exe.to_string_lossy();
        daemon::install(&exe_str).context("failed to install launchd plist")?;
    }

    daemon::start().context("failed to start proxy daemon")?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if daemon::probe() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "proxy did not start within 15 s.\n\
                 Check logs:  open-interceptor logs\n\
                 Plist:       ~/Library/LaunchAgents/com.open-interceptor.plist"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    eprintln!(
        "open-interceptor: proxy ready at http://127.0.0.1:{}",
        daemon::PROXY_PORT
    );
    Ok(())
}

/// `open-interceptor claude [args...]` — Ollama-style wrapper.
///
/// 1. Ensures the proxy is up (auto-starts if needed).
/// 2. Injects ANTHROPIC_BASE_URL + CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY.
/// 3. Replaces the process image with the real `claude` binary (preserves
///    TTY, signals, exit code).
async fn do_claude(args: Vec<String>) -> anyhow::Result<()> {
    ensure_daemon_running().await?;

    // Locate the real `claude` binary via PATH.
    let claude_path = find_in_path("claude").ok_or_else(|| {
        anyhow::anyhow!(
            "`claude` not found on PATH.\n\
             Install Claude Code: https://claude.ai/code"
        )
    })?;

    // Recursion guard: bail if `claude` resolves to ourselves.
    if let (Ok(them), Ok(us)) = (
        claude_path.canonicalize(),
        std::env::current_exe().and_then(|p| p.canonicalize()),
    ) && them == us
    {
        anyhow::bail!(
            "`claude` on PATH resolves to `open-interceptor` itself — \
             that would cause infinite recursion. \
             Fix your PATH so the real Claude Code binary comes first."
        );
    }

    // Inject the proxy env vars. Safety: we are about to exec() — no other
    // threads are running at this point in the dispatch path.
    unsafe {
        std::env::set_var(
            "ANTHROPIC_BASE_URL",
            format!("http://127.0.0.1:{}", daemon::PROXY_PORT),
        );
        std::env::set_var("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1");
    }

    // Replace the process image (Unix exec — preserves TTY, signals, exit code).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&claude_path).args(&args).exec();
        anyhow::bail!("failed to exec `{}`: {err}", claude_path.display());
    }

    // Non-Unix fallback (keeps `cargo check` on Windows green; not used on macOS).
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&claude_path)
            .args(&args)
            .status()
            .context("failed to run `claude`")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// `open-interceptor claude-app` — launch Claude desktop with proxy env vars.
///
/// macOS GUI apps don't inherit shell environment variables. We use
/// `launchctl setenv` to inject them into the GUI session domain,
/// launch the app with `open -a`, then clean up.
async fn do_claude_app() -> anyhow::Result<()> {
    ensure_daemon_running().await?;

    let base_url = format!("http://127.0.0.1:{}", daemon::PROXY_PORT);

    std::process::Command::new("launchctl")
        .args(["setenv", "ANTHROPIC_BASE_URL", &base_url])
        .status()
        .context("failed to run launchctl setenv")?;

    std::process::Command::new("launchctl")
        .args([
            "setenv",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
            "1",
        ])
        .status()
        .context("failed to run launchctl setenv")?;

    let status = std::process::Command::new("open")
        .args(["-a", "Claude"])
        .status()
        .context("failed to launch Claude.app")?;

    if !status.success() {
        anyhow::bail!("`open -a Claude` failed — is Claude.app installed in /Applications?");
    }

    eprintln!("open-interceptor: Claude.app launched with proxy at {base_url}");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let _ = std::process::Command::new("launchctl")
        .args(["unsetenv", "ANTHROPIC_BASE_URL"])
        .status();
    let _ = std::process::Command::new("launchctl")
        .args(["unsetenv", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"])
        .status();

    Ok(())
}

/// Walk `PATH` and return the first executable named `name`.
fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            // Check execute bit on Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&candidate)
                    && meta.permissions().mode() & 0o111 != 0
                {
                    return Some(candidate);
                }
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
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

    #[test]
    fn claude_subcommand_no_args() {
        let cli = Cli::try_parse_from(["open-interceptor", "claude"]).unwrap();
        match cli.command {
            Command::Claude { args } => assert!(args.is_empty()),
            other => panic!("expected Claude, got {other:?}"),
        }
    }

    #[test]
    fn claude_subcommand_passes_through_trailing_args() {
        let cli = Cli::try_parse_from([
            "open-interceptor",
            "claude",
            "--model",
            "claude-opus-4-7",
            "--",
            "foo",
        ])
        .unwrap();
        match cli.command {
            Command::Claude { args } => {
                assert_eq!(args, vec!["--model", "claude-opus-4-7", "--", "foo"]);
            }
            other => panic!("expected Claude, got {other:?}"),
        }
    }

    #[test]
    fn claude_subcommand_allows_hyphenated_values() {
        let cli =
            Cli::try_parse_from(["open-interceptor", "claude", "--debug", "--no-cache"]).unwrap();
        match cli.command {
            Command::Claude { args } => assert_eq!(args, vec!["--debug", "--no-cache"]),
            other => panic!("expected Claude, got {other:?}"),
        }
    }

    #[test]
    fn claude_app_subcommand_parses() {
        let cli = Cli::try_parse_from(["open-interceptor", "claude-app"]).unwrap();
        assert!(matches!(cli.command, Command::ClaudeApp));
    }
}

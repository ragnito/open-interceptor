//! launchd daemon management (Phase 4).
//!
//! Registers `open-interceptor` as a per-user background agent on macOS
//! via `launchd`. The daemon starts at login and restarts automatically
//! if it exits, so the proxy is always available without a terminal
//! session.
//!
//! Platform note: this is macOS-only. Linux systemd support can be
//! added later following the same trait/interface pattern.

use std::path::PathBuf;

use anyhow::Context;

const SERVICE_LABEL: &str = "com.open-interceptor";
const PLIST_FILENAME: &str = "com.open-interceptor.plist";
pub const PROXY_PORT: u16 = 3300;

/// Path to the launchd plist in the user's Library.
fn plist_path() -> PathBuf {
    dirs_plist()
}

fn dirs_plist() -> PathBuf {
    let home = dirs_home();
    home.join("Library")
        .join("LaunchAgents")
        .join(PLIST_FILENAME)
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Path to the config file the daemon should use.
fn daemon_config_path() -> PathBuf {
    let home = dirs_home();
    home.join(".config").join("open-interceptor").join("config.yaml")
}

/// Path to the log directory (macOS standard: ~/Library/Logs/).
fn daemon_log_dir() -> PathBuf {
    let home = dirs_home();
    home.join("Library").join("Logs").join("open-interceptor")
}

/// Returns `true` if the proxy is accepting connections on the local port.
/// Does NOT check launchd — only the TCP socket. Fast (2 s timeout).
pub fn probe() -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], PROXY_PORT)),
        std::time::Duration::from_secs(2),
    )
    .is_ok()
}

/// Returns `true` if the launchd plist is installed.
pub fn is_installed() -> bool {
    plist_path().exists()
}

/// Generate and write the launchd plist that defines the background
/// agent. Requires the absolute path to the `open-interceptor` binary
/// so launchd can invoke it.
pub fn install(binary_path: &str) -> anyhow::Result<()> {
    let plist_dir = dirs_home().join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&plist_dir).context("creating LaunchAgents directory")?;

    let config_path = daemon_config_path();
    if !config_path.exists() {
        anyhow::bail!(
            "config not found at {}. Create it first:\n  cp config.yaml.example {}",
            config_path.display(),
            config_path.display()
        );
    }

    let log_dir = daemon_log_dir();
    std::fs::create_dir_all(&log_dir).context("creating log directory")?;

    let stdout_log = log_dir.join("stdout.log");
    let stderr_log = log_dir.join("stderr.log");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>run</string>
        <string>--config</string>
        <string>{config}</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>{stdout_log}</string>

    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>

    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>open_interceptor=info</string>
    </dict>

    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
        label = SERVICE_LABEL,
        binary = binary_path,
        config = config_path.display(),
        stdout_log = stdout_log.display(),
        stderr_log = stderr_log.display(),
    );

    let path = plist_path();
    std::fs::write(&path, plist).context("writing plist")?;

    eprintln!(
        "plist written to {}\nconfig: {}\nlogs: {}",
        path.display(),
        config_path.display(),
        log_dir.display()
    );
    Ok(())
}

/// Load the launchd agent and start it running.
pub fn start() -> anyhow::Result<()> {
    let path = plist_path();
    if !path.exists() {
        anyhow::bail!(
            "plist not found at {}. Run `open-interceptor start --install` first.",
            path.display()
        );
    }

    // bootstrap (macOS 10.10+) is preferred over load
    let output = std::process::Command::new("launchctl")
        .args(["bootstrap", "gui/501", path.to_str().unwrap()])
        .output()
        .context("running launchctl bootstrap")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already bootstrapped") || stderr.contains("service already loaded") {
            eprintln!("service is already running");
            return Ok(());
        }
        anyhow::bail!("launchctl bootstrap failed: {}", stderr.trim());
    }

    eprintln!("open-interceptor started as launchd agent");
    Ok(())
}

/// Unload and stop the launchd agent.
pub fn stop() -> anyhow::Result<()> {
    // bootout first, then remove the plist
    let output = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/501/{SERVICE_LABEL}")])
        .output()
        .context("running launchctl bootout")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // "Could not find domain for" means it's not running — that's fine.
        if !stderr.contains("Could not find") {
            eprintln!("launchctl bootout note: {}", stderr.trim());
        }
    }

    eprintln!("open-interceptor stopped");
    Ok(())
}

/// Check whether the daemon is running and on which port.
pub fn status() -> anyhow::Result<()> {
    let path = plist_path();
    if !path.exists() {
        println!("not installed (no plist at {})", path.display());
        return Ok(());
    }

    // Ask launchctl about our service.
    let output = std::process::Command::new("launchctl")
        .args(["print", &format!("gui/501/{SERVICE_LABEL}")])
        .output()
        .context("running launchctl print")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() || stdout.contains("Could not find") {
        println!("installed but not running");
        return Ok(());
    }

    match probe() {
        true => println!("running: http://127.0.0.1:{PROXY_PORT}"),
        false => println!("process running but port {PROXY_PORT} not reachable yet"),
    }

    Ok(())
}

/// Remove the plist. The service must be stopped first (no-op if already
/// stopped).
#[allow(dead_code)]
pub fn uninstall() -> anyhow::Result<()> {
    let path = plist_path();
    if path.exists() {
        std::fs::remove_file(&path).context("removing plist")?;
        eprintln!("uninstalled: plist removed");
    } else {
        eprintln!("nothing to uninstall (plist not found)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_label_is_valid() {
        assert!(!SERVICE_LABEL.is_empty());
        assert!(SERVICE_LABEL.contains('.'));
    }

    #[test]
    fn plist_path_in_user_library() {
        let p = plist_path();
        assert!(p.ends_with(PLIST_FILENAME));
        assert!(p.to_string_lossy().contains("LaunchAgents"));
    }
}

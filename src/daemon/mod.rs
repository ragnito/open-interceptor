//! Cross-platform daemon management.
//!
//! Registers `open-interceptor` as a per-user background service that starts
//! at login and restarts automatically if it exits, so the proxy is always
//! available without a terminal session.
//!
//! Platform backends:
//!   - macOS → `launchd` (LaunchAgent plist).  See [`launchd`].
//!   - Linux → `systemd` user service (`systemctl --user`).  See [`systemd`].
//!
//! Other platforms compile against an [`unsupported`] stub that returns a
//! clear error at runtime, so `cargo check` stays green everywhere.
//!
//! The public surface (`install` / `start` / `stop` / `status` / `probe` /
//! `is_installed` / `uninstall`) is identical across platforms; each backend
//! implements the same functions and this module delegates to the active one.

use std::path::PathBuf;

/// Local port the proxy listens on.
pub const PROXY_PORT: u16 = 3300;

/// Reverse-DNS service identifier. Used as the launchd label on macOS. The
/// systemd backend names its unit `open-interceptor.service` instead, so this
/// is unused on non-macOS targets.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SERVICE_NAME: &str = "com.open-interceptor";

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "macos")]
use launchd as platform;

#[cfg(target_os = "linux")]
mod systemd;
#[cfg(target_os = "linux")]
use systemd as platform;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use unsupported as platform;

// ---- shared helpers (used by the platform backends and the CLI) ----------

/// The user's home directory, falling back to `.` if `$HOME` is unset.
pub(crate) fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Path to the config file the daemon should use.
pub(crate) fn config_path() -> PathBuf {
    home()
        .join(".config")
        .join("open-interceptor")
        .join("config.yaml")
}

/// Directory where the daemon writes its rolling log files. Platform-aware:
///   - macOS: `~/Library/Logs/open-interceptor` (standard user-daemon log dir)
///   - Linux: `$XDG_STATE_HOME/open-interceptor`, or `~/.local/state/open-interceptor`
///
/// Used both by the tracing file appender (in `main`) and by `logs`.
pub fn log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home().join("Library").join("Logs").join("open-interceptor")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local").join("state"));
        base.join("open-interceptor")
    }
}

/// Returns `true` if the proxy is accepting connections on the local port.
/// Does NOT check the service manager — only the TCP socket. Fast (2 s timeout).
pub fn probe() -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], PROXY_PORT)),
        std::time::Duration::from_secs(2),
    )
    .is_ok()
}

// ---- delegating public API ------------------------------------------------

/// Returns `true` if the service definition (plist / unit file) is installed.
pub fn is_installed() -> bool {
    platform::is_installed()
}

/// Generate and write the service definition that defines the background
/// service. Requires the absolute path to the `open-interceptor` binary so
/// the service manager can invoke it.
pub fn install(binary_path: &str) -> anyhow::Result<()> {
    platform::install(binary_path)
}

/// Load the service and start it running.
pub fn start() -> anyhow::Result<()> {
    platform::start()
}

/// Stop the service.
pub fn stop() -> anyhow::Result<()> {
    platform::stop()
}

/// Print whether the daemon is running and on which port.
pub fn status() -> anyhow::Result<()> {
    platform::status()
}

/// Remove the service definition. The service should be stopped first.
#[allow(dead_code)]
pub fn uninstall() -> anyhow::Result<()> {
    platform::uninstall()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_valid() {
        assert!(!SERVICE_NAME.is_empty());
        assert!(SERVICE_NAME.contains('.'));
    }

    #[test]
    fn log_dir_under_home() {
        let dir = log_dir();
        assert!(dir.to_string_lossy().contains("open-interceptor"));
    }

    #[test]
    fn config_path_points_at_yaml() {
        let p = config_path();
        assert!(p.ends_with("config.yaml"));
        assert!(p.to_string_lossy().contains("open-interceptor"));
    }
}

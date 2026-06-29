//! Linux systemd backend.
//!
//! Registers `open-interceptor` as a per-user systemd service
//! (`systemctl --user`). The service starts at login and restarts
//! automatically if it exits. `loginctl enable-linger` is attempted so the
//! service also runs at boot without an interactive session — mirroring the
//! macOS LaunchAgent `RunAtLoad` + `KeepAlive` behaviour.
//!
//! Logs: systemd captures stdout/stderr into the journal
//! (`journalctl --user -u open-interceptor`), and the tracing file appender
//! also writes rolling logs under [`super::log_dir`]. No `/dev/null`
//! redirection is needed — journald rotates on its own.

use std::path::PathBuf;

use anyhow::Context;

use super::{config_path, home, log_dir};

const UNIT_FILENAME: &str = "open-interceptor.service";

/// Path to the systemd user unit file.
fn unit_path() -> PathBuf {
    home()
        .join(".config")
        .join("systemd")
        .join("user")
        .join(UNIT_FILENAME)
}

pub fn is_installed() -> bool {
    unit_path().exists()
}

pub fn install(binary_path: &str) -> anyhow::Result<()> {
    let unit_dir = home().join(".config").join("systemd").join("user");
    std::fs::create_dir_all(&unit_dir).context("creating systemd user unit directory")?;

    let config = config_path();
    if !config.exists() {
        anyhow::bail!(
            "config not found at {}. Create it first:\n  cp config.yaml.example {}",
            config.display(),
            config.display()
        );
    }

    let logs = log_dir();
    std::fs::create_dir_all(&logs).context("creating log directory")?;

    let unit = format!(
        r#"[Unit]
Description=open-interceptor local proxy for Claude Code
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={binary} run --config {config}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"#,
        binary = binary_path,
        config = config.display(),
    );

    let path = unit_path();
    std::fs::write(&path, unit).context("writing systemd unit")?;

    // Pick up the new unit file.
    run_systemctl(&["daemon-reload"]).context("systemctl --user daemon-reload")?;

    eprintln!(
        "unit written to {}\nconfig: {}\nlogs: {}",
        path.display(),
        config.display(),
        logs.display()
    );

    // Best-effort: let the service run at boot without an active login session.
    // This usually succeeds without sudo via polkit; warn (don't fail) if not.
    match std::process::Command::new("loginctl")
        .args(["enable-linger"])
        .output()
    {
        Ok(out) if out.status.success() => {}
        Ok(out) => eprintln!(
            "note: `loginctl enable-linger` did not succeed ({}). \
             The daemon will still run while you are logged in.",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => eprintln!(
            "note: could not run `loginctl enable-linger` ({e}). \
             The daemon will still run while you are logged in."
        ),
    }

    Ok(())
}

pub fn start() -> anyhow::Result<()> {
    let path = unit_path();
    if !path.exists() {
        anyhow::bail!(
            "unit not found at {}. Run `open-interceptor start --install` first.",
            path.display()
        );
    }

    // `enable --now` both enables at boot and starts immediately.
    run_systemctl(&["enable", "--now", UNIT_FILENAME]).context("systemctl --user enable --now")?;

    eprintln!("open-interceptor started as systemd user service");
    Ok(())
}

pub fn stop() -> anyhow::Result<()> {
    // `disable --now` stops the service and removes the boot symlink. We
    // ignore a non-zero exit (e.g. service not loaded) — stopping something
    // already stopped is not an error for our CLI.
    let output = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", UNIT_FILENAME])
        .output()
        .context("running systemctl --user disable --now")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("systemctl note: {}", stderr.trim());
        }
    }

    eprintln!("open-interceptor stopped");
    Ok(())
}

pub fn status() -> anyhow::Result<()> {
    let path = unit_path();
    if !path.exists() {
        println!("not installed (no unit at {})", path.display());
        return Ok(());
    }

    let output = std::process::Command::new("systemctl")
        .args(["--user", "is-active", UNIT_FILENAME])
        .output()
        .context("running systemctl --user is-active")?;

    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if state != "active" {
        println!("installed but not running ({state})");
        return Ok(());
    }

    match super::probe() {
        true => println!("running: http://127.0.0.1:{}", super::PROXY_PORT),
        false => println!(
            "process running but port {} not reachable yet",
            super::PROXY_PORT
        ),
    }

    Ok(())
}

#[allow(dead_code)]
pub fn uninstall() -> anyhow::Result<()> {
    let path = unit_path();
    if path.exists() {
        // Make sure it's stopped/disabled before removing the unit file.
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", UNIT_FILENAME])
            .output();
        std::fs::remove_file(&path).context("removing systemd unit")?;
        let _ = run_systemctl(&["daemon-reload"]);
        eprintln!("uninstalled: unit removed");
    } else {
        eprintln!("nothing to uninstall (unit not found)");
    }
    Ok(())
}

/// Run `systemctl --user <args>` and fail with the captured stderr on a
/// non-zero exit.
fn run_systemctl(args: &[&str]) -> anyhow::Result<()> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    let output = std::process::Command::new("systemctl")
        .args(&full)
        .output()
        .with_context(|| format!("running systemctl {}", full.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl {} failed: {}",
            full.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_path_in_user_systemd_dir() {
        let p = unit_path();
        assert!(p.ends_with(UNIT_FILENAME));
        assert!(p.to_string_lossy().contains("systemd/user"));
    }
}

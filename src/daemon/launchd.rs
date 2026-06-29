//! macOS launchd backend.
//!
//! Registers `open-interceptor` as a per-user LaunchAgent. The agent starts
//! at login and restarts automatically if it exits.

use std::path::PathBuf;

use anyhow::Context;

use super::{SERVICE_NAME, config_path, home, log_dir};

const PLIST_FILENAME: &str = "com.open-interceptor.plist";

/// Path to the launchd plist in the user's Library.
fn plist_path() -> PathBuf {
    home()
        .join("Library")
        .join("LaunchAgents")
        .join(PLIST_FILENAME)
}

/// The current user's numeric UID, used to address the launchd GUI domain
/// (`gui/<uid>`). launchd domains are per-user, so a hardcoded UID only works
/// for one account — we resolve it at runtime via `id -u`.
fn current_uid() -> anyhow::Result<String> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("running `id -u` to resolve the launchd domain")?;
    if !output.status.success() {
        anyhow::bail!(
            "`id -u` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn is_installed() -> bool {
    plist_path().exists()
}

pub fn install(binary_path: &str) -> anyhow::Result<()> {
    let plist_dir = home().join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&plist_dir).context("creating LaunchAgents directory")?;

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

    // stdout/stderr are redirected to /dev/null in the plist — launchd would
    // otherwise grow unbounded files in ~/Library/Logs. Structured logs go to
    // the tracing rolling file appender instead.
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
    <string>/dev/null</string>

    <key>StandardErrorPath</key>
    <string>/dev/null</string>

    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
        label = SERVICE_NAME,
        binary = binary_path,
        config = config.display(),
    );

    let path = plist_path();
    std::fs::write(&path, plist).context("writing plist")?;

    eprintln!(
        "plist written to {}\nconfig: {}\nlogs: {}",
        path.display(),
        config.display(),
        logs.display()
    );
    Ok(())
}

pub fn start() -> anyhow::Result<()> {
    let path = plist_path();
    if !path.exists() {
        anyhow::bail!(
            "plist not found at {}. Run `open-interceptor start --install` first.",
            path.display()
        );
    }

    let uid = current_uid()?;
    // bootstrap (macOS 10.10+) is preferred over the legacy `load`.
    let output = std::process::Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}"), path.to_str().unwrap()])
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

pub fn stop() -> anyhow::Result<()> {
    let uid = current_uid()?;
    let output = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{SERVICE_NAME}")])
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

pub fn status() -> anyhow::Result<()> {
    let path = plist_path();
    if !path.exists() {
        println!("not installed (no plist at {})", path.display());
        return Ok(());
    }

    let uid = current_uid()?;
    let output = std::process::Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{SERVICE_NAME}")])
        .output()
        .context("running launchctl print")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() || stdout.contains("Could not find") {
        println!("installed but not running");
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
    fn plist_path_in_user_library() {
        let p = plist_path();
        assert!(p.ends_with(PLIST_FILENAME));
        assert!(p.to_string_lossy().contains("LaunchAgents"));
    }
}

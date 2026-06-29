//! Fallback backend for platforms without a supported service manager.
//!
//! `open-interceptor run` still works everywhere (it is just a foreground
//! process); only the daemon lifecycle commands are unavailable. These stubs
//! keep `cargo check` green on such platforms and return a clear error at
//! runtime.

const MSG: &str = "daemon management is only supported on macOS (launchd) and Linux (systemd). \
                   Use `open-interceptor run` to run the proxy in the foreground.";

pub fn is_installed() -> bool {
    false
}

pub fn install(_binary_path: &str) -> anyhow::Result<()> {
    anyhow::bail!(MSG)
}

pub fn start() -> anyhow::Result<()> {
    anyhow::bail!(MSG)
}

pub fn stop() -> anyhow::Result<()> {
    anyhow::bail!(MSG)
}

pub fn status() -> anyhow::Result<()> {
    anyhow::bail!(MSG)
}

#[allow(dead_code)]
pub fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!(MSG)
}

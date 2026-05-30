//! open-interceptor menu-bar app.
//!
//! A native macOS status-bar item that shows proxy health and lets the
//! user start/stop the launchd daemon without opening a terminal.
//!
//! Build: cargo build --features menubar --bin open-interceptor-menubar
//! Bundle: tools/bundle-app.sh
//!
//! Design decisions:
//! - Plain fn main() — tao's event loop MUST own the macOS main thread
//!   (AppKit requirement). No tokio here; the headless proxy binary is a
//!   separate process.
//! - All daemon operations (launchctl) are dispatched to background
//!   threads so they never block the event loop.
//! - The app quits cleanly; the daemon keeps running (launchd-managed).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use open_interceptor::daemon;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

// Poll the proxy port every 5 seconds.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    let event_loop = EventLoop::new();

    // Load icons (embedded at compile time — paths relative to CARGO_MANIFEST_DIR).
    let icon_idle = load_icon(include_bytes!("../../assets/menubar-template.png"));
    let icon_active = load_icon(include_bytes!("../../assets/menubar-active.png"));

    // Build menu.
    let menu = Menu::new();

    let item_status = MenuItem::new("● Checking…", false, None);
    let item_start = MenuItem::new("Start proxy", true, None);
    let item_stop = MenuItem::new("Stop proxy", true, None);
    let item_logs = MenuItem::new("View Logs", true, None);
    let item_config = MenuItem::new("Open Config", true, None);
    let item_quit = MenuItem::new("Quit", true, None);

    let _ = menu.append(&item_status);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&item_start);
    let _ = menu.append(&item_stop);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&item_logs);
    let _ = menu.append(&item_config);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&item_quit);

    // Build tray icon.
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon_idle.clone())
        .with_tooltip("open-interceptor")
        .build()
        .expect("failed to create tray icon");

    let start_id = item_start.id().clone();
    let stop_id = item_stop.id().clone();
    let logs_id = item_logs.id().clone();
    let config_id = item_config.id().clone();
    let quit_id = item_quit.id().clone();

    // Shared status flag updated by the poll tick.
    let is_running = Arc::new(Mutex::new(false));
    let is_running_evt = is_running.clone();

    // Last update label — avoid redundant setTitle calls.
    let last_label: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let last_label_evt = last_label.clone();

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    // We poll on a WaitUntil timer.
    let mut next_poll = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        // Default: wait until next poll, interrupted by events.
        *control_flow = ControlFlow::WaitUntil(next_poll);

        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. })
            | Event::NewEvents(StartCause::Init) => {
                // Time to probe.
                let running = daemon::probe();
                *is_running.lock().unwrap() = running;

                let label = if running {
                    "● Running — http://127.0.0.1:3300".to_string()
                } else {
                    "○ Stopped".to_string()
                };

                // Only update the menu item when the label changes.
                let mut last = last_label.lock().unwrap();
                if *last != label {
                    item_status.set_text(&label);
                    *last = label;
                    // Swap icon.
                    let icon = if running {
                        icon_active.clone()
                    } else {
                        icon_idle.clone()
                    };
                    tray.set_icon(Some(icon)).ok();
                }

                next_poll = Instant::now() + POLL_INTERVAL;
                *control_flow = ControlFlow::WaitUntil(next_poll);
            }

            Event::LoopDestroyed => {}
            _ => {}
        }

        // Drain tray events (not used for actions, just keep the channel clear).
        if let Ok(_evt) = tray_channel.try_recv() {}

        // Handle menu clicks.
        if let Ok(evt) = menu_channel.try_recv() {
            let id = &evt.id;

            if id == &quit_id {
                *control_flow = ControlFlow::Exit;
            } else if id == &start_id {
                let running = *is_running_evt.lock().unwrap();
                if running {
                    item_status.set_text("Already running");
                    return;
                }
                item_status.set_text("Starting…");
                let cli = resolve_cli_binary();
                std::thread::spawn(move || {
                    if !daemon::is_installed() {
                        if let Err(e) = daemon::install(&cli) {
                            eprintln!("open-interceptor-menubar: install failed: {e}");
                            return;
                        }
                    }
                    if let Err(e) = daemon::start() {
                        eprintln!("open-interceptor-menubar: start failed: {e}");
                    }
                });
                // Next poll will refresh the label.
                next_poll = Instant::now() + Duration::from_secs(2);
                *control_flow = ControlFlow::WaitUntil(next_poll);
            } else if id == &stop_id {
                item_status.set_text("Stopping…");
                std::thread::spawn(|| {
                    if let Err(e) = daemon::stop() {
                        eprintln!("open-interceptor-menubar: stop failed: {e}");
                    }
                });
                next_poll = Instant::now() + Duration::from_secs(2);
                *control_flow = ControlFlow::WaitUntil(next_poll);
            } else if id == &logs_id {
                let log_dir = home_dir()
                    .join("Library")
                    .join("Logs")
                    .join("open-interceptor");
                // tracing-appender writes open-interceptor.YYYY-MM-DD.log;
                // launchd also redirects stderr to stderr.log. Pick the
                // most recently modified file in the directory.
                let best = most_recent_log(&log_dir);
                match best {
                    Some(path) => {
                        // -t forces TextEdit — always shows the content,
                        // unlike Console.app which may ignore plain files.
                        let _ = std::process::Command::new("open")
                            .args(["-t", path.to_str().unwrap_or("")])
                            .spawn();
                    }
                    None => {
                        item_status.set_text("No logs yet");
                        *last_label.lock().unwrap() = String::new(); // reset so next poll refreshes it
                    }
                }
            } else if id == &config_id {
                let cfg = home_dir()
                    .join(".config")
                    .join("open-interceptor")
                    .join("config.yaml");
                let _ = std::process::Command::new("open").arg(&cfg).spawn();
            }

            // Suppress unused-variable warnings for label tracking.
            let _ = &last_label_evt;
        }
    });
}

/// Return the most recently modified `.log` file in `dir`, or `None` if
/// the directory is missing or empty.
fn most_recent_log(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
            if best.as_ref().map_or(true, |(t, _)| mtime > *t) {
                best = Some((mtime, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Locate the `open-interceptor` CLI binary to pass to `daemon::install`.
/// In the bundled .app it sits next to us in Contents/MacOS/.
/// Falls back to searching PATH.
fn resolve_cli_binary() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe
            .parent()
            .map(|d| d.join("open-interceptor"))
            .unwrap_or_default();
        if sibling.is_file() {
            return sibling.to_string_lossy().into_owned();
        }
    }
    // Fallback: hope it's on PATH (works when running from Cargo directly).
    "open-interceptor".to_string()
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Decode a PNG byte slice into a `tray_icon::Icon`.
fn load_icon(bytes: &[u8]) -> Icon {
    let img = image_from_png(bytes);
    Icon::from_rgba(img.0, img.1, img.2).expect("valid icon")
}

/// Minimal PNG decoder using the `tao` / `tray-icon` bundled image support.
/// Returns (rgba_bytes, width, height).
fn image_from_png(bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    // tray-icon re-exports png decoding via its image dep.
    // We decode manually using the png crate that tray-icon pulls in.
    use std::io::Cursor;
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("PNG frame");
    let w = info.width;
    let h = info.height;
    // Convert to RGBA if needed.
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf[..info.buffer_size()].chunks(3) {
                out.extend_from_slice(chunk);
                out.push(255);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf[..info.buffer_size()].chunks(2) {
                let v = chunk[0];
                out.extend_from_slice(&[v, v, v, chunk[1]]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &v in &buf[..info.buffer_size()] {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        _ => panic!("unsupported PNG color type"),
    };
    (rgba, w, h)
}

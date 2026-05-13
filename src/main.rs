//! `open-interceptor` — local proxy that auto-routes Claude Code traffic
//! to the right upstream based on the `model` field. See `CLAUDE.md` and
//! `TODO.md` for context.

mod config;
mod providers;
mod translate;

fn main() {
    // Real entrypoint comes in T1.2/T1.3 (clap dispatch + tracing init).
    println!("open-interceptor stub — run via `cargo run -- run --config ...` (Phase 1 WIP)");
}

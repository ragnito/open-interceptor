# open-interceptor

Local proxy that sits between Claude Code and remote providers (Anthropic, DeepSeek, OpenRouter, OpenAI-compatible, etc.). Reads the `model` field from each request and **auto-routes** to the right provider — including format translation when needed (Anthropic Messages ↔ OpenAI Chat Completions).

Set `ANTHROPIC_BASE_URL=http://127.0.0.1:3300` once, then switch providers from inside Claude Code with `/model <name>` — no reload, no wrapper scripts.

## Install

### Quick install (macOS & Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/ragnito/open-interceptor/master/install.sh | sh
```

Downloads the prebuilt binary for your platform (macOS arm64/x86_64, Linux x86_64/aarch64), verifies its checksum, and installs it to `~/.local/bin`. The installer prints the next steps.

Pin a version or change the install dir:

```bash
OPEN_INTERCEPTOR_VERSION=v0.1.0 OPEN_INTERCEPTOR_BIN_DIR=/usr/local/bin \
  sh -c "$(curl -fsSL https://raw.githubusercontent.com/ragnito/open-interceptor/master/install.sh)"
```

### Homebrew (macOS)

```bash
brew tap ragnito/tap
brew install open-interceptor
```

### After installing

```bash
# 1. Create your config
mkdir -p ~/.config/open-interceptor
cp config.yaml.example ~/.config/open-interceptor/config.yaml
# edit it with your providers / API keys

# 2. Start the background daemon (launchd on macOS, systemd user service on Linux)
open-interceptor start --install
open-interceptor status

# 3. Point Claude Code at the proxy — add to ~/.zshrc / ~/.bashrc
export ANTHROPIC_BASE_URL=http://127.0.0.1:3300
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1
```

### Build from source

Requires Rust stable (1.85+).

```bash
git clone https://github.com/ragnito/open-interceptor
cd open-interceptor
cargo build --release
sudo cp target/release/open-interceptor /usr/local/bin/
open-interceptor start --install --binary /usr/local/bin/open-interceptor
```

## Usage

### The easy way: `open-interceptor claude`

```bash
open-interceptor claude
```

That's it. The command:
1. Checks if the proxy is running on port 3300.
2. If not, installs the service definition (first time only) and starts the daemon automatically.
3. Injects `ANTHROPIC_BASE_URL=http://127.0.0.1:3300` and `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`.
4. Replaces the process image with the real `claude` binary — TTY, signals, and exit code are preserved exactly.

Pass any Claude Code flags through verbatim:

```bash
open-interceptor claude --model claude-opus-4-7
open-interceptor claude --debug
```

No more exporting env vars manually. Works like `ollama run`.

### Manual (existing workflow)

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:3300
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1
claude
```

### CLI reference

```
open-interceptor claude [args...]         run claude with proxy auto-started
open-interceptor run --config <path>      foreground server
open-interceptor start --install          install + start the background daemon
open-interceptor start                    start daemon (already installed)
open-interceptor stop                     stop daemon
open-interceptor status                   check if daemon is running
open-interceptor logs --follow            tail live logs
open-interceptor logs                     last 20 log lines
open-interceptor config --config <path>   validate config
```

Inside Claude Code use `/model <name>` to switch providers. If `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` is set, `/model` (with no argument) opens a picker listing all models from all configured providers.

## What makes this different

There are already several similar projects ([claude-code-router](https://github.com/musistudio/claude-code-router), [anthropic-proxy-rs](https://github.com/m0n0x41d/anthropic-proxy-rs), [litellm](https://docs.litellm.ai/), etc.). The one feature `open-interceptor` offers that none of them do cleanly:

> **OAuth subscription pass-through.** When the request targets an Anthropic model, the proxy forwards your Claude Code OAuth token unchanged to `api.anthropic.com` — so you keep using your **Pro / Max subscription**, not API credits. When the request targets DeepSeek / OpenAI / OpenRouter / etc., the proxy substitutes the configured API key for that provider.

## Architecture

```
Claude Code
  │  ANTHROPIC_BASE_URL=http://127.0.0.1:3300
  ▼
open-interceptor (localhost:3300, background daemon)
  ├─ reads `model` field from request body
  ├─ matches against configured route patterns (glob)
  └─ dispatches to provider:
       ├─ Anthropic-compatible → passthrough
       ├─ OpenAI-compatible    → translation layer
       └─ Custom passthrough   → forward client auth
```

## Why Rust

- Streaming SSE chunk-by-chunk with minimal buffering
- No GC pauses during sustained streaming responses
- ~5–10 MB RAM footprint as a long-running daemon
- Single binary distribution, no runtime required

## Risk: Anthropic Terms of Service

Anthropic's policy (updated February 2026) prohibits the use of **OAuth tokens from Free / Pro / Max subscriptions** in any product, tool, or service other than the official Claude Code CLI and Claude.ai. The first enforcement case was a personal usage-tracker app — non-commercial use does **not** automatically exempt you.

`open-interceptor` is designed to run **locally on your own machine, for your own requests only**. Under that constraint, the proxy is effectively invisible to Anthropic (same User-Agent, same headers, same TLS endpoint) and is in the same defensible gray zone as any local debugging tool that observes your traffic. But that's a gray zone, not a green light.

**Do not** use this proxy to:

- Share your subscription token with other users
- Route requests through it from machines that aren't yours
- Build a commercial product on top of it that uses subscription auth

If you do any of the above, switch the `anthropic` provider in your config from `passthrough_auth: true` to a real API key. Anthropic publishes API credits for exactly this case.

Use of this software is **at your own risk**. The maintainers do not encourage or condone violation of any provider's terms of service.

## Documentation

- [TODO.md](./TODO.md) — phased implementation roadmap
- [CLAUDE.md](./CLAUDE.md) — entry point for Claude Code sessions in this repo
- [config.yaml.example](./config.yaml.example) — example configuration

## License

MIT — see [LICENSE](./LICENSE).

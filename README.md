# open-interceptor

Local proxy that sits between Claude Code and remote providers (Anthropic, DeepSeek, OpenRouter, OpenAI-compatible, etc.). Reads the `model` field from each request and **auto-routes** to the right provider — including format translation when needed (Anthropic Messages ↔ OpenAI Chat Completions).

Set `ANTHROPIC_BASE_URL=http://127.0.0.1:3300` once, then switch providers from inside Claude Code with `/model <name>` — no reload, no wrapper scripts.

## Status

**Alpha — under active development.** See [TODO.md](./TODO.md) for the implementation roadmap.

## What makes this different

There are already several similar projects ([claude-code-router](https://github.com/musistudio/claude-code-router), [anthropic-proxy-rs](https://github.com/m0n0x41d/anthropic-proxy-rs), [litellm](https://docs.litellm.ai/), etc.). The one feature `open-interceptor` is being built to support that **none of them offer cleanly today** is:

> **OAuth subscription pass-through.** When the request targets an Anthropic model, the proxy forwards your Claude Code OAuth token unchanged to `api.anthropic.com` — so you keep using your **Pro / Max subscription**, not API credits. When the request targets DeepSeek / OpenAI / OpenRouter / etc., the proxy substitutes the configured API key for that provider.

This means a single `/model` switch inside Claude Code can move you between your subscription and pay-per-token providers without re-authenticating or restarting.

## ⚠️ Risk: Anthropic Terms of Service

Anthropic's policy (updated February 2026) prohibits the use of **OAuth tokens from Free / Pro / Max subscriptions** in any product, tool, or service other than the official Claude Code CLI and Claude.ai. The first enforcement case was a personal usage-tracker app — non-commercial use does **not** automatically exempt you.

`open-interceptor` is designed to run **locally on your own machine, for your own requests only**. Under that constraint, the proxy is effectively invisible to Anthropic (same User-Agent, same headers, same TLS endpoint) and is in the same defensible gray zone as any local debugging tool that observes your traffic. But that's a gray zone, not a green light.

**Do not** use this proxy to:

- Share your subscription token with other users
- Route requests through it from machines that aren't yours
- Build a commercial product on top of it that uses subscription auth

If you do any of the above, switch the `anthropic` provider in your config from `passthrough_auth: true` to a real API key. Anthropic publishes API credits for exactly this case.

Use of this software is **at your own risk**. The maintainers do not encourage or condone violation of any provider's terms of service.

## Why Rust

- Streaming SSE chunk-by-chunk with minimal buffering
- No GC pauses during sustained streaming responses
- ~5–10 MB RAM footprint as a long-running daemon
- Single binary distribution, no runtime required

## Architecture

```
Claude Code
  │  ANTHROPIC_BASE_URL=http://127.0.0.1:3300
  ▼
open-interceptor (localhost:3300, launchd daemon)
  ├─ reads `model` field from request body
  ├─ matches against configured route patterns (glob)
  └─ dispatches to provider:
       ├─ Anthropic-compatible → passthrough
       ├─ OpenAI-compatible    → translation layer
       └─ Custom passthrough   → forward client auth
```

## Quickstart (development)

Requires Rust stable (1.75+).

```bash
cp config.yaml.example ~/.open-interceptor/config.yaml
# edit with your API keys
cargo run --release -- run --config ~/.open-interceptor/config.yaml
```

In another terminal:

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:3300
claude
```

Then inside Claude Code use `/model <name>` to switch providers.

## Installation (planned, post v1.0)

```bash
brew install <tap>/open-interceptor
open-interceptor start
```

## Documentation

- [TODO.md](./TODO.md) — phased implementation roadmap
- [CLAUDE.md](./CLAUDE.md) — entry point for Claude Code sessions in this repo
- [config.yaml.example](./config.yaml.example) — example configuration

## License

MIT — see [LICENSE](./LICENSE).

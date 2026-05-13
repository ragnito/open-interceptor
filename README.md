# open-interceptor

Local proxy that sits between Claude Code and remote providers (Anthropic, DeepSeek, OpenRouter, OpenAI-compatible, etc.). Reads the `model` field from each request and **auto-routes** to the right provider — including format translation when needed (Anthropic Messages ↔ OpenAI Chat Completions).

Set `ANTHROPIC_BASE_URL=http://127.0.0.1:3300` once, then switch providers from inside Claude Code with `/model <name>` — no reload, no wrapper scripts.

## Status

**Alpha — under active development.** See [TODO.md](./TODO.md) for the implementation roadmap.

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

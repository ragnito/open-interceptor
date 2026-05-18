# CLAUDE.md — open-interceptor

Este archivo es el punto de entrada para Claude Code cuando trabaje en este repo. Léelo antes de hacer cualquier cambio.

## Qué es este proyecto

`open-interceptor` es un proxy local (Rust + Axum) que se sitúa entre Claude Code y los providers remotos (Anthropic, DeepSeek, OpenRouter, OpenAI-compatible, etc.). Lee el campo `model` de cada request y rutea automáticamente al provider correcto, traduciendo formatos cuando es necesario (Anthropic Messages ↔ OpenAI Chat Completions).

El usuario configura `ANTHROPIC_BASE_URL=http://127.0.0.1:3300` una vez y cambia de proveedor con `/model <nombre>` dentro de Claude Code, sin recargar nada.

## Documentos clave

- **[Plan técnico completo](/Users/usuario/.claude/plans/tengo-el-siguiente-plan-logical-wall.md)** — arquitectura, decisiones, estructura de archivos, fases
- **[TODO.md](./TODO.md)** — lista trazable de tareas por fase. Marca tareas conforme las completes y commitea junto al cambio.
- **[config.yaml.example](./config.yaml.example)** — ejemplo de configuración con 5 providers

## Stack

- **HTTP server**: Axum 0.8 + Hyper 1
- **HTTP client**: Reqwest 0.12 (con rustls-tls + stream)
- **Runtime**: Tokio multi-thread (solo en el bin principal; el menubar app usa AppKit event loop)
- **CLI**: clap v4 derive
- **Logging**: tracing + tracing-subscriber + tracing-appender
- **Config**: serde_yml + shellexpand para `${ENV_VAR}`
- **Pattern matching modelos**: globset (mismo crate que ripgrep)
- **SSE parsing**: eventsource-stream
- **Menu bar app** (feature `menubar`): tray-icon + tao + muda + png (deps opcionales; NO compilados en el build default)

## Binarios

| Binary | Feature | Descripción |
|--------|---------|-------------|
| `open-interceptor` | (default) | Proxy headless + CLI (run/start/stop/status/logs/claude) |
| `open-interceptor-menubar` | `menubar` | App nativa macOS para la barra de menú |

### Construir la app de menú

```bash
tools/bundle-app.sh   # produce "Open Interceptor.app" (ad-hoc signed)
```

El bundle incluye ambos binarios en `Contents/MacOS/`. El menubar app resuelve el CLI hermano para `daemon::install`.

## Reglas de trabajo en este repo

1. **Una tarea de TODO.md por commit** (o grupos pequeños relacionados). El mensaje de commit referencia el ID: `[T1.4] add router with pre-compiled globset`.
2. **Marca el TODO con `[x]`** en el mismo commit que cierra la tarea.
3. **Snapshot tests obligatorios** para cualquier cambio en `src/translate/` — usa `insta` con fixtures reales capturados de Claude Code.
4. **No introducir runtimes alternativos**: solo Tokio. No usar `async-std`, `smol`, etc.
5. **Errores con `thiserror` para tipos de la lib y `anyhow` solo en `main.rs`/`cli.rs`**.
6. **No bloquear el reactor**: nada de `std::fs`, `std::thread::sleep`, ni operaciones síncronas costosas dentro de handlers async. Usa `tokio::fs`, `tokio::time::sleep`.
7. **Streaming siempre pass-through chunk-a-chunk**: nunca bufferizar la response completa antes de emitir al cliente (rompería el punto de hacerlo en Rust).
8. **Logs estructurados**: usar `tracing::info!(model = %m, provider = %p, "...")`, no `println!` ni `format!` ad-hoc.

## Cómo arrancar localmente durante desarrollo

```bash
cargo run -- run --config config.yaml.example
```

En otra terminal:
```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:3300
claude
```

## Cómo verificar antes de commit

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Fases del proyecto

Ver `TODO.md`. Estamos siguiendo este orden estricto:

1. **Phase 0** — setup
2. **Phase 1** — proxy core + Anthropic-compatible (tag `v0.1.0-phase1`)
3. **Phase 2** — `/v1/models` spoofing (tag `v0.2.0-phase2`)
4. **Phase 3** — translation OpenAI ↔ Anthropic (tag `v0.3.0-phase3`)
5. **Phase 4** — daemon launchd + CLI + Homebrew (tag `v1.0.0`)
6. **Phase 5** — hardening post-MVP

No saltar de fase hasta que todas las tareas de la anterior estén `[x]` y exista el tag git correspondiente.

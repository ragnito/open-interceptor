# open-interceptor — TODO

Estado: `[ ]` pendiente · `[~]` en progreso · `[x]` hecho

Marca cada tarea al completarla y commitea junto al cambio. El detalle técnico de cada item está en `/Users/usuario/.claude/plans/tengo-el-siguiente-plan-logical-wall.md`.

## Phase 0 — Setup del proyecto (1 día)

- [x] **T0.1** Crear este `TODO.md` en la raíz del repo
  - Lista trazable de tareas. Se actualiza commit a commit.
- [x] **T0.2** Crear `CLAUDE.md` en la raíz enlazando `TODO.md` y el plan
  - Punto de entrada para futuras sesiones de Claude Code en este repo.
- [x] **T0.3** `cargo init --name open-interceptor` y commit inicial
  - Base del crate binario.
- [x] **T0.4** Configurar `Cargo.toml` con dependencias core: `tokio` (full), `axum` 0.8, `hyper` 1, `reqwest` 0.12 (rustls-tls, stream, json), `serde` (derive), `serde_json`, `serde_yml` (en lugar de serde_yaml deprecated), `clap` v4 (derive), `tracing`, `tracing-subscriber` (env-filter), `tracing-appender`, `globset`, `shellexpand`, `anyhow`, `thiserror`, `eventsource-stream`, `futures`, `tokio-util` (rt)
  - `[profile.release]`: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `opt-level = 3`, `panic = "abort"` — binario stripped, optimizado para tamaño + velocidad.
- [x] **T0.5** Crear `.gitignore`, `README.md` mínimo, `LICENSE` (MIT)
- [x] **T0.6** Crear `config.yaml.example` con los 5 providers del plan
- [x] **T0.7** Crear directorios `src/providers/` y `src/translate/` con `mod.rs` vacíos

## Phase 1 — Core proxy + ruteo Anthropic-compatible (semana 1)

- [x] **T1.1** `src/config.rs`: definir structs `Config`, `Provider`, `ProviderType`, `Route`. Implementar `Config::load(path)` con expansión de `${ENV_VAR}` en cualquier campo string (incluido `api_key`).
  - Validación: `route.provider` debe referenciar un provider existente; `route.models` no puede estar vacío; ambos fallan al cargar.
  - 4 unit tests cubriendo: carga minimal, expansión de env vars, provider desconocido, models vacío.
- [x] **T1.2** `src/cli.rs`: subcomando `run --config <path>` funcional + stubs claros para `start`/`stop`/`status`/`logs`/`config-validate` (estos retornan "Phase 4 — not implemented yet"). Soporte de `~` con `shellexpand::tilde`. 3 unit tests (clap valid, run parsing, stubs error).
- [x] **T1.3** `src/main.rs`: `#[tokio::main]` multi-thread, init `tracing-subscriber` con `EnvFilter` (default: `open_interceptor=info,tower_http=warn,reqwest=warn,hyper=warn`, override con `RUST_LOG`). Dispatch a `cli::dispatch()` y print de error chain via `anyhow` Debug.
- [x] **T1.4** `src/router.rs`: `Router` con `GlobSetBuilder` pre-compilado en `build()`. `resolve(model)` retorna `Option<Resolution<'_>>` con `&Provider`, `provider_name` y `effective_model` (aplica `remap` si existe). 6 unit tests: match básico, primer-match-gana, remap, sin match, catch-all `*`, glob inválido rechazado al build.
- [x] **T1.5** `src/proxy.rs`: servidor Axum 0.8 con `tokio::net::TcpListener`. Handler `POST /v1/messages`: bufferiza body como `Bytes`, parsea solo el campo `model` (deja el resto opaco para forwardearlo intacto), llama `router.resolve()`, dispatch (stub 501 hasta T1.7). Mensaje de error claro si el puerto está ocupado.
- [x] **T1.6** Handler `POST /v1/messages/count_tokens` reusando el mismo `dispatch()`. Plus stub `GET /v1/models` que retorna la unión de los `models:` declarados en cada provider (versión final de T2.1-T2.4 ya esbozada).
- [x] **T1.7** `src/providers/anthropic.rs::forward()` con dos modos según `passthrough_auth`. Validado empíricamente: el mock upstream ve los headers byte-exact (Authorization Bearer completo, anthropic-version, anthropic-beta con `oauth-2025-04-20`, x-app, X-Claude-Code-Session-Id, los 7 X-Stainless-*, User-Agent), el path con `?beta=true` preservado, sin `Via`/`X-Forwarded-*` añadidos. Hop-by-hop dropeados (`Connection`, `Keep-Alive`, `Transfer-Encoding`, `TE`, `Trailer`, `Upgrade`, `Proxy-*`, `Host`, `Content-Length`). En modo API-key sustituye con `x-api-key` y droppea la auth del cliente. 9 unit tests cubriendo todas las ramas. Cliente reqwest compartido vía `OnceLock` con connection pooling.
- [x] **T1.8** Streaming SSE response con `axum::body::Body::from_stream(upstream.bytes_stream())`. Validado: 6 eventos SSE (`message_start`/`content_block_start`/`content_block_delta`/`content_block_stop`/`message_delta`/`message_stop`) llegan al cliente vía proxy idénticos a los del upstream, con `elapsed_ms=1` (el proxy responde al cliente desde que recibe el primer byte del upstream, sin esperar al `message_stop`). Headers de la response upstream relayed con la misma lista de hop-by-hop dropeados.
- [x] **T1.9** `sse_inject_usage()` — estima output_tokens via chars/4 cuando el upstream omite usage ✅
- [x] **T1.10** `strip_thinking_blocks()` — implementado en `anthropic::sanitize_body()` ✅
- [x] **T1.11** Error handling más fino — timeout → 504, conexión → 502, ambos providers ✅
- [ ] **T1.12** Logging — **suficiente para Phase 1**: cada dispatch loguea `model`, `effective_model`, `provider`, `provider_type`, `passthrough_auth`, `body_bytes`, `upstream_status`, `elapsed_ms`. Validado en producción.
- [x] **T1.13** ✅ E2E real contra `api.anthropic.com` con OAuth Pro/Max passthrough. 13 requests, todas con `upstream_status=200`, conversación multi-turno con tool use confirmada. Anthropic aceptó nuestros requests byte-idénticos.
- [x] **T1.14** ✅ E2E real contra `https://opencode.ai/zen/go/v1/messages` con MiniMax. Suscripción OpenCode Go funcionando vía nuestro proxy. (Replantea original con DeepSeek — el endpoint anthropic-compatible de DeepSeek directo queda para cuando alguien lo necesite, OpenCode Go ya cubre acceso a DeepSeek V4 Pro/Flash via Phase 3.)
- [~] **T1.15** OpenRouter — descartado por ahora (solo interesa Claude)
- [x] **T1.16** Tag `v0.1.0-phase1` en git ✅

## Phase 2 — Endpoint `/v1/models` (1-2 días)

- [x] **T2.1** `src/models_endpoint.rs` con handler Anthropic-shape `{ data: [{ id, type: "model", display_name }] }`
- [x] **T2.2** Campo `models: Vec<String>` por provider en `config.rs` (ya existía desde T1.1)
- [x] **T2.3** Dynamic fetch: si provider no tiene `models:`, fetchear `/v1/models` real (solo `anthropic_compatible`) y cachear con TTL 1h
- [x] **T2.4** Ruta registrada en `proxy.rs` via `AppState`
- [x] **T2.5** Test manual: `curl http://127.0.0.1:3300/v1/models | jq`, verificar que Claude Code lista los modelos en el picker
- [x] **T2.6** Documentado `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` en README
- [x] **T2.7** Tag `v0.2.0-phase2`
- [x] **T2.8** `context_window` y `max_output_tokens` por modelo en `config.yaml`. `ModelSpec` reemplaza `Vec<String>` en `Provider.models`. `ModelEntry` emite los campos en `/v1/models` (skip-if-none). Dynamic fetch parsea `context_window` del upstream. Claude Code muestra el budget correcto en `/context` por modelo.

## Phase 3 — Translation layer OpenAI ↔ Anthropic (semana 2)

- [x] **T3.1** `src/translate/types_anthropic.rs`: structs para MessagesRequest/Response, ContentBlock, SSE events
- [x] **T3.2** `src/translate/types_openai.rs`: structs para ChatCompletionRequest/Response, streaming chunks
- [x] **T3.3** `src/translate/req_anthropic_to_openai.rs`: conversión Anthropic→OpenAI (system, messages, tools, tool_choice, stop_sequences)
- [x] **T3.4** `src/translate/resp_openai_to_anthropic.rs::convert_non_streaming()`: OpenAI→Anthropic non-streaming
- [x] **T3.5** `src/translate/sse_stream.rs`: streaming SSE conversion con state machine (text blocks, tool call argument reassembly)
- [x] **T3.6** `tool_translation.rs` — no necesario: traducción de tools inline en los módulos req/resp
- [x] **T3.7** `src/providers/openai.rs`: pipeline completo non-streaming + streaming con SSE translator integrado
- [x] **T3.8** Snapshot tests con `insta`: 8 snapshots (req translation simple/complejo/tool calls, response text/tool/multiple tools, SSE text-only/text+tool). 68 tests total.
- [x] **T3.9** Cancelación SSE: el drop del Body de Axum se propaga al stream → bytes_stream → Response de reqwest, abortando upstream automáticamente. Documentado en `providers/openai.rs`.
- [x] **T3.10** Test manual: con OpenAI API key real, `/model gpt-4o` dentro de Claude Code ✅
- [x] **T3.11** Test manual con endpoint OpenAI-compatible alternativo (OpenCode Go / DeepSeek V4 Pro) ✅ non-streaming + streaming OK
- [x] **T3.12** Tag `v0.3.0-phase3`

## Phase 4 — Daemon + CLI completa + Homebrew (semana 3)

- [x] **T4.1** `src/daemon.rs::install()`: escribe plist en `~/Library/LaunchAgents/com.open-interceptor.plist`
- [x] **T4.2** `daemon::start()` con `launchctl bootstrap`, `stop()` con `launchctl bootout`, `status()` con `launchctl print` + check TCP
- [x] **T4.3** CLI completa: `start [--install] [--binary]`, `stop`, `status`, `logs [--follow]`, `config [--config]`
- [x] **T4.4** Rotación de logs con `tracing-appender` daily, retener últimos 7 días
- [x] **T4.5** Error claro si puerto ocupado (ya en `proxy.rs` desde T1.5)
- [x] **T4.6** Plist generado programáticamente en `daemon.rs` con placeholders rellenos
- [x] **T4.7** GitHub Actions `release.yml`: build arm64 + x64 macos, upload a GitHub Release
- [x] **T4.8** Crear repo `homebrew-tap` separado con formula `open-interceptor.rb`
- [x] **T4.9** Documentar instalación en README: `brew install`, `open-interceptor start`, setup env vars
- [x] **T4.10** Test end-to-end con instalación limpia ✅
- [x] **T4.11** Tag `v1.0.0` y release pública ✅

## Phase 5 — Hardening post-MVP (continuo)

- [~] **T5.1** ~~Métricas internas~~ — descartado
- [x] **T5.2** Health check `/healthz` que valida que cada provider configurado responde ✅
- [~] **T5.3** ~~Retry con backoff exponencial~~ — el harness de Claude Code ya lo maneja
- [x] **T5.4** `passthrough_auth: true` — validado en Anthropic (el único que importa) ✅
- [~] **T5.5** Tests de integración con servidor mock (`wiremock-rs`) — backlog

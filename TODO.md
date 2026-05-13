# open-interceptor — TODO

Estado: `[ ]` pendiente · `[~]` en progreso · `[x]` hecho

Marca cada tarea al completarla y commitea junto al cambio. El detalle técnico de cada item está en `/Users/usuario/.claude/plans/tengo-el-siguiente-plan-logical-wall.md`.

## Phase 0 — Setup del proyecto (1 día)

- [x] **T0.1** Crear este `TODO.md` en la raíz del repo
  - Lista trazable de tareas. Se actualiza commit a commit.
- [x] **T0.2** Crear `CLAUDE.md` en la raíz enlazando `TODO.md` y el plan
  - Punto de entrada para futuras sesiones de Claude Code en este repo.
- [ ] **T0.3** `cargo init --name open-interceptor` y commit inicial
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
- [ ] **T1.9** `sse_inject_usage()` — **deferred**: el upstream Anthropic real envía `usage` correctamente en `message_delta`. Reactivamos cuando aparezca un provider que omita el campo.
- [ ] **T1.10** `strip_thinking_blocks()` — **deferred**: por probar contra modelos open-source que no soportan thinking. Hasta entonces no se necesita.
- [ ] **T1.11** Error handling más fino — **parcialmente hecho**: errores upstream se mapean a 502 con shape Anthropic, errores de config a 500. Retry con backoff queda para v0.1.x.
- [ ] **T1.12** Logging — **suficiente para Phase 1**: cada dispatch loguea `model`, `effective_model`, `provider`, `provider_type`, `passthrough_auth`, `body_bytes`, `upstream_status`, `elapsed_ms`. Validado en producción.
- [x] **T1.13** ✅ E2E real contra `api.anthropic.com` con OAuth Pro/Max passthrough. 13 requests, todas con `upstream_status=200`, conversación multi-turno con tool use confirmada. Anthropic aceptó nuestros requests byte-idénticos.
- [x] **T1.14** ✅ E2E real contra `https://opencode.ai/zen/go/v1/messages` con MiniMax. Suscripción OpenCode Go funcionando vía nuestro proxy. (Replantea original con DeepSeek — el endpoint anthropic-compatible de DeepSeek directo queda para cuando alguien lo necesite, OpenCode Go ya cubre acceso a DeepSeek V4 Pro/Flash via Phase 3.)
- [ ] **T1.15** OpenRouter — **deferred**: no probado aún. La integración debería funcionar tal cual (anthropic_compatible). Lo añadimos si surge necesidad real.
- [x] **T1.16** Tag `v0.1.0-phase1` en git ✅

## Phase 2 — Endpoint `/v1/models` (1-2 días)

- [ ] **T2.1** `src/models_endpoint.rs`: handler `GET /v1/models` que retorna JSON Anthropic-shape `{ data: [{ id, type: "model", display_name }] }`
- [ ] **T2.2** Extender `config.rs`: campo opcional `models: Vec<String>` por provider en el YAML (lista estática)
- [ ] **T2.3** Opción dinámica: si el provider no declara `models`, hacer fetch real al `/v1/models` del provider y cachear en memoria con TTL configurable (default 1h)
- [ ] **T2.4** Registrar la ruta en `proxy.rs`
- [ ] **T2.5** Test: `curl http://127.0.0.1:3300/v1/models | jq` muestra union; en Claude Code el tab-completion de `/model` lista todos
- [ ] **T2.6** Documentar en README la variable `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`
- [ ] **T2.7** Tag `v0.2.0-phase2`

## Phase 3 — Translation layer OpenAI ↔ Anthropic (semana 2)

- [ ] **T3.1** `src/translate/types_anthropic.rs`: structs serde para `MessagesRequest`, `Message`, `ContentBlock` (enum tagged: `text`, `image`, `tool_use`, `tool_result`, `thinking`), `MessagesResponse`, eventos SSE (`message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, `ping`)
- [ ] **T3.2** `src/translate/types_openai.rs`: structs para `ChatCompletionRequest`, `ChatMessage` (con `role`, `content`, `tool_calls`, `tool_call_id`), `Tool`, `ChatCompletionResponse`, chunks SSE (`ChatCompletionChunk` con `choices[].delta`)
- [ ] **T3.3** `src/translate/req_anthropic_to_openai.rs`: función `convert(req)` que:
  - Convierte `system` field → mensaje `role: system`
  - Para cada `Message` Anthropic, aplana content blocks: `text` se concatena; `tool_use` → `tool_calls[]` en mensaje assistant; `tool_result` → mensaje separado `role: tool` con `tool_call_id`; `thinking` se descarta
  - Convierte `tools[]` (Anthropic) → `tools[]` (OpenAI), respetando el campo `input_schema` → `parameters`
  - Mapea `max_tokens`, `temperature`, `top_p`, `stop_sequences` → `stop`
- [ ] **T3.4** `src/translate/resp_openai_to_anthropic.rs::convert_non_streaming()`: toma `ChatCompletionResponse` y produce `MessagesResponse` válido. Mapea `finish_reason`: `stop` → `end_turn`, `length` → `max_tokens`, `tool_calls` → `tool_use`.
- [ ] **T3.5** `src/translate/resp_openai_to_anthropic.rs::convert_streaming()`: toma `impl Stream<Item = OpenAIChunk>` y emite `impl Stream<Item = AnthropicSSEEvent>`. Mantener estado: índice de content block actual, si está dentro de un tool_use, accumular argumentos JSON de tool calls.
- [ ] **T3.6** `src/translate/tool_translation.rs`: helpers compartidos para conversion bidireccional de tool definitions y resultados
- [ ] **T3.7** `src/providers/openai.rs`: pipeline completo (parse → translate request → POST upstream → translate response/stream → emit)
- [ ] **T3.8** Snapshot tests con `insta`: capturar 5 payloads reales de Claude Code (mensaje simple, con tool use, con tool result, con system prompt largo, streaming) y verificar que la traducción a OpenAI es consistente
- [ ] **T3.9** Cancelación: cuando el cliente cierra la conexión SSE, propagar drop al stream upstream con `tokio_util::sync::CancellationToken`
- [ ] **T3.10** Test manual: con OpenAI API key real, `/model gpt-4o` dentro de Claude Code; verificar mensaje simple, tool use (bash + read file), streaming largo
- [ ] **T3.11** Test manual con un endpoint OpenAI-compatible alternativo (Groq, Together, etc.)
- [ ] **T3.12** Tag `v0.3.0-phase3`

## Phase 4 — Daemon + CLI completa + Homebrew (semana 3)

- [ ] **T4.1** `src/daemon.rs::install()`: escribe `~/Library/LaunchAgents/com.open-interceptor.plist` con `RunAtLoad=true`, `KeepAlive=true`, `StandardOutPath` y `StandardErrorPath` en `~/.open-interceptor/logs/`
- [ ] **T4.2** `daemon::start()` con `launchctl load`, `stop()` con `launchctl unload`, `status()` que parsea `launchctl list` + check del puerto con `TcpStream::connect`
- [ ] **T4.3** Completar `cli.rs`: subcomandos `start`, `stop`, `status`, `config edit`, `config validate`, `logs [--follow]`
- [ ] **T4.4** Rotación de logs con `tracing-appender` daily, retener últimos 7 días
- [ ] **T4.5** Mensaje de error claro si puerto 3300 está ocupado, con instrucción de cambiar `port` en config
- [ ] **T4.6** `com.open-interceptor.plist.tmpl` con placeholders para path del binario
- [ ] **T4.7** GitHub Actions: workflow `release.yml` que cross-compila arm64 (apple-darwin) y x86_64 (apple-darwin) en `macos-latest` runner, sube binarios a la release
- [ ] **T4.8** Crear repo `homebrew-tap` separado con formula `open-interceptor.rb` apuntando a los binarios precompilados de GitHub Releases
- [ ] **T4.9** Documentar instalación en README: `brew install <tap>/open-interceptor`, `open-interceptor start`, setup de `ANTHROPIC_BASE_URL`
- [ ] **T4.10** Test end-to-end con instalación limpia: `brew install`, `open-interceptor start`, reiniciar máquina, verificar que sigue corriendo
- [ ] **T4.11** Tag `v1.0.0` y release pública

## Phase 5 — Hardening post-MVP (continuo)

- [ ] **T5.1** Métricas internas: contador de requests por provider, p50/p95/p99 latencia, expuesto en `/metrics` (Prometheus format) opcional
- [ ] **T5.2** Health check `/healthz` que valida que cada provider configurado responde
- [ ] **T5.3** Retry con backoff exponencial para errores transitorios 5xx upstream
- [ ] **T5.4** Soporte de `passthrough_auth: true` validado en todos los providers
- [ ] **T5.5** Tests de integración con servidor mock (`wiremock-rs`) para evitar depender de APIs reales en CI

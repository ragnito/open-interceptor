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
- [ ] **T1.2** `src/cli.rs`: subcomando `run --config <path>` (resto de comandos quedan stub para Phase 4)
- [ ] **T1.3** `src/main.rs`: parse args con `clap`, init tracing-subscriber, despacha a `cli::run()`
- [ ] **T1.4** `src/router.rs`: struct `Router` con `GlobSetBuilder` pre-compilado al cargar. Método `resolve(model: &str) -> Option<&Provider>` itera rutas en orden.
  - Soportar `remap: HashMap<String, String>` por ruta para reescribir el `model` antes de despachar al provider.
- [ ] **T1.5** `src/proxy.rs`: servidor Axum con `axum::Router::new()` y handler `POST /v1/messages`. Bufferizar body, parsear `model`, llamar al router.
  - Configurar `tokio::main` con `Builder::new_multi_thread`.
- [ ] **T1.6** Handler para `POST /v1/messages/count_tokens` con misma lógica de ruteo
- [ ] **T1.7** `src/providers/anthropic.rs`: implementar `forward()` con dos modos según `passthrough_auth`:
  - `passthrough_auth: false` (modo API key): sustituye `Authorization`/`x-api-key` con la del provider, forwardea el resto de headers end-to-end.
  - `passthrough_auth: true` (modo suscripción OAuth — DIFERENCIADOR): preserva **byte-exact** los headers entrantes incluyendo `Authorization: Bearer sk-ant-oat01-...`, `anthropic-version`, `anthropic-beta` (crítico: incluye `oauth-2025-04-20`), `anthropic-dangerous-direct-browser-access`, `x-app: cli`, `X-Claude-Code-Session-Id`, todos los `X-Stainless-*`, `User-Agent: claude-cli/...`. Preservar el query string del path (`?beta=true`). Forwardea el body byte-for-byte (incluido `metadata.user_id` con device/account/session IDs).
  - En ambos modos: rewrite del header `Host` al upstream; dropear hop-by-hop (RFC 7230 §6.1: `Connection`, `Keep-Alive`, `Proxy-*`, `TE`, `Trailer`, `Transfer-Encoding`, `Upgrade`); **nunca** añadir `Via`, `X-Forwarded-*`, `Forwarded`.
  - Ver `docs/claude-code-headers.md` para la especificación completa basada en captura empírica de Claude Code 2.1.140.
- [ ] **T1.8** Streaming SSE response passthrough con `axum::body::Body::from_stream(reqwest_response.bytes_stream())`
- [ ] **T1.9** `src/normalizer.rs`: función `sse_inject_usage()` que parsea cada chunk SSE Anthropic, detecta `message_delta` sin `usage`, inyecta `{ input_tokens: 0, output_tokens: 0 }`. Wrappear el stream del provider con esta función.
- [ ] **T1.10** `src/normalizer.rs`: función `strip_thinking_blocks()` que elimina content blocks tipo `thinking` del body request (para providers que no los soportan)
- [ ] **T1.11** Manejo de errores: si upstream retorna 4xx/5xx, propagar status code y body al cliente sin transformar. Si reqwest falla (network), retornar 502 con body Anthropic-like (`{type: "error", error: {type, message}}`).
- [ ] **T1.12** Logging con `tracing`: por cada request, log `model`, `provider`, `status`, `duration_ms` a nivel `info`. Body a nivel `debug` (no por defecto).
- [ ] **T1.13** Test manual end-to-end: con `ANTHROPIC_API_KEY` real, lanzar `cargo run -- run --config config.yaml.example` y desde otra terminal con `ANTHROPIC_BASE_URL=http://127.0.0.1:3300 claude` confirmar que `claude-sonnet-4-6` responde
- [ ] **T1.14** Repetir T1.13 con DeepSeek (provider `anthropic_compatible` apuntando a `https://api.deepseek.com/anthropic`)
- [ ] **T1.15** Repetir T1.13 con OpenRouter
- [ ] **T1.16** Tag `v0.1.0-phase1` en git

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

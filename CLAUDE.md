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

## Despliegue (daemon multiplataforma)

El daemon funciona igual en **macOS (launchd)** y **Linux (systemd user service)**. El binario es el mismo; la lógica específica de cada gestor de servicios vive en `src/daemon/` con dispatch por plataforma:

- `src/daemon/mod.rs` — API pública (`install`/`start`/`stop`/`status`/`probe`/`is_installed`/`uninstall`), helpers compartidos (`home`, `config_path`, `log_dir`) y dispatch por `cfg(target_os)`.
- `src/daemon/launchd.rs` — backend macOS (plist en `~/Library/LaunchAgents/`, `launchctl bootstrap/bootout gui/<uid>`).
- `src/daemon/systemd.rs` — backend Linux (unit en `~/.config/systemd/user/open-interceptor.service`, `systemctl --user enable --now`, `loginctl enable-linger`).
- `src/daemon/unsupported.rs` — stub para otras plataformas (mantiene `cargo check` verde; `run` sigue funcionando en foreground).

El `log_dir()` también es platform-aware: macOS `~/Library/Logs/open-interceptor`, Linux `$XDG_STATE_HOME/open-interceptor` (o `~/.local/state/open-interceptor`).

### Script de despliegue

```bash
./tools/deploy.sh            # release build + install + restart (Mac y Linux)
./tools/deploy.sh --debug     # debug build
./tools/deploy.sh --binary /path/to/binary  # usar binary ya compilado
```

### Reiniciar manualmente

```bash
open-interceptor stop
open-interceptor start --install   # genera plist (Mac) o unit (Linux) con path absoluto
open-interceptor start
```

### Notas sobre el path absoluto

**Ni launchd ni systemd resuelven un nombre suelto del binario.** El plist (`ProgramArguments`) y el unit (`ExecStart`) deben usar el **path absoluto**. En macOS, un nombre sin path falla con `Function not implemented` (PID 78 en `launchctl list`).

El script `deploy.sh` y `open-interceptor start --install` usan `std::env::current_exe()` para obtener el path absoluto al binary en el momento del install. Si luego se mueve el binary sin reinstall, el servicio volverá a fallar.

### Troubleshooting macOS (launchd)

```bash
launchctl list | grep open-interceptor          # estado del job
launchctl error <pid>                            # PID sin proceso → zombie
launchctl kickstart -k gui/$(id -u)/com.open-interceptor
ls -lah ~/Library/Logs/open-interceptor/         # logs (stdout/stderr van a /dev/null)
~/.local/bin/open-interceptor run --config ~/.config/open-interceptor/config.yaml  # arranque directo
```

| Síntoma | Causa | Solución |
|---------|-------|----------|
| `Bootstrap failed: 5: Input/output error` | Binary no encontrado por launchd | `open-interceptor start --install` para regenerar plist con path absoluto |
| `Function not implemented` (PID 78) | Mismo: plist sin path absoluto | Mismo |
| `Address already in use` | Otra instancia ocupando el puerto | `pkill -f open-interceptor` antes de arrancar |
| Daemon "running" pero puerto no responde | Zombie launchd entry | `launchctl remove com.open-interceptor` + reinstall |

### Troubleshooting Linux (systemd)

```bash
systemctl --user status open-interceptor         # estado del servicio
journalctl --user -u open-interceptor -e          # logs del journal
systemctl --user restart open-interceptor         # reiniciar
loginctl enable-linger                            # correr sin sesión activa (al boot)
ls -lah "${XDG_STATE_HOME:-$HOME/.local/state}/open-interceptor/"  # logs (tracing)
~/.local/bin/open-interceptor run --config ~/.config/open-interceptor/config.yaml  # arranque directo
```

| Síntoma | Causa | Solución |
|---------|-------|----------|
| `Failed to connect to bus` | No hay sesión de usuario systemd (SSH sin lingering) | `loginctl enable-linger` y reintentar |
| Servicio no arranca al boot | Falta lingering | `loginctl enable-linger` (lo intenta `start --install`) |
| `Address already in use` | Otra instancia ocupando el puerto | `pkill -f open-interceptor` antes de arrancar |
| Unit no aparece tras editar | systemd no recargó | `systemctl --user daemon-reload` (lo hace `start --install`) |

## Fases del proyecto

Ver `TODO.md`. Estamos siguiendo este orden estricto:

1. **Phase 0** — setup
2. **Phase 1** — proxy core + Anthropic-compatible (tag `v0.1.0-phase1`)
3. **Phase 2** — `/v1/models` spoofing (tag `v0.2.0-phase2`)
4. **Phase 3** — translation OpenAI ↔ Anthropic (tag `v0.3.0-phase3`)
5. **Phase 4** — daemon launchd + CLI + Homebrew (tag `v1.0.0`)
6. **Phase 5** — hardening post-MVP

No saltar de fase hasta que todas las tareas de la anterior estén `[x]` y exista el tag git correspondiente.

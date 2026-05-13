# Headers emitted by Claude Code 2.1.140 (subscription / OAuth auth)

Captured on 2026-05-13 with `tools/capture_headers.py` against a Pro/Max
subscription session (no `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` set,
so Claude Code falls back to the OAuth token stored locally).

This document describes the exact contract the proxy must preserve when
`passthrough_auth: true` is set on the `anthropic` provider.

## Two request shapes observed

### A. Validation / health-check (small, non-streaming)

```
POST /v1/messages?beta=true
Accept: application/json
Authorization: Bearer sk-ant-oat01-...   (length 115)
Content-Type: application/json
User-Agent: claude-cli/2.1.140 (external, cli)
X-Claude-Code-Session-Id: <uuid>
X-Stainless-Arch: arm64
X-Stainless-Lang: js
X-Stainless-OS: MacOS
X-Stainless-Package-Version: 0.93.0
X-Stainless-Retry-Count: 0
X-Stainless-Runtime: node
X-Stainless-Runtime-Version: v24.3.0
X-Stainless-Timeout: 600
anthropic-beta: oauth-2025-04-20,interleaved-thinking-2025-05-14,
                redact-thinking-2026-02-12,context-management-2025-06-27,
                prompt-caching-scope-2026-01-05
anthropic-dangerous-direct-browser-access: true
anthropic-version: 2023-06-01
x-app: cli
Accept-Encoding: gzip, deflate, br, zstd
Connection: keep-alive
Host: <proxy host>:<port>
Content-Length: ~320
```

Body (~320 B): `model: claude-haiku-4-5-...`, `stream: null`, `max_tokens: 1`,
1 message, no system, no tools, `metadata.user_id` carries a JSON-encoded
`{device_id, account_uuid, session_id}` triple.

### B. Real conversational turn (streaming)

Same headers as (A) **except**:

- `anthropic-beta` is longer and includes additional flags:
  `claude-code-20250219`, `context-1m-2025-08-07`, `advisor-tool-2026-03-01`,
  `effort-2025-11-24`, `extended-cache-ttl-2025-04-11` on top of the (A) list.
- Body is much larger (~118 KB): the system prompt, all 41 built-in tool
  definitions, `stream: true`, `max_tokens: 64000`, `model: claude-opus-4-7`.

## Auth header

- Scheme: `Authorization: Bearer <token>`
- Token shape: 115 chars, prefix `sk-ant-oat01-...` (OAuth access token).
  API keys instead start with `sk-ant-api03-...` and have a different length.
- Critical: the `anthropic-beta` value **must include `oauth-2025-04-20`** for
  the OAuth token to be accepted. Stripping it almost certainly breaks auth.

## Path

`/v1/messages?beta=true` — the `?beta=true` query parameter is present and
must be forwarded. Do not normalize the path without preserving the query
string.

## Body opacity

The proxy MUST forward the body byte-for-byte when `passthrough_auth` is on.
In particular `metadata.user_id` contains the user's `device_id`,
`account_uuid`, and `session_id` — tampering or stripping would be an obvious
"there is a proxy here" signal.

## Header policy for the proxy

### Transparency model

The contract is: **the HTTP request that arrives at `api.anthropic.com` must
be byte-identical, at the application layer, to the one Claude Code would
have sent going direct.** Same `Authorization`, same `anthropic-*`, same body
down to the metadata fields. Nothing added, nothing removed, nothing
reordered.

There are two headers that look like they "change" but are not application-
layer modifications — they describe the underlying TCP connection itself,
not the request:

- **`Host`** belongs to the TCP destination. When Claude Code talks straight
  to Anthropic, the client library writes `Host: api.anthropic.com` because
  that's where the socket points. When it talks to the proxy, it writes
  `Host: 127.0.0.1:3300` for the same reason. The proxy then opens its own
  new TCP connection to `api.anthropic.com`, and that connection naturally
  carries `Host: api.anthropic.com`. Net effect at the upstream: the same
  `Host` value Anthropic would have seen anyway.
- **`Connection`, `Keep-Alive`, `Transfer-Encoding`, `TE`, `Trailer`,
  `Upgrade`, `Proxy-*`** are hop-by-hop per RFC 7230 §6.1. They describe a
  single TCP hop, not the end-to-end request. The proxy's upstream client
  generates whatever values are correct for the new TCP hop.

Everything else — every end-to-end header, the path, the query string, the
body — must be forwarded verbatim. The lists below make that explicit so
nothing drifts in the implementation.

### Preserve verbatim (end-to-end headers)

- `Authorization`
- `anthropic-version`
- `anthropic-beta`
- `anthropic-dangerous-direct-browser-access`
- `x-app`
- `X-Claude-Code-Session-Id`
- `X-Stainless-Arch` / `Lang` / `OS` / `Package-Version` / `Retry-Count` /
  `Runtime` / `Runtime-Version` / `Timeout`
- `User-Agent`
- `Accept`
- `Accept-Encoding`
- `Content-Type`
- `Content-Length`

Plus any other end-to-end header the client may add in future CLI versions —
in practice, **forward everything except the hop-by-hop list below and the
`Host` header**.

### Rewrite

- `Host`: replace `127.0.0.1:<port>` with the upstream host
  (e.g. `api.anthropic.com`)

### Drop (hop-by-hop, RFC 7230 §6.1)

- `Connection`
- `Keep-Alive`
- `Proxy-Authenticate`
- `Proxy-Authorization`
- `TE`
- `Trailer`
- `Transfer-Encoding`
- `Upgrade`

Also drop any header listed in the incoming `Connection` value (RFC 7230
allows the client to mark additional headers as hop-by-hop that way).

### Never add

These are the typical "I'm a proxy" tell-tales — explicitly do not insert:

- `Via`
- `X-Forwarded-For`
- `X-Forwarded-Host`
- `X-Forwarded-Proto`
- `X-Real-IP`
- `Forwarded`

The goal under `passthrough_auth: true` is that an outgoing request is
indistinguishable from one Claude Code would have sent directly. Anything
that breaks that property risks getting the user's subscription flagged.

## Response path

Streaming responses are SSE (`Content-Type: text/event-stream`). The proxy
must:

- Forward the response status line and headers verbatim, again dropping
  hop-by-hop headers.
- Forward `Content-Encoding` and the compressed body bytes untouched.
  **Do not decompress, do not re-encode.** Decompressing would force the
  proxy to handle gzip/br/zstd, change `Content-Length`, and risk altering
  byte ordering of SSE chunks.
- Flush each chunk to the client as it arrives — no buffering past what the
  underlying TCP stack does. (This is the main reason this proxy exists in
  Rust rather than Node.)
- Propagate connection cancellation upstream when the client disconnects, so
  Anthropic doesn't see ghost streams keeping a slot open.

## Out of scope (translation case)

When the request routes to an OpenAI-compatible provider via
`translate/`, none of the Anthropic-specific headers survive — the body
itself is rewritten. The fingerprinting headers above are only relevant
when `provider.type == anthropic_compatible` AND `passthrough_auth: true`.

#!/usr/bin/env python3
"""
Mock Anthropic-API server for capturing the exact headers Claude Code sends.

Use case: figure out what authentication / version / user-agent headers Claude
Code emits when authenticating with an OAuth subscription (Pro/Max) vs an API
key, so we can implement passthrough_auth correctly in the Rust proxy.

Usage:
    python3 tools/capture_headers.py

Then in another terminal:
    export ANTHROPIC_BASE_URL=http://127.0.0.1:3300
    claude
    # send any message, e.g. "hi"

The server responds with a valid (mock) Anthropic message so Claude Code does
not show an error, then logs every header/body to stdout for inspection.

The token in the auth header is redacted by default to avoid accidentally
leaking it in screenshots / pasted output. Use --no-redact to see it raw.
"""

from __future__ import annotations
import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

REDACT = True


def redact_secret(value: str) -> str:
    """Show only first 12 and last 4 chars of a secret-looking value."""
    if not REDACT or len(value) < 24:
        return value
    return f"{value[:12]}...{value[-4:]} [len={len(value)}]"


def format_headers(headers) -> str:
    sensitive = {"authorization", "x-api-key", "anthropic-auth-token", "cookie"}
    lines = []
    for k, v in headers.items():
        if k.lower() in sensitive:
            v = redact_secret(v)
        lines.append(f"  {k}: {v}")
    return "\n".join(lines)


def sse_event(event: str, data: dict) -> bytes:
    return f"event: {event}\ndata: {json.dumps(data)}\n\n".encode()


class Handler(BaseHTTPRequestHandler):
    # http.server buffers writes by default, which breaks SSE — the
    # client sees no bytes until the handler returns. Setting wbufsize=0
    # makes self.wfile unbuffered so each `write()` reaches the socket.
    wbufsize = 0

    def log_message(self, fmt, *args):
        # Silence default access log; we print our own.
        pass

    def _capture(self):
        print("\n" + "=" * 72, flush=True)
        print(f"  {self.command} {self.path}", flush=True)
        print("-" * 72, flush=True)
        print(format_headers(self.headers), flush=True)

        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""
        if body:
            print("-" * 72, flush=True)
            print(f"  Body ({len(body)} bytes):", flush=True)
            try:
                obj = json.loads(body)
                # show only the headline fields, not the whole conversation
                summary = {
                    "model": obj.get("model"),
                    "stream": obj.get("stream"),
                    "max_tokens": obj.get("max_tokens"),
                    "messages_count": len(obj.get("messages", [])),
                    "system_present": "system" in obj,
                    "tools_count": len(obj.get("tools", [])),
                    "metadata": obj.get("metadata"),
                }
                print(f"  {json.dumps(summary, indent=2)}", flush=True)
            except Exception as e:
                print(f"  (not json: {e})", flush=True)
                print(f"  {body[:200]!r}", flush=True)
        print("=" * 72 + "\n", flush=True)

        return body

    def do_POST(self):
        body = self._capture()

        # Decide streaming vs non-streaming from body
        is_streaming = False
        try:
            is_streaming = json.loads(body).get("stream", False)
        except Exception:
            pass

        if "/v1/messages" in self.path and is_streaming:
            self._send_sse_mock()
        elif "/v1/messages" in self.path:
            self._send_json_mock()
        else:
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"type":"error","error":{"type":"not_found","message":"mock"}}')

    def do_GET(self):
        self._capture()
        if self.path == "/models":
            payload = {"data": [{"id": "mock-model", "type": "model"}]}
            data = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def _send_json_mock(self):
        resp = {
            "id": "msg_mock_capture",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "headers captured"}],
            "model": "mock",
            "stop_reason": "end_turn",
            "stop_sequence": None,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        }
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _send_sse_mock(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        # Force connection close at end so curl/clients know the response
        # is over once we stop writing. Without this, http.server keeps
        # the socket alive and clients hang waiting for more bytes.
        self.send_header("Connection", "close")
        self.end_headers()

        events = [
            ("message_start", {
                "type": "message_start",
                "message": {
                    "id": "msg_mock_capture",
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": "mock",
                    "stop_reason": None,
                    "stop_sequence": None,
                    "usage": {"input_tokens": 1, "output_tokens": 0},
                },
            }),
            ("content_block_start", {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            }),
            ("content_block_delta", {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "headers captured"},
            }),
            ("content_block_stop", {
                "type": "content_block_stop",
                "index": 0,
            }),
            ("message_delta", {
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                "usage": {"output_tokens": 2},
            }),
            ("message_stop", {"type": "message_stop"}),
        ]
        for name, payload in events:
            self.wfile.write(sse_event(name, payload))
            self.wfile.flush()


def main():
    global REDACT
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=3300)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument(
        "--no-redact",
        action="store_true",
        help="Show auth tokens in plaintext (DANGER: do not screenshot/paste)",
    )
    args = p.parse_args()

    REDACT = not args.no_redact

    print(f"Mock Anthropic server listening on http://{args.host}:{args.port}")
    print("Point Claude Code at it:")
    print(f"    export ANTHROPIC_BASE_URL=http://{args.host}:{args.port}")
    print("    claude")
    print(f"Token redaction: {'ON (use --no-redact to disable)' if REDACT else 'OFF'}")
    print("Press Ctrl+C to stop.\n", flush=True)

    server = HTTPServer((args.host, args.port), Handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()

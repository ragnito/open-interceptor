#!/usr/bin/env python3
"""
Minimal mock OpenAI Chat Completions server for verifying the open-
interceptor translation layer end-to-end without spending real tokens.

Listens on a port (default 3302), expects POST /v1/chat/completions
with an OpenAI request body, prints the incoming body summary, and
returns a fixed OpenAI-shaped response. The proxy is supposed to
translate Anthropic → OpenAI on the way in and OpenAI → Anthropic on
the way out, so the client sees Anthropic shape both sides of the wire.

Usage:
    python3 tools/mock_openai.py --port 3302
"""

from __future__ import annotations
import argparse
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    wbufsize = 0

    def log_message(self, fmt, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b""

        print("\n" + "=" * 72, flush=True)
        print(f"  POST {self.path}", flush=True)
        print("-" * 72, flush=True)
        for k, v in self.headers.items():
            if k.lower() == "authorization":
                v = v[:16] + "..." + v[-6:] if len(v) > 24 else v
            print(f"  {k}: {v}", flush=True)
        print("-" * 72, flush=True)
        print(f"  Body ({len(raw)} bytes):", flush=True)
        try:
            obj = json.loads(raw)
            # Print a structural summary, not the whole conversation
            summary = {
                "model": obj.get("model"),
                "stream": obj.get("stream"),
                "max_tokens": obj.get("max_tokens"),
                "messages": [
                    {
                        "role": (m.get("role") if isinstance(m, dict) else "?"),
                        "content_type": (
                            "str"
                            if isinstance(m.get("content"), str)
                            else (
                                "list"
                                if isinstance(m.get("content"), list)
                                else type(m.get("content")).__name__
                            )
                        ),
                        "tool_calls": len(m.get("tool_calls", []))
                        if isinstance(m, dict)
                        else 0,
                    }
                    for m in obj.get("messages", [])
                ],
                "tools": [t.get("function", {}).get("name") for t in obj.get("tools", [])],
                "tool_choice": obj.get("tool_choice"),
                "stop": obj.get("stop"),
            }
            print(f"  {json.dumps(summary, indent=2)}", flush=True)
        except Exception as e:
            print(f"  (not json: {e})", flush=True)
        print("=" * 72 + "\n", flush=True)

        # Reply with a canned OpenAI response. We force non-streaming
        # here regardless of what the request asked for — the mock isn't
        # in the business of doing real SSE.
        resp = {
            "id": "chatcmpl-mock-001",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": "mock-llm",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "mock OpenAI response (translation OK)",
                    },
                    "finish_reason": "stop",
                }
            ],
            "usage": {
                "prompt_tokens": 13,
                "completion_tokens": 7,
                "total_tokens": 20,
            },
        }
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        # /v1/models stub, just in case.
        if self.path == "/v1/models":
            payload = {"data": [{"id": "mock-llm", "object": "model"}]}
            data = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=3302)
    p.add_argument("--host", default="127.0.0.1")
    args = p.parse_args()
    print(f"mock OpenAI server on http://{args.host}:{args.port}", flush=True)
    print("Press Ctrl+C to stop.\n", flush=True)
    HTTPServer((args.host, args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()

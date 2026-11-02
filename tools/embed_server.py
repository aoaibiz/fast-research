#!/usr/bin/env python3
"""OpenAI-compatible local embedding server (fastembed + onnx).

RustXNG calls POST {embedding_url}/v1/embeddings to re-rank results
semantically, matching neural search engines like Exa without a cloud key.

    python3 tools/embed_server.py --port 18998
"""
from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from fastembed import TextEmbedding

MODEL = None


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if not self.path.endswith("/embeddings"):
            self._send(404, {"error": "only /embeddings is supported"})
            return
        try:
            length = int(self.headers.get("Content-Length", 0))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception:  # noqa: BLE001
            self._send(400, {"error": "invalid JSON"})
            return
        text = body.get("input", "")
        texts = [text] if isinstance(text, str) else list(text)
        if not texts:
            self._send(400, {"error": "input is required"})
            return
        vectors = [v.tolist() for v in MODEL.embed(texts)]
        payload = {
            "object": "list",
            "model": body.get("model", "fastembed"),
            "data": [{"object": "embedding", "index": i, "embedding": v} for i, v in enumerate(vectors)],
            "usage": {"prompt_tokens": 0, "total_tokens": 0},
        }
        self._send(200, payload)

    def _send(self, status: int, obj: dict) -> None:
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def main() -> None:
    global MODEL
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18998)
    parser.add_argument("--model", default="BAAI/bge-small-en-v1.5")
    args = parser.parse_args()
    MODEL = TextEmbedding(args.model)
    print(f"embedding server on :{args.port} model={args.model}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()

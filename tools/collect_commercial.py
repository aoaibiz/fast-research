#!/usr/bin/env python3
"""Collect Tavily and Ollama search results for the quality questions.

Reads TAVILY_API_KEY / OLLAMA_API_KEY from the environment (source the env
file first). Writes bench/content-tavily.json and bench/content-ollama.json in
the same shape as bench/content-rustxng.json, so tools/eval_content.py can score
all three with identical metrics.

Usage:
    set -a; source ~/.dsh/search-keys.env; set +a
    python3 tools/collect_commercial.py
"""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.request
from pathlib import Path

QUESTIONS = json.loads(Path("bench/quality-questions.json").read_text(encoding="utf-8"))["queries"]


def post(url: str, key: str, body: dict, timeout: float = 60.0) -> dict:
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json", "User-Agent": "content-eval"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def tavily(key: str) -> dict:
    rows = []
    for q in QUESTIONS:
        try:
            data = post("https://api.tavily.com/search", key, {"query": q["query"], "max_results": 10, "search_depth": "basic"})
            results = [{"url": r.get("url"), "title": r.get("title"), "content": r.get("content")} for r in data.get("results", [])]
        except Exception as error:  # noqa: BLE001
            print("  tavily error", q["id"], error, file=sys.stderr)
            results = []
        rows.append({"id": q["id"], "query": q["query"], "gold_url": q["gold"][0], "results": results})
        print("tavily", q["id"], len(results))
        time.sleep(1.5)
    return {"source": "tavily", "queries": rows}


def ollama(key: str) -> dict:
    rows = []
    for q in QUESTIONS:
        try:
            data = post("https://ollama.com/api/web_search", key, {"query": q["query"], "max_results": 10})
            results = [{"url": r.get("url"), "title": r.get("title"), "content": r.get("content")} for r in data.get("results", [])]
        except Exception as error:  # noqa: BLE001
            print("  ollama error", q["id"], error, file=sys.stderr)
            results = []
        rows.append({"id": q["id"], "query": q["query"], "gold_url": q["gold"][0], "results": results})
        print("ollama", q["id"], len(results))
        time.sleep(1.5)
    return {"source": "ollama", "queries": rows}


def exa(key: str) -> dict:
    """Exa neural search: request text content so the metrics match Tavily."""
    rows = []
    for q in QUESTIONS:
        try:
            data = post(
                "https://api.exa.ai/search",
                key,
                {
                    "query": q["query"],
                    "numResults": 10,
                    "type": "auto",
                    "contents": {"text": True},
                },
            )
            # contents.text: True makes Exa return the page text in "text"
            results = [
                {"url": r.get("url"), "title": r.get("title"), "content": r.get("text") or ""}
                for r in data.get("results", [])
            ]
        except Exception as error:  # noqa: BLE001
            print("  exa error", q["id"], error, file=sys.stderr)
            results = []
        rows.append({"id": q["id"], "query": q["query"], "gold_url": q["gold"][0], "results": results})
        print("exa", q["id"], len(results))
        time.sleep(1.5)
    return {"source": "exa", "queries": rows}


def main() -> int:
    if os.environ.get("EXA_API_KEY"):
        Path("bench/content-exa.json").write_text(json.dumps(exa(os.environ["EXA_API_KEY"]), ensure_ascii=False, indent=1), encoding="utf-8")
        print("wrote bench/content-exa.json")
    else:
        print("EXA_API_KEY not set; skipping exa", file=sys.stderr)
    if os.environ.get("TAVILY_API_KEY"):
        Path("bench/content-tavily.json").write_text(json.dumps(tavily(os.environ["TAVILY_API_KEY"]), ensure_ascii=False, indent=1), encoding="utf-8")
        print("wrote bench/content-tavily.json")
    else:
        print("TAVILY_API_KEY not set; skipping tavily", file=sys.stderr)
    if os.environ.get("OLLAMA_API_KEY"):
        Path("bench/content-ollama.json").write_text(json.dumps(ollama(os.environ["OLLAMA_API_KEY"]), ensure_ascii=False, indent=1), encoding="utf-8")
        print("wrote bench/content-ollama.json")
    else:
        print("OLLAMA_API_KEY not set; skipping ollama", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())

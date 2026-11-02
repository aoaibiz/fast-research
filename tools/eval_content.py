#!/usr/bin/env python3
"""Measure the CONTENT quality of search results (not just URL ranking).

For each question it scores how useful the returned snippets are:
  - coverage: fraction of the top-k results with a non-empty content snippet
  - snippet_chars: median length of the content snippets
  - term_coverage: average fraction of query terms found in title+content
  - gold_snippet: whether the snippet of the result that hit the gold URL
    actually mentions a query term (the agent reads this text to answer)

It works on any JSON that has a "results" list with url/title/content, so it
can score RustXNG directly and (after saving their responses) Tavily/Ollama.

Usage:
    python3 tools/eval_content.py --input bench/content-rustxng.json
    python3 tools/eval_content.py --input bench/content-tavily.json --top 5
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
from pathlib import Path


def query_terms(query: str) -> list[str]:
    lower = query.lower()
    words = [w for w in re.split(r"[^\w]+", lower) if len(w) >= 3]
    if words:
        return words
    chars = [c for c in lower if c.isalnum()]
    return ["".join(chars[i:i + 2]) for i in range(len(chars) - 1)] or chars


def score(payload: dict, top: int) -> dict:
    terms = query_terms(payload["query"])
    results = (payload.get("results") or [])[:top]
    if not results:
        return {"results": 0, "coverage": 0.0, "snippet_chars": 0, "term_coverage": 0.0}

    snippets = [str(r.get("content") or "").strip() for r in results]
    covered = sum(1 for s in snippets if s)
    chars = [len(s) for s in snippets if s]

    term_hits = []
    for r in results:
        haystack = (str(r.get("title") or "") + " " + str(r.get("content") or "")).lower()
        if terms:
            term_hits.append(sum(1 for t in terms if t in haystack) / len(terms))
    gold_snippet = None
    gold_url = payload.get("gold_url")
    if gold_url:
        for r in results:
            if str(r.get("url", "")).startswith(gold_url.rstrip("/")):
                haystack = (str(r.get("title") or "") + " " + str(r.get("content") or "")).lower()
                gold_snippet = round(sum(1 for t in terms if t in haystack) / len(terms), 3) if terms else 1.0
                break

    return {
        "results": len(results),
        "coverage": round(covered / len(results), 3),
        "snippet_chars": int(statistics.median(chars)) if chars else 0,
        "term_coverage": round(statistics.fmean(term_hits), 3),
        "gold_snippet": gold_snippet,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--input", required=True)
    parser.add_argument("--top", type=int, default=5)
    args = parser.parse_args()

    data = json.loads(Path(args.input).read_text(encoding="utf-8"))
    rows = [score(q, args.top) | {"id": q["id"]} for q in data["queries"]]
    for row in rows:
        gold = row.get("gold_snippet")
        print(
            f"{row['id']:<18} n={row['results']:<3} coverage={row['coverage']:.2f} "
            f"snippet={row['snippet_chars']:>5} chars  terms={row['term_coverage']:.2f} "
            f"gold_terms={'-' if gold is None else f'{gold:.2f}'}"
        )
    n = len(rows)
    summary = {
        "input": args.input,
        "top": args.top,
        "questions": n,
        "mean_coverage": round(statistics.fmean(r["coverage"] for r in rows), 3),
        "median_snippet_chars": int(statistics.median(r["snippet_chars"] for r in rows if r["snippet_chars"])) if any(r["snippet_chars"] for r in rows) else 0,
        "mean_term_coverage": round(statistics.fmean(r["term_coverage"] for r in rows), 3),
    }
    golds = [r["gold_snippet"] for r in rows if r.get("gold_snippet") is not None]
    if golds:
        summary["mean_gold_term_coverage"] = round(statistics.fmean(golds), 3)
    print()
    print(json.dumps(summary, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())

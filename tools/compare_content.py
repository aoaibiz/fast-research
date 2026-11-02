#!/usr/bin/env python3
"""Side-by-side content-quality comparison across search backends.

Scores bench/content-*.json files with identical metrics and prints a table so
RustXNG can be compared against SearXNG, Tavily, Ollama, ... with one command.

Usage:
    python3 tools/compare_content.py
    python3 tools/compare_content.py --top 10
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from eval_content import query_terms  # noqa: E402

BACKENDS = [
    ("RustXNG", "bench/content-rustxng.json"),
    ("SearXNG", "bench/content-searxng.json"),
    ("Tavily", "bench/content-tavily.json"),
    ("Exa", "bench/content-exa.json"),
    ("Ollama", "bench/content-ollama.json"),
]


def score_file(path: Path, top: int) -> dict | None:
    if not path.exists():
        return None
    data = json.loads(path.read_text(encoding="utf-8"))
    covs, chars, term_covs, gold_covs = [], [], [], []
    for q in data["queries"]:
        terms = query_terms(q["query"])
        results = (q.get("results") or [])[:top]
        if not results:
            covs.append(0.0)
            continue
        snippets = [str(r.get("content") or "").strip() for r in results]
        covs.append(sum(1 for s in snippets if s) / len(results))
        chars += [len(s) for s in snippets if s]
        hits = []
        for r in results:
            haystack = (str(r.get("title") or "") + " " + str(r.get("content") or "")).lower()
            hits.append(sum(1 for t in terms if t in haystack) / len(terms) if terms else 1.0)
        term_covs.append(statistics.fmean(hits))
        gold = q.get("gold_url")
        if gold:
            for r in results:
                if str(r.get("url", "")).startswith(gold.rstrip("/")):
                    haystack = (str(r.get("title") or "") + " " + str(r.get("content") or "")).lower()
                    gold_covs.append(sum(1 for t in terms if t in haystack) / len(terms) if terms else 1.0)
                    break
    return {
        "questions": len(data["queries"]),
        "coverage": round(statistics.fmean(covs), 3) if covs else 0,
        "snippet_chars": int(statistics.median(chars)) if chars else 0,
        "term_coverage": round(statistics.fmean(term_covs), 3) if term_covs else 0,
        "gold_term_coverage": round(statistics.fmean(gold_covs), 3) if gold_covs else None,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--top", type=int, default=5)
    args = parser.parse_args()

    print(f"内容品質の比較（上位 {args.top} 件）\n")
    header = f"{'backend':<10} {'coverage':>9} {'snippet':>9} {'terms':>7} {'gold_terms':>11}"
    print(header)
    print("-" * len(header))
    for name, rel in BACKENDS:
        result = score_file(Path(rel), args.top)
        if result is None:
            print(f"{name:<10} {'(未収集)':>9}")
            continue
        gold = "-" if result["gold_term_coverage"] is None else f"{result['gold_term_coverage']:.3f}"
        print(f"{name:<10} {result['coverage']:>9.3f} {result['snippet_chars']:>7} ch {result['term_coverage']:>7.3f} {gold:>11}")
    print()
    print("coverage = クエリ語がスニペットに含まれる割合（記述の豊富さの代理指標）")
    print("gold_terms = gold URL のスニペットがクエリ語を含む割合（答えに近い記述か）")
    return 0


if __name__ == "__main__":
    sys.exit(main())

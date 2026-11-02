#!/usr/bin/env python3
"""Compare a rustxng instance with a reference SearXNG instance.

For every engine and query the script asks both instances for JSON results and
reports (a) the latency distribution and (b) how much the result sets agree:

  overlap@10   Jaccard index of the top-10 URLs (1.0 = identical sets)
  top1         share of queries where both instances rank the same URL first
  found        share of queries where the engine returned at least one result

The same query string (`!engine <query>`) is sent to both instances, so each
one runs exactly that engine.

Usage:
    python3 bench/compare.py --a http://127.0.0.1:8899 --b http://127.0.0.1:8001 \
        --engines wikipedia,hackernews,arxiv --repeats 2 --json bench/result.json
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

# name on the rustxng side [= name on the SearXNG side]
DEFAULT_ENGINES = [
    "hackernews",
    "github",
    "crates.io",
    "npm",
    "deezer",
    "dailymotion",
    "openstreetmap",
    "photon",
    "crossref",
    "bing news",
]
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0"


def search(base: str, query: str, engine: str, timeout: float) -> dict:
    """Run one search, returning latency, results and errors.

    The engine is selected with the `engines` parameter, so both instances use
    their own name for it (names differ for a few engines).
    """
    params = {"q": query, "format": "json"}
    if engine:
        params["engines"] = engine
    url = base.rstrip("/") + "/search?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read()[:200].decode("utf-8", "replace")
        return {"error": f"HTTP {error.code}: {detail}", "millis": (time.perf_counter() - started) * 1000}
    except Exception as error:  # noqa: BLE001 - report whatever happened
        return {"error": f"{type(error).__name__}: {error}", "millis": (time.perf_counter() - started) * 1000}

    millis = (time.perf_counter() - started) * 1000
    results = body.get("results") or []
    urls = [str(result.get("url", "")) for result in results[:10]]
    unresponsive = []
    for entry in body.get("unresponsive_engines") or []:
        if isinstance(entry, (list, tuple)) and len(entry) >= 2:
            unresponsive.append(f"{entry[0]}:{entry[1]}")
        elif isinstance(entry, dict):
            unresponsive.append(f"{entry.get('engine')}:{entry.get('error_type')}")
    return {
        "millis": millis,
        "urls": urls,
        "count": len(results),
        "number_of_results": body.get("number_of_results", 0),
        "unresponsive": unresponsive,
    }


def stats(values: list[float]) -> dict:
    if not values:
        return {"n": 0, "mean": 0.0, "median": 0.0, "p95": 0.0, "max": 0.0}
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round(0.95 * len(ordered))) - 1))
    return {
        "n": len(ordered),
        "mean": round(statistics.fmean(ordered), 1),
        "median": round(statistics.median(ordered), 1),
        "p95": round(ordered[index], 1),
        "max": round(ordered[-1], 1),
    }


def jaccard(left: list[str], right: list[str]) -> float:
    a, b = set(left), set(right)
    if not a and not b:
        return 1.0
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--a", default="http://127.0.0.1:8899", help="rustxng base URL")
    parser.add_argument("--b", default="http://127.0.0.1:8001", help="reference SearXNG base URL")
    parser.add_argument(
        "--engines",
        default=",".join(DEFAULT_ENGINES),
        help="comma separated engine names, optionally rust_name=searxng_name",
    )
    parser.add_argument("--queries", default="bench/queries.txt")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument(
        "--delay",
        type=float,
        default=0.6,
        help="seconds to sleep between requests (avoids rate limits)",
    )
    parser.add_argument("--json", dest="json_path", default=None)
    args = parser.parse_args()

    engines = []
    for item in args.engines.split(","):
        item = item.strip()
        if not item:
            continue
        if "=" in item:
            rust_name, searxng_name = (part.strip() for part in item.split("=", 1))
        else:
            rust_name = searxng_name = item
        engines.append((rust_name, searxng_name))
    with open(args.queries, encoding="utf-8") as handle:
        queries = [line.strip() for line in handle if line.strip() and not line.startswith("#")]

    report = {
        "a": args.a,
        "b": args.b,
        "engines": {},
        "queries": queries,
        "repeats": args.repeats,
        "delay": args.delay,
    }
    print(f"comparing A={args.a} (rustxng) with B={args.b} (searxng)")
    print(f"{len(engines)} engines × {len(queries)} queries × {args.repeats} repeats\n")
    print(f"{'engine':<16} {'query':<26} {'A ms':>8} {'B ms':>8} {'A/B':>6} {'A n':>5} {'B n':>5} {'overlap':>8} {'top1':>6}")

    for rust_engine, searxng_engine in engines:
        engine = rust_engine
        latencies_a: list[float] = []
        latencies_b: list[float] = []
        overlaps: list[float] = []
        top1_hits = 0
        comparisons = 0
        found_a = found_b = 0
        errors: list[str] = []
        for query in queries:
            for repeat in range(args.repeats):
                result_a = search(args.a, query, rust_engine, args.timeout)
                if args.delay:
                    time.sleep(args.delay)
                result_b = search(args.b, query, searxng_engine, args.timeout)
                if args.delay:
                    time.sleep(args.delay)
                for name, result in (("A", result_a), ("B", result_b)):
                    if "error" in result:
                        errors.append(f"{name} {engine} {query!r}: {result['error']}")

                if "millis" in result_a and "error" not in result_a:
                    latencies_a.append(result_a["millis"])
                if "millis" in result_b and "error" not in result_b:
                    latencies_b.append(result_b["millis"])

                urls_a = result_a.get("urls") or []
                urls_b = result_b.get("urls") or []
                overlap = jaccard(urls_a, urls_b)
                overlaps.append(overlap)
                comparisons += 1
                if urls_a and urls_b and urls_a[0] == urls_b[0]:
                    top1_hits += 1
                found_a += 1 if urls_a else 0
                found_b += 1 if urls_b else 0

                ratio = (result_a["millis"] / result_b["millis"]) if result_b.get("millis") else 0.0
                print(
                    f"{engine:<16} {query[:26]:<26} {result_a['millis']:>8.0f} {result_b['millis']:>8.0f} "
                    f"{ratio:>6.2f} {len(urls_a):>5} {len(urls_b):>5} {overlap:>8.2f} "
                    f"{'yes' if urls_a and urls_b and urls_a[0] == urls_b[0] else 'no':>6}"
                )

        report["engines"][engine] = {
            "a": stats(latencies_a),
            "b": stats(latencies_b),
            "overlap_at_10": round(statistics.fmean(overlaps), 3) if overlaps else 0.0,
            "top1_agreement": round(top1_hits / comparisons, 3) if comparisons else 0.0,
            "found_a": round(found_a / comparisons, 3) if comparisons else 0.0,
            "found_b": round(found_b / comparisons, 3) if comparisons else 0.0,
            "errors": errors[:5],
        }

    print("\n== summary ==")
    print(f"{'engine':<16} {'A median':>9} {'B median':>9} {'A/B':>6} {'overlap@10':>11} {'top1':>6} {'A found':>8} {'B found':>8}")
    for engine, data in report["engines"].items():
        a_median = data["a"]["median"]
        b_median = data["b"]["median"]
        ratio = (a_median / b_median) if b_median else 0.0
        print(
            f"{engine:<16} {a_median:>8.0f}ms {b_median:>8.0f}ms {ratio:>6.2f} "
            f"{data['overlap_at_10']:>11.2f} {data['top1_agreement']:>6.2f} {data['found_a']:>8.2f} {data['found_b']:>8.2f}"
        )
        if data["errors"]:
            for error in data["errors"]:
                print(f"    ! {error}")

    if args.json_path:
        with open(args.json_path, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2, ensure_ascii=False)
        print(f"\nwrote {args.json_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

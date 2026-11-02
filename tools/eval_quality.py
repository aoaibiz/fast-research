#!/usr/bin/env python3
"""Measure the quality of a RustXNG instance on a fixed question set.

For every question the script asks the instance for JSON results and checks
whether one of the gold URL prefixes appears in the top k results. It reports
hit@1 / hit@5 / MRR together with the latency and the response size, which are
the numbers an agent tool call actually pays for.

Usage:
    python3 tools/eval_quality.py --base http://127.0.0.1:8905
    python3 tools/eval_quality.py --base http://127.0.0.1:8905 \
        --params "strategy=staged&max_results=10" --json bench/quality-staged.json
"""

from __future__ import annotations

import argparse
import re
import json
import statistics
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def search(base: str, query: str, extra: dict[str, str], timeout: float) -> dict:
    params = {"q": query, "format": "json", "max_results": "10", **extra}
    url = base.rstrip("/") + "/search?" + urllib.parse.urlencode(params)
    request = urllib.request.Request(url, headers={"User-Agent": "rustxng-quality-eval"})
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
    except Exception as error:  # noqa: BLE001 - the report wants the reason
        return {"error": f"{type(error).__name__}: {error}", "millis": 0.0, "urls": [], "bytes": 0}
    millis = (time.perf_counter() - started) * 1000
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return {"error": "invalid JSON", "millis": millis, "urls": [], "bytes": len(body)}
    urls = [str(item.get("url", "")) for item in payload.get("results") or []]
    return {"millis": millis, "urls": urls, "bytes": len(body), "unresponsive": len(payload.get("unresponsive_engines") or [])}


_VERSION_SEGMENT = re.compile(r"^(?:stable|current|latest|v?\d+(?:\.\d+)*|\d+(?:\.\d+)*)$")


def normalise(url: str) -> str:
    """Strip the scheme and percent escapes, and treat version segments
    (stable / current / 6.1 / 3 / v2) as wildcards: the same page under a
    different version path still counts as the answer."""
    from urllib.parse import unquote

    rest = unquote(str(url).split("://", 1)[-1]).rstrip("/").lower()
    host, _, path = rest.partition("/")
    parts = []
    for segment in path.split("/"):
        parts.append("*" if _VERSION_SEGMENT.match(segment) else segment)
    return host + "/" + "/".join(parts)


def hit_rank(urls: list[str], gold: list[str]) -> int | None:
    prefixes = [normalise(prefix) for prefix in gold]
    for index, url in enumerate(urls, start=1):
        candidate = normalise(url)
        if any(candidate.startswith(prefix) for prefix in prefixes):
            return index
    return None


def restart_instance(binary: str, port: int, base: str) -> None:
    """Stop whatever listens on the port, start the binary, wait for /healthz."""
    import subprocess
    import time as _time

    # Kill whatever listens on the port. A pkill -f pattern would also match the
    # shell that launched this script (its command line contains the pattern).
    subprocess.run(
        f"fuser -k {port}/tcp",
        shell=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    _time.sleep(1)
    subprocess.Popen(
        [binary, "--port", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    for _ in range(40):
        _time.sleep(0.5)
        try:
            with urllib.request.urlopen(f"{base}/healthz", timeout=2) as response:
                if response.status == 200:
                    return
        except Exception:  # noqa: BLE001 - still starting
            continue


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default="http://127.0.0.1:8905")
    parser.add_argument("--questions", default=str(ROOT / "bench" / "quality-questions.json"))
    parser.add_argument("--params", default="", help="extra query parameters, e.g. strategy=staged")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--json", dest="json_path", default=None)
    parser.add_argument("--delay", type=float, default=1.0)
    parser.add_argument(
        "--restart-binary",
        default=None,
        help="restart this binary every --restart-every questions, so engine "
        "suspensions from the run itself do not pollute the later questions",
    )
    parser.add_argument("--restart-every", type=int, default=20)
    parser.add_argument("--port", type=int, default=8979)
    args = parser.parse_args()

    extra: dict[str, str] = {}
    for item in args.params.split("&"):
        if not item:
            continue
        key, _, value = item.partition("=")
        extra[key] = value

    questions = json.loads(Path(args.questions).read_text(encoding="utf-8"))["queries"]
    rows = []
    ranks = []
    if args.restart_binary:
        # start a fresh instance for the first batch too
        base = f"http://127.0.0.1:{args.port}"
        restart_instance(args.restart_binary, args.port, base)
        print("  (started instance)", file=sys.stderr)
    for index, question in enumerate(questions):
        if args.restart_binary and index and index % args.restart_every == 0:
            base = f"http://127.0.0.1:{args.port}"
            restart_instance(args.restart_binary, args.port, base)
            print(f"  (restarted instance at question {index})", file=sys.stderr)
        result = search(args.base, question["query"], extra, args.timeout)
        rank = hit_rank(result.get("urls") or [], question["gold"])
        if rank is not None:
            ranks.append(rank)
        rows.append({**question, "rank": rank, **result})
        print(
            f"{question['id']:<18} {('hit@%d' % rank) if rank else 'miss':<8} "
            f"{result.get('millis', 0):>7.0f} ms  {result.get('bytes', 0):>7} B  "
            f"n={len(result.get('urls') or []):>2}  {question['query']}"
        )
        if args.delay:
            time.sleep(args.delay)

    total = len(rows)
    hits5 = sum(1 for row in rows if row["rank"] is not None and row["rank"] <= 5)
    hits1 = sum(1 for row in rows if row["rank"] == 1)
    mrr = sum(1.0 / row["rank"] for row in rows if row["rank"] is not None) / total
    latencies = [row["millis"] for row in rows if "millis" in row and row["millis"] > 0]
    sizes = [row["bytes"] for row in rows if row.get("bytes")]
    report = {
        "base": args.base,
        "params": extra,
        "questions": total,
        "hit@1": hits1,
        "hit@5": hits5,
        "mrr@10": round(mrr, 3),
        "median_ms": round(statistics.median(latencies), 1) if latencies else 0,
        "median_bytes": int(statistics.median(sizes)) if sizes else 0,
        "errors": sum(1 for row in rows if row.get("error")),
        "rows": rows,
    }
    print()
    print(f"questions={total}  hit@1={hits1}/{total}  hit@5={hits5}/{total}  MRR@10={mrr:.3f}")
    print(f"median latency={report['median_ms']} ms  median response={report['median_bytes']} B  errors={report['errors']}")
    if args.json_path:
        Path(args.json_path).write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"wrote {args.json_path}")
    return 0 if report["errors"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())

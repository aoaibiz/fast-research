#!/usr/bin/env python3
"""Fail when a benchmark run is worse than the floor that proves it worked.

A nightly run compares badly across machines and network conditions, so this
only catches breakage (engine wiring, parsers, the search path itself) rather
than small regressions. Use the uploaded JSON for the trend.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("path")
    parser.add_argument("--floor-hit1", type=int, default=50)
    parser.add_argument("--max-errors", type=int, default=0)
    args = parser.parse_args()

    data = json.loads(Path(args.path).read_text(encoding="utf-8"))
    hit1 = int(data.get("hit@1", 0))
    errors = int(data.get("errors", 0))
    print(f"hit@1={hit1} hit@5={data.get('hit@5')} mrr={data.get('mrr@10')} errors={errors}")

    failed = False
    if hit1 < args.floor_hit1:
        print(f"::error::hit@1 {hit1} is below the floor {args.floor_hit1}")
        failed = True
    if errors > args.max_errors:
        print(f"::error::{errors} requests failed")
        failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

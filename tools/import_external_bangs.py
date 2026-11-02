#!/usr/bin/env python3
"""Flatten SearXNG's external bangs trie into a plain table.

Upstream ships searx/data/external_bangs.json, a compressed trie where every
node key is a substring of a bang and the leaf value is
"<url template>\\x01<rank>" (\\x02 is the query placeholder). The Rust port
stores the flattened result as TSV and loads it lazily.

Usage:
    python3 tools/import_external_bangs.py /path/to/searxng [output.tsv]
"""
import json
import sys
from pathlib import Path

LEAF_KEY = chr(16)
QUERY_KEY = chr(2)
SEPARATOR = chr(1)


def walk(node, prefix, out):
    if isinstance(node, str):
        out.append((prefix, node))
        return
    if not isinstance(node, dict):
        return
    leaf = node.get(LEAF_KEY)
    if isinstance(leaf, str):
        out.append((prefix, leaf))
    for key, child in node.items():
        if key == LEAF_KEY:
            continue
        walk(child, prefix + key, out)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    source = Path(sys.argv[1]) / "searx" / "data" / "external_bangs.json"
    target = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("src/data/external_bangs.tsv")
    data = json.loads(source.read_text(encoding="utf-8"))
    entries = []
    walk(data["trie"], "", entries)
    rows = []
    for bang, definition in entries:
        if SEPARATOR not in definition:
            continue
        url, _, rank = definition.partition(SEPARATOR)
        url = url.replace(QUERY_KEY, "{query}")
        if url.startswith("//"):
            url = "https:" + url
        try:
            rank_value = int(rank) if rank else 0
        except ValueError:
            rank_value = 0
        if not bang or not url:
            continue
        rows.append((bang, url, rank_value))
    rows.sort(key=lambda row: row[0])
    seen = set()
    unique = []
    for bang, url, rank in rows:
        if bang in seen:
            continue
        seen.add(bang)
        unique.append((bang, url, rank))
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("w", encoding="utf-8") as handle:
        handle.write("# bang\turl-template\trank — generated from SearXNG external_bangs.json\n")
        for bang, url, rank in unique:
            handle.write(f"{bang}\t{url}\t{rank}\n")
    print(f"wrote {target} with {len(unique)} bangs (from {len(entries)} trie leaves)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

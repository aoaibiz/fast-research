#!/usr/bin/env python3
"""Compare our engine coverage with SearXNG's settings.yml.

Usage: python3 tools/coverage.py /path/to/searxng [settings.toml]
"""
import re
import sys
from pathlib import Path

import yaml


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    upstream = yaml.safe_load(
        (Path(sys.argv[1]) / "searx" / "settings.yml").read_text(encoding="utf-8")
    )["engines"]
    settings = Path(sys.argv[2] if len(sys.argv) > 2 else "settings.toml").read_text(encoding="utf-8")
    ours = re.findall('''^\\[\\[engines\\]\\]\\nname = "([^"]+)"\\nengine = "([^"]+)"''', settings, flags=re.M)
    our_names = {name for name, _ in ours}
    our_kinds = {kind for _, kind in ours}

    kinds = {e.get("engine") for e in upstream if e.get("engine")}
    print(f"upstream engine entries: {len(upstream)}")
    print(f"upstream engine types:   {len(kinds)}")
    print(f"our engine entries:      {len(ours)}")
    print(f"our engine types:        {len(our_kinds)}")
    implemented_kinds = sorted(kinds & our_kinds)
    print(f"engine types implemented: {len(implemented_kinds)}")
    # entries we can serve: those whose engine type we implement
    servable = [e for e in upstream if e.get("engine") in our_kinds]
    print(f"upstream entries whose engine type is implemented: {len(servable)}")
    # entries we also have a settings entry for
    configured = [e for e in servable if e.get("name") in our_names]
    print(f"upstream entries with a settings entry here:       {len(configured)}")
    print()
    missing_kinds = sorted(kinds - our_kinds)
    print(f"engine types still missing ({len(missing_kinds)}):")
    for chunk in range(0, len(missing_kinds), 8):
        print("  " + ", ".join(missing_kinds[chunk : chunk + 8]))
    return 0


if __name__ == "__main__":
    sys.exit(main())

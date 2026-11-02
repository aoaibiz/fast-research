#!/usr/bin/env python3
"""After full engine rollout: verify every engine registered in settings.toml
builds (by starting the binary and reading /config), and report any that fail.
Requires cargo build --release done first.
"""
import json, subprocess, sys, time, urllib.error, urllib.request

ROOT = "/home/sahen/File/Prog/searxng-rust"
BIN = ROOT + "/target/release/rustxng"

def main():
    proc = subprocess.Popen([BIN, "--port", "8897", "--bind", "127.0.0.1"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(300):
            try:
                urllib.request.urlopen("http://127.0.0.1:8897/healthz", timeout=2)
                break
            except Exception:
                time.sleep(0.1)
        req = urllib.request.Request("http://127.0.0.1:8897/config", headers={"User-Agent": "probe"})
        data = json.load(urllib.request.urlopen(req, timeout=30))
        engines = data.get("engines", [])
        print("configured engines loaded:", len(engines))
        # each engine must appear with its name
        names = {e["name"] for e in engines}
        settings_names = []
        for line in open(ROOT + "/settings.toml", encoding="utf-8"):
            if line.strip().startswith("name = "):
                settings_names.append(line.split('"')[1])
        missing = [n for n in settings_names if n not in names]
        print("settings names without a built engine:", missing[:20] if missing else "none")
        return 0 if not missing else 1
    finally:
        proc.terminate()

if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env bash
# Measure startup time, warm search latency and resident memory of one instance.
#
#   bash bench/resources.sh rustxng http://127.0.0.1:8899
#   bash bench/resources.sh granian      http://127.0.0.1:8001
#
# The SearXNG side needs the checkout at /tmp/searxng-ref, the virtualenv at
# /tmp/sxng-venv and the bench settings at /tmp/sxng-bench-settings.yml
# (see docs/BENCHMARK.md). The granian supervisor renames itself to
# "granian wsgi <version>", hence the two different process matchers.
set -u
NAME="$1"
BASE="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ "$NAME" = "rustxng" ]; then
  kill_all() { pkill -x rustxng 2>/dev/null || true; }
  alive() { pgrep -x rustxng >/dev/null 2>&1; }
  list_pids() { pgrep -x rustxng; }
else
  kill_all() { pkill -f "granian wsgi" 2>/dev/null || true; }
  alive() { pgrep -f "granian wsgi" >/dev/null 2>&1; }
  list_pids() { pgrep -f "granian wsgi"; }
fi

kill_all
for _ in $(seq 1 200); do
  if ! alive; then break; fi
  sleep 0.05
done

START="$(date +%s.%N)"
if [ "$NAME" = "rustxng" ]; then
  (cd "$ROOT" && exec ./target/release/rustxng --port 8899) >/tmp/bench-start.log 2>&1 &
else
  (cd /tmp/searxng-ref && SEARXNG_SETTINGS_PATH=/tmp/sxng-bench-settings.yml \
    exec /tmp/sxng-venv/bin/granian --interface wsgi --host 127.0.0.1 --port 8001 searx.webapp:app) \
    >/tmp/searxng-server.log 2>&1 &
fi

READY=""
for _ in $(seq 1 3000); do
  if curl -s -m 1 -o /dev/null "$BASE/healthz"; then READY="$(date +%s.%N)"; break; fi
  sleep 0.01
done
if [ -z "$READY" ]; then
  echo "$NAME: did not become ready"
  exit 1
fi
READY_SECONDS="$(python3 -c "print(f'{$READY - $START:.3f}')")"
WARM="$(curl -s -m 60 -o /dev/null -w '%{time_total}' "$BASE/search?q=rust&format=json" || echo 0)"

PIDS="$(list_pids | tr '\n' ' ')"
COUNT="$(echo $PIDS | wc -w)"
MAX_RSS=0
MAX_PID=""
for pid in $PIDS; do
  rss="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')"
  if [ -n "$rss" ] && [ "$rss" -gt "$MAX_RSS" ]; then
    MAX_RSS="$rss"
    MAX_PID="$pid"
  fi
done
THREADS="$(ps -o nlwp= -p "$MAX_PID" 2>/dev/null | tr -d ' ')"
printf '%s: startup=%ss warm_search=%ss processes=%s max_rss=%sMiB threads=%s\n' \
  "$NAME" "$READY_SECONDS" "$WARM" "$COUNT" "$((MAX_RSS / 1024))" "$THREADS"

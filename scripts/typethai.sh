#!/bin/bash
# Build and run the typeThai desktop app from this checkout (macOS).
# usage: typethai [start|stop|status|log|fg]   (default: start)
#   start   build (release, incremental), stop any running copy, run in the background
#   stop    stop every running typethai-desktop (the dev build and /Applications/typeThai.app)
#   status  show whether it is running
#   log     follow the log
#   fg      build and run in the foreground (Ctrl+C stops it)
# Model: $TYPETHAI_MODEL, else the installed app's, else /tmp/ttmodel. Extra args after the command go to the app.
set -euo pipefail
root=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
bin="$root/rust/target/release/typethai-desktop"
log="$HOME/Library/Logs/typethai.log"

model() {
  for m in "${TYPETHAI_MODEL:-}" "/Applications/typeThai.app/Contents/Resources/typethai.ttm" "/tmp/ttmodel/typethai.ttm"; do
    [ -n "$m" ] && [ -e "$m" ] && { echo "$m"; return; }
  done
  echo "typethai: no model; set TYPETHAI_MODEL to a typethai.ttm" >&2; exit 1
}
build() { (cd "$root/rust" && cargo build --release -q -p typethai-desktop); }
stop() { pkill -x typethai-desktop && sleep 0.3 || true; }

cmd=${1:-start}; [ $# -gt 0 ] && shift
case $cmd in
  start)
    m=$(model); build; stop
    mkdir -p "$(dirname "$log")"
    nohup "$bin" --model "$m" "$@" >>"$log" 2>&1 &
    echo "typeThai started (pid $!, model $m, log $log)" ;;
  fg) m=$(model); build; stop; exec "$bin" --model "$m" "$@" ;;
  stop) stop; echo "typeThai stopped" ;;
  status) pgrep -lx typethai-desktop || echo "typeThai is not running" ;;
  log) exec tail -f "$log" ;;
  *) sed -n '3,9p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac

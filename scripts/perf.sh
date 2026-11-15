#!/usr/bin/env bash
# Mesures de performance de Lapinux (daemon au repos, latence IPC, scan, taille).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${LAPINUX_BIN:-$ROOT/target/debug/lapinux}
TMP=$(mktemp -d)
export LAPINUX_HOME="$TMP"
PID=""
cleanup() {
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

if [ ! -x "$BIN" ]; then
  echo "binaire absent : $BIN (cargo build d'abord)" >&2
  exit 1
fi

echo "Lapinux — rapport de performances"
echo "binaire : $BIN"
printf 'taille   : %s Kio\n' "$(( $(stat -c%s "$BIN") / 1024 ))"

# Démarrage du daemon isolé (LAPINUX_HOME temporaire).
"$BIN" --daemon >"$TMP/daemon.log" 2>&1 &
PID=$!
SOCKET="$TMP/run/control.sock"
for _ in $(seq 1 200); do
  [ -S "$SOCKET" ] && break
  sleep 0.02
done
if [ ! -S "$SOCKET" ]; then
  echo "le daemon n'a pas démarré" >&2
  cat "$TMP/daemon.log" >&2
  exit 1
fi

# Laisse le scan initial se terminer.
sleep 3

rss=$(awk '/VmRSS/ {print $2}' "/proc/$PID/status")
ticks_start=$(awk '{print $14+$15}' "/proc/$PID/stat")
sleep 5
ticks_end=$(awk '{print $14+$15}' "/proc/$PID/stat")
hz=$(getconf CLK_TCK)
cpu=$(awk -v d="$((ticks_end - ticks_start))" -v h="$hz" 'BEGIN {printf "%.2f", (d / h) / 5 * 100}')

# Latence d'un aller-retour IPC (10 mesures).
total_us=0
for _ in $(seq 1 10); do
  start=$(date +%s%N)
  "$BIN" cli session >/dev/null 2>&1 || true
  end=$(date +%s%N)
  total_us=$((total_us + (end - start) / 1000))
done
ipc_us=$((total_us / 10))

# Scan réel des bibliothèques.
scan_start=$(date +%s%N)
"$BIN" cli scan --json >"$TMP/scan.json"
scan_end=$(date +%s%N)
scan_ms=$(( (scan_end - scan_start) / 1000000 ))
games=$(python3 - "$TMP/scan.json" <<'PY'
import json, sys
print(len(json.load(open(sys.argv[1]))))
PY
)

printf 'daemon  : %s Kio RSS, %s%% CPU au repos (5 s)\n' "$rss" "$cpu"
printf 'IPC     : %s µs par aller-retour\n' "$ipc_us"
printf 'scan    : %s ms pour %s jeux\n' "$scan_ms" "$games"

status=0
check() {
  local label=$1 value=$2 limit=$3
  if awk -v v="$value" -v l="$limit" 'BEGIN {exit !(v > l)}'; then
    printf 'ÉCHEC   : %s = %s (limite %s)\n' "$label" "$value" "$limit"
    status=1
  else
    printf 'OK      : %s = %s (limite %s)\n' "$label" "$value" "$limit"
  fi
}

check "RSS daemon" "$rss" 60000
check "CPU daemon" "$cpu" 1.0
check "latence IPC" "$ipc_us" 20000
check "scan" "$scan_ms" 20000

exit "$status"

#!/usr/bin/env bash
# Vérifications réelles sur la machine : bibliothèque, Protons, entrées, overlay.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${LAPINUX_BIN:-$ROOT/target/debug/lapinux}

if [ ! -x "$BIN" ]; then
  echo "binaire absent : $BIN" >&2
  exit 1
fi

step() { printf '\n=== %s ===\n' "$1"; }

step "diagnostic"
"$BIN" cli doctor

step "bibliothèque Steam"
"$BIN" cli scan --json > /tmp/lapinux-scan.json
python3 - <<'PY'
import json
games = json.load(open('/tmp/lapinux-scan.json'))
installed = [g for g in games if g['installed']]
print(f"{len(games)} jeux, {len(installed)} installés")
for g in installed[:5]:
    print(f"  {g['appid']:>8}  {g['name'][:38]:<38} {g['drm']}")
PY

step "Protons"
"$BIN" cli protons | head -8

step "périphériques evdev"
"$BIN" cli devices | head -6

step "pad virtuel uinput"
"$BIN" cli virtual-pad

step "lancement (dry-run)"
APPID=$(python3 - <<'PY'
import json
games = json.load(open('/tmp/lapinux-scan.json'))
candidates = [g for g in games if g['installed'] and g.get('exe') and not g.get('native')]
print(candidates[0]['appid'] if candidates else '')
PY
)
if [ -n "$APPID" ]; then
  "$BIN" cli launch "$APPID" --dry-run
else
  echo "aucun jeu Windows installé pour le dry-run"
fi

step "overlay layer-shell (1 s)"
"$BIN" cli overlay-demo --seconds 1

echo
echo "smoke test terminé"

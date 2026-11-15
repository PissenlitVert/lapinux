#!/usr/bin/env bash
# Vérifie les liens relatifs et les ancres des pages publiques.
#
#   ./scripts/check-docs.sh
#
# Contrôle README.md et docs/*.md : chaque lien relatif doit exister, et
# chaque `#ancre` doit correspondre à un titre de la page visée. Les liens
# externes ne sont pas visités (pas de réseau en CI).
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

python3 - <<'PY'
import re
import sys
from pathlib import Path

def slug(heading: str) -> str:
    """Ancre GitHub : minuscules, ponctuation retirée, espaces en tirets."""
    text = heading.strip().lower()
    text = re.sub(r"`", "", text)
    text = re.sub(r"[^\w\s-]", "", text, flags=re.UNICODE)
    return re.sub(r"\s+", "-", text).strip("-")

files = [Path("README.md")] + sorted(Path("docs").glob("*.md"))
errors = []
checked = 0

for page in files:
    text = page.read_text(encoding="utf-8")
    for match in re.finditer(r"\]\(([^)\s]+)\)", text):
        target = match.group(1)
        checked += 1
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        path_part, _, anchor = target.partition("#")
        destination = (page.parent / path_part).resolve()
        if not destination.exists():
            errors.append(f"{page}: lien absent -> {target}")
            continue
        if anchor and destination.suffix == ".md":
            headings = [
                slug(line.lstrip("#").strip())
                for line in destination.read_text(encoding="utf-8").splitlines()
                if line.startswith("#")
            ]
            if anchor not in headings:
                errors.append(f"{page}: ancre absente -> {target}")

if errors:
    print("Liens de documentation cassés :", file=sys.stderr)
    for error in errors:
        print(f"  {error}", file=sys.stderr)
    sys.exit(1)

print(f"Documentation : {checked} liens vérifiés dans {len(files)} pages.")
PY

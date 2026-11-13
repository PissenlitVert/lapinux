#!/usr/bin/env python3
"""Validate every configuration and build the published catalog.

Usage, from the repository root:

    python3 tools/catalog.py            # validate, then write catalog.json
    python3 tools/catalog.py --check    # validate, fail if catalog.json is stale

Only the Python standard library is used, so the CI job needs no install step.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CATALOG = ROOT / "catalog.json"
ENTRIES = ROOT / "configurations"
EXAMPLES = ROOT / "examples"

CATALOG_VERSION = 2
MAINTAINER = {"name": "Pissenlit Vert", "github": "PissenlitVert"}

TYPES = {
    "graphics",
    "performance",
    "display",
    "language",
    "compatibility",
    "input",
    "content",
}
PLATFORMS = {"windows", "linux", "both"}
LAYERS = {"dxvk", "vkd3d", "wine", "proton", "gamescope", "game"}
STATUSES = {"proposed", "verified", "broken"}
TOP_KEYS = {
    "id",
    "title",
    "description",
    "solution",
    "type",
    "platform",
    "layers",
    "language",
    "author",
    "version",
    "license",
    "status",
    "tested",
    "game",
    "requires",
    "conflicts",
    "retired",
    "actions",
    "links",
}
SLUG = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SHA256 = re.compile(r"^[0-9a-fA-F]{64}$")
VERSION = re.compile(r"^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")


class Problems:
    def __init__(self) -> None:
        self.items: list[str] = []

    def add(self, where: str, message: str) -> None:
        self.items.append(f"{where}: {message}")

    def __bool__(self) -> bool:
        return bool(self.items)


def relative_path(text: object) -> str | None:
    """Return the reason a path is unsafe, or None when it is fine."""
    if not isinstance(text, str) or not text.strip():
        return "empty path"
    raw = text.strip()
    if raw.startswith("/"):
        return "absolute path"
    if "\\" in raw:
        return "Windows separator (use /)"
    if "\0" in raw:
        return "NUL byte"
    if ":" in raw:
        return "colon (Windows drive)"
    parts = raw.split("/")
    if any(part in ("", ".", "..") for part in parts):
        return "empty, '.' or '..' segment"
    return None


def check_manifest(entry: dict, folder: Path, problems: Problems) -> int:
    """Validate one manifest. Returns the total size of its files."""
    where = str(folder.relative_to(ROOT))
    unknown = set(entry) - TOP_KEYS
    if unknown:
        problems.add(where, f"unknown keys: {', '.join(sorted(unknown))}")
    for key in ("id", "title", "description", "solution", "type", "platform", "version", "license"):
        if not isinstance(entry.get(key), str) or not entry[key].strip():
            problems.add(where, f"{key} is required")
    ident = entry.get("id")
    if isinstance(ident, str) and ident:
        if not SLUG.match(ident):
            problems.add(where, "id must be lowercase, digits and dashes only")
    if entry.get("type") not in TYPES:
        problems.add(where, f"type must be one of: {', '.join(sorted(TYPES))}")
    if entry.get("platform") not in PLATFORMS:
        problems.add(where, f"platform must be one of: {', '.join(sorted(PLATFORMS))}")
    version = entry.get("version")
    if isinstance(version, str) and version and not VERSION.match(version):
        problems.add(where, "version must look like 1.2.3")
    status = entry.get("status", "proposed")
    if status not in STATUSES:
        problems.add(where, f"status must be one of: {', '.join(sorted(STATUSES))}")

    layers = entry.get("layers", [])
    if not isinstance(layers, list):
        problems.add(where, "layers must be a list")
    else:
        for layer in layers:
            if layer not in LAYERS:
                problems.add(where, f"unknown layer '{layer}'")

    author = entry.get("author", {})
    if not isinstance(author, dict) or not str(author.get("name", "")).strip():
        problems.add(where, "author.name is required")
    elif author.get("github") and not re.match(r"^[A-Za-z0-9-]+$", str(author["github"])):
        problems.add(where, "author.github must be a GitHub handle, without @")

    game = entry.get("game", {})
    if not isinstance(game, dict) or not game.get("names"):
        problems.add(where, "game.names needs at least one name")
    else:
        for appid in game.get("steam_appids", []):
            if not isinstance(appid, int) or appid <= 0:
                problems.add(where, "game.steam_appids must be positive integers")
        for store_id in game.get("store_ids", []):
            if not isinstance(store_id, str) or not re.match(r"^(epic|gog|itch):.+", store_id):
                problems.add(where, "game.store_ids must look like epic:<appName>, gog:<id> or itch:<slug>")

    for key in ("requires", "conflicts"):
        values = entry.get(key, [])
        if not isinstance(values, list):
            problems.add(where, f"{key} must be a list")
            continue
        if ident in values:
            problems.add(where, f"{key} cannot reference the configuration itself")

    actions = entry.get("actions", {})
    if not isinstance(actions, dict):
        problems.add(where, "actions must be an object")
        return 0
    settings = actions.get("settings", {}) or {}
    if not isinstance(settings, dict):
        problems.add(where, "actions.settings must be an object")
        settings = {}
    env = settings.get("env", [])
    if not isinstance(env, list):
        problems.add(where, "actions.settings.env must be a list of [NAME, value]")
    else:
        for pair in env:
            if (
                not isinstance(pair, list)
                or len(pair) != 2
                or not isinstance(pair[0], str)
                or not isinstance(pair[1], str)
                or not pair[0].strip()
            ):
                problems.add(where, "actions.settings.env must be a list of [NAME, value]")
    args = settings.get("args", [])
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
        problems.add(where, "actions.settings.args must be a list of strings")
    dxvk = settings.get("dxvk", {}) or {}
    if not isinstance(dxvk, dict):
        problems.add(where, "actions.settings.dxvk must be an object")
    else:
        for key in ("shared_mb", "device_mb"):
            value = dxvk.get(key)
            if value is not None and (not isinstance(value, int) or value <= 0):
                problems.add(where, f"actions.settings.dxvk.{key} must be a positive integer")
        if dxvk.get("device") is not None and not isinstance(dxvk["device"], str):
            problems.add(where, "actions.settings.dxvk.device must be a string")

    files = actions.get("files", [])
    if not isinstance(files, list):
        problems.add(where, "actions.files must be a list")
        files = []
    if not env and not args and not dxvk and not files:
        problems.add(where, "actions: nothing to apply (settings or files)")
    total = 0
    seen: set[str] = set()
    for index, item in enumerate(files):
        spot = f"{where} files[{index}]"
        if not isinstance(item, dict):
            problems.add(spot, "must be an object")
            continue
        source = item.get("source")
        target = item.get("target")
        digest = item.get("sha256")
        for label, value in (("source", source), ("target", target)):
            reason = relative_path(value)
            if reason:
                problems.add(spot, f"{label}: {reason}")
        if isinstance(target, str):
            if target in seen:
                problems.add(spot, f"target '{target}' is declared twice")
            seen.add(target)
        if not isinstance(digest, str) or not SHA256.match(digest):
            problems.add(spot, "sha256 must be 64 hexadecimal characters")
            continue
        if not isinstance(source, str):
            continue
        path = folder / source
        if not path.is_file():
            problems.add(spot, f"file not found: {source}")
            continue
        data = path.read_bytes()
        actual = hashlib.sha256(data).hexdigest()
        if actual.lower() != digest.lower():
            problems.add(spot, f"sha256 mismatch: published file is {actual}")
        total += len(data)
        if len(data) > 20 * 1024 * 1024:
            problems.add(spot, "file is larger than 20 MiB")
    if total > 100 * 1024 * 1024:
        problems.add(where, "configuration is larger than 100 MiB")

    for link in entry.get("links", []):
        if not isinstance(link, str) or not link.startswith("https://"):
            problems.add(where, "links must be https:// URLs")
    return total


def load(path: Path, problems: Problems) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        problems.add(str(path.relative_to(ROOT)), f"invalid JSON: {error}")
        return {}


def collect(folder: Path, published: bool, problems: Problems) -> list[dict]:
    entries: list[dict] = []
    if not folder.is_dir():
        return entries
    for game in sorted(path for path in folder.iterdir() if path.is_dir()):
        for config in sorted(path for path in game.iterdir() if path.is_dir()):
            manifest = config / "manifest.json"
            if not manifest.is_file():
                if published:
                    problems.add(str(config.relative_to(ROOT)), "missing manifest.json")
                continue
            entry = load(manifest, problems)
            if not entry:
                continue
            size = check_manifest(entry, config, problems)
            if published:
                base = config.relative_to(ROOT).as_posix()
                entry = dict(entry)
                entry["size"] = size
                entry["base"] = base
                entries.append(entry)
    return entries


def build(problems: Problems) -> dict:
    entries = collect(ENTRIES, True, problems)
    collect(EXAMPLES, False, problems)
    ids = [entry.get("id") for entry in entries]
    for ident in sorted({ident for ident in ids if ids.count(ident) > 1}):
        problems.add(str(ENTRIES.relative_to(ROOT)), f"duplicate id '{ident}'")
    entries.sort(key=lambda entry: entry["base"])
    return {
        "version": CATALOG_VERSION,
        "maintainer": MAINTAINER,
        "configurations": entries,
    }


def main(argv: list[str]) -> int:
    check = "--check" in argv
    problems = Problems()
    catalog = build(problems)
    text = json.dumps(catalog, ensure_ascii=False, indent=2) + "\n"
    if check:
        current = CATALOG.read_text(encoding="utf-8") if CATALOG.is_file() else ""
        if current != text:
            problems.add("catalog.json", "out of date: run python3 tools/catalog.py")
    if problems:
        for problem in problems.items:
            print(f"error: {problem}", file=sys.stderr)
        return 1
    if not check:
        CATALOG.write_text(text, encoding="utf-8")
        print(f"catalog.json: {len(catalog['configurations'])} configuration(s)")
    else:
        print(f"catalog.json: {len(catalog['configurations'])} configuration(s), up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

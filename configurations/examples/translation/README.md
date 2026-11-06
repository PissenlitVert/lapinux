# Translation example (not published)

This folder is a template for a `language` configuration. It is deliberately
**outside** `configurations/`: the catalog builder ignores `examples/`, so this
entry never reaches Lapinux users. Copy it, point it at a real game, then move
it under `configurations/<game>/<translation>/`.

## What it shows

- `"type": "language"` and `"language": "fr"`: the game sheet shows a
  **Langue** badge and the author gets credited;
- one file action: `files/fr.txt` is copied over `lang/fr.txt` inside the
  game's install folder;
- `"backup": true` (the default): the file that was already there is kept
  before being replaced, so **Remove** restores it exactly;
- `sha256`: the digest of `files/fr.txt`. Lapinux refuses to install the file
  if the published bytes do not match, so update the hash every time the file
  changes.

## Turning it into a real translation

1. Copy this folder to `configurations/<game-slug>/<language-code>/`.
2. Change `id` (for example `hollow-knight-french`), `title`, `description`,
   `solution` and `language`.
3. Replace `files/fr.txt` with the file the game actually loads. Keep every
   key the game expects; translate only the values.
4. Update `game.names` and, when you know them, `steam_appids` or `store_ids`
   (`epic:<appName>`, `gog:<product id>`, `itch:<slug>`).
5. Set `license` to the licence of **your** translation. CC0-1.0 is
   recommended so anyone can reuse it; do not publish files you do not own or
   have no permission to redistribute.
6. Compute the digest: `sha256sum files/fr.txt`.
7. Run `python3 tools/catalog.py` from the repository root, then commit both
   your folder and the updated `catalog.json`.
8. Test in Lapinux: install, play, remove. Removing must restore the original
   language file.

## Rules

- Never ship game assets you did not create: only text files, and only if the
  game's licence or the rights holder allows redistribution.
- One configuration per language. Do not bundle several languages in one file
  action unless the game loads them from one file.
- If a translation stops working after a game update, bump the version, fix the
  file and update `tested`.

# Lapinux Configurations

A community catalog of small, reversible fixes and additions for games played
with **Lapinux** — the Linux game library that runs Steam, Epic, GOG and
itch.io games under Proton.

A configuration can:

- fix a game that misreads the GPU, a Proton feature or a display mode;
- add or correct a translation;
- replace a data file, a texture set or a mod package.

It cannot run a script. Every entry is **declarative**: Lapinux applies game
settings and copies files, and can undo both.

Maintained by **Pissenlit Vert** ([@PissenlitVert](https://github.com/PissenlitVert)).

---

## Contents

1. [How Lapinux uses this repository](#how-lapinux-uses-this-repository)
2. [Repository layout](#repository-layout)
3. [Quick start: publish a fix](#quick-start-publish-a-fix)
4. [Manifest reference](#manifest-reference)
5. [Actions in detail](#actions-in-detail)
6. [Translations](#translations)
7. [Mods and data files](#mods-and-data-files)
8. [Themes](#themes)
9. [Testing your configuration](#testing-your-configuration)
10. [Versions, updates and retirement](#versions-updates-and-retirement)
11. [Safety rules](#safety-rules)
12. [Building the catalog and CI](#building-the-catalog-and-ci)
13. [Pull request checklist](#pull-request-checklist)
14. [FAQ](#faq)
15. [License](#license)

---

## How Lapinux uses this repository

1. At startup, and at most every six hours, Lapinux downloads
   [`catalog.json`](catalog.json) from `main`. The file lists every published
   configuration; it is a few kilobytes.
2. If GitHub is unreachable, the copy shipped inside the Lapinux binary is
   used. A known fix therefore stays available offline.
3. When a game sheet is opened, the entries whose `game` block matches that
   game are shown in the **Configurations** tab.
4. **Install this configuration** asks the daemon to:
   - verify the manifest and the game state (installed, not running);
   - download each declared file from `raw.githubusercontent.com` and check
     its SHA-256;
   - back up every file it is about to replace;
   - apply the settings patch and write the files atomically;
   - record what was done, so **Remove** restores the previous state.
5. Nothing is downloaded or written until the player clicks **Install**.

The catalog URL can be overridden for development:

```bash
LAPINUX_CONFIGURATIONS_CATALOG=http://127.0.0.1:8000/catalog.json lapinux
```

Any HTTP server works; `catalog.json` and the entry folders must be reachable
under the same base URL.

### How a game is recognised

A configuration applies to a game when **any** of these match:

- `game.steam_appids` contains the game's Steam AppID (Steam games only);
- `game.store_ids` contains `epic:<appName>`, `gog:<product id>` or
  `itch:<slug>` for the matching store;
- `game.names` or `game.aliases` equals the game's name, ignoring case,
  accents, punctuation and trademark symbols.

Prefer an identifier over a name: names change with localisation and editions.
Store identifiers are the most reliable for non-Steam games.

---

## Repository layout

```
.
├── catalog.json                     # generated; do not edit by hand
├── configurations/
│   └── <game-slug>/
│       └── <variant-slug>/
│           ├── manifest.json        # the configuration
│           ├── README.md            # what it fixes, how it was tested
│           └── files/…              # optional files copied into the game
├── examples/
│   └── translation/                 # template, never published
├── schema/
│   ├── configuration.schema.json
│   └── catalog.schema.json
└── tools/
    └── catalog.py                   # validator and catalog builder
```

`configurations/<game-slug>/<variant-slug>/` is the **base folder** of an
entry. Its path becomes the `base` field in `catalog.json`, and file URLs are
resolved from there. Use one folder per game and one sub-folder per variant,
for example:

```
configurations/example-game/performance/
configurations/hollow-knight/french/
configurations/hollow-knight/hd-textures/
```

Folder names are lowercase, digits and dashes. They do not have to equal
`id`, but keeping them close makes the catalog readable.

---

## Quick start: publish a fix

1. **Fork** this repository and create a branch.
2. Copy the closest existing entry, or start from
   [`examples/translation`](examples/translation).
3. Put it under `configurations/<game>/<variant>/manifest.json`.
4. Describe the problem in `description`, the change in `solution`, and set
   `game`, `author`, `version`, `license` and `tested`.
5. Run the validator:

   ```bash
   python3 tools/catalog.py
   ```

   It rewrites `catalog.json`, checks hashes and prints every problem.
6. Install the catalog locally in Lapinux (see
   [Testing](#testing-your-configuration)), then install your configuration
   from the game sheet.
7. Play, remove the configuration, check the game is back to its previous
   state.
8. Open a pull request. Include the game, the symptom, the result and the
   Proton version you used.

Start from the complete translation template in
[`examples/translation`](examples/translation): it shows every required field,
the optional files and the README expected for a published entry.

---

## Manifest reference

A manifest is a single JSON object. Unknown fields are rejected by the
validator; the JSON Schema in [`schema/`](schema) documents types and limits.

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Globally unique slug. Must match `^[a-z0-9]+(-[a-z0-9]+)*$`. Never reuse it for another fix. |
| `title` | yes | One line shown on the card. |
| `description` | yes | The problem, from the player's point of view. |
| `solution` | yes | What the configuration changes, and why. |
| `type` | yes | `graphics`, `performance`, `display`, `language`, `compatibility`, `input` or `content`. |
| `platform` | yes | `windows` (Proton), `linux` (native) or `both`. |
| `layers` | no | `dxvk`, `vkd3d`, `wine`, `proton`, `gamescope`, `game`. Informational. |
| `language` | no | ISO code for a translation (`fr`, `pt-BR`). |
| `author` | yes | `{ "name": "...", "github": "..." }`. The GitHub handle becomes a link in the app. |
| `version` | yes | Semantic version. Bump it on **every** content change. |
| `license` | yes | SPDX id of the content. `CC0-1.0` is recommended. |
| `status` | no | `proposed` (default), `verified` or `broken`. `broken` cannot be installed. |
| `tested` | no | `proton`, `gpu`, `ram`, `distro`, `date` (`YYYY-MM-DD`). Shown in **Details**. |
| `game` | yes | Names, aliases, Steam AppIDs and store identifiers. |
| `requires` | no | Ids that must be installed first. |
| `conflicts` | no | Ids that cannot be installed at the same time. |
| `retired` | no | `true`: no longer offered, still removable by players who have it. |
| `actions` | yes | Settings and/or files. At least one action is required. |
| `links` | no | `https://` documentation or issue links. |

`status` is a promise, not decoration:

- `proposed`: the author believes it works; nobody reproduced it yet.
- `verified`: the author reproduced the result on the hardware listed in
  `tested`. Say what changed (FPS, crash, missing text) — numbers help.
- `broken`: known not to work after an update. Lapinux refuses to install it
  but players can still remove it.

---

## Actions in detail

### Settings

```json
"actions": {
  "settings": {
    "env": [["DXVK_ASYNC", "1"]],
    "dxvk": { "shared_mb": 1024, "device_mb": 8192, "device": "NVIDIA GeForce RTX 4070" },
    "args": ["-novid"]
  }
}
```

| Key | Effect |
| --- | --- |
| `env` | Environment variables passed to the game process. `[NAME, value]` pairs, applied after the player's own values. |
| `dxvk.shared_mb` | `dxgi.maxSharedMemory`: maximum shared memory DXVK reports to the engine, in MiB. |
| `dxvk.device_mb` | `dxgi.maxDeviceMemory`: maximum dedicated memory reported, in MiB. DXVK can only lower the real value. |
| `dxvk.device` | `dxgi.customDeviceDesc`: GPU name reported to the engine, 64 characters at most. |
| `args` | Arguments appended to the game's command line. |

Settings are applied when the game is launched, not while it runs. Lapinux
keeps the previous value of every key it touches; removing the configuration
restores it unless the player changed it in the meantime.

Do not put a value you do not understand in `env`: environment variables are
inherited by the game and by its child processes.

### Files

```json
"files": [
  {
    "source": "files/fr.txt",
    "target": "lang/fr.txt",
    "sha256": "686ede2039aa785bc349ded15888b0b07e635abdcbf3d2ed05659a24c810940d",
    "backup": true
  }
]
```

| Key | Meaning |
| --- | --- |
| `source` | Path inside the entry folder. Must exist in the repository. |
| `target` | Path inside the game's install folder. Relative, no `..`, no `\`, no drive letter, no absolute path. |
| `sha256` | Digest of `source`. Lapinux downloads the published file and refuses it if the digest differs. |
| `backup` | Default `true`: the file being replaced is kept, and **Remove** puts it back. Set `false` only for a file the configuration creates from scratch. |

Limits: 20 MiB per file, 100 MiB per configuration. Above that, publish the
content elsewhere and link to it — a catalog entry must stay cheap to download.

Paths are resolved inside the game folder: a target cannot escape it. Files
that do not exist yet are created with their parent folders. A file modified by
the player after installation is **kept** on removal; Lapinux never overwrites
a player's edit silently.

The whole installation is transactional: if a file cannot be written, the
files already written are rolled back, the settings are restored, and nothing
is recorded.

---

## Translations

Translations are `language` configurations that replace the file the game
loads. Start from [`examples/translation`](examples/translation): it is a
working template that the catalog builder ignores.

1. Find the file the game reads for your language. Keep a copy of the original
   before installing anything.
2. Put your translated file in `files/`.
3. Set `type` to `language`, `language` to the code, and declare the file with
   `backup: true`.
4. Translate only the values; keep every key and placeholder exactly as the
   game expects (`%s`, `{0}`, `\n`…).
5. Test a full playthrough of menus, dialogues and subtitles. A misplaced
   placeholder can crash the game.
6. Update `sha256` after every change: `sha256sum files/fr.txt`.

Do not publish machine-translated text without reviewing it, and do not
redistribute a game's original files. A translation must contain only your own
text (or text you have the right to redistribute) and must be licensed
accordingly.

If the game has no French font coverage, a translation may need a font file.
That font must be redistributable (SIL OFL, CC0…), and its license must be
mentioned in the entry README.

---

## Mods and data files

The same `files` mechanism handles texture packs, configuration files and mod
packages:

- prefer one file per variant; avoid overriding a file another mod also
  overrides, and use `conflicts` to say so;
- keep the mod's license and author in `license` and `author` — do not
  relicense someone else's work;
- a mod that needs a script, an injector or a DLL loader cannot be expressed
  here. Lapinux does not run code from a catalog.

---

## Themes

A theme is a declarative TOML manifest: a palette, a layout (`classic` or
`nx`), a rounding, a font scale and two home texts. It carries no image, no
script and no game action. Lapinux validates it before writing it to
`~/.config/lapinux/themes/`; the player then picks it in Settings → Appearance.

Themes are published in `catalog.json` under `themes`, each with the path of
its manifest (`themes/<id>.toml`), its SHA-256 and its size. The NX theme is
published this way: it is experimental, is not compiled into Lapinux, and only
exists once installed from the catalog.

To add or change a theme, edit `themes/<id>.toml` (with `id`, `name`,
`description`, `author`, `version`, `license`, `status` and the theme fields),
then run `python3 tools/catalog.py` so the digest and size follow the file.

## Testing your configuration

A catalog served locally is enough; nothing needs to be published first.

```bash
cd /path/to/lapinux-configurations
python3 -m http.server 8000
```

In another terminal:

```bash
LAPINUX_CONFIGURATIONS_CATALOG=http://127.0.0.1:8000/catalog.json lapinux
```

Then open the game sheet → **Configurations** → **Install this
configuration**. Check, in this order:

1. the entry appears for the right game, with your name and category;
2. installation succeeds, and the game actually behaves differently;
3. **Remove** succeeds, and the game is back to its original state
   (file restored, setting restored);
4. installing twice, or installing while the game runs, gives a clean error;
5. a wrong `sha256` is refused (change one byte of the source file);
6. a `target` with `..` or an absolute path is refused.

For a file entry, verify the backup exists in
`~/.local/share/lapinux/configurations/<appid>/<id>/backup/` while installed,
and disappears after removal.

Run `cargo test -p lapinux-core configurations` in the Lapinux repository if
you change the catalog format itself.

---

## Versions, updates and retirement

- **Update**: edit the manifest, bump `version`, update `sha256` values and
  `tested`, then run the builder. Lapinux shows **Update** when the installed
  version differs from the catalog.
- **Retire**: set `"retired": true`. The entry disappears from the catalog but
  players who installed it can still remove it. Never delete a folder that has
  been published: installed entries point at it for removal.
- **Break**: set `"status": "broken"` and explain why in `description`. Do not
  silently change what an installed version does.
- **Deprecate**: choose a new `id` rather than changing the meaning of an old
  one.

---

## Safety rules

- No executable, script, DLL, `.exe`, `.so`, `.bat`, `.sh` or installer in
  `files/`. The validator does not scan file types; reviewers do. A
  configuration is data, never code.
- No game assets you do not own or have no permission to redistribute. A
  texture pack you made from scratch is fine; extracted game files are not.
- No credential, token or personal data in a manifest, a README or a file.
- No `target` outside the game folder: the daemon rejects `..`, absolute paths,
  backslashes and drive letters, but a malicious manifest should never reach
  review in the first place.
- `links` must be `https://`.
- A manifest that changes settings must say what it changes and how to undo it
  in its README.
- Report a broken entry by opening an issue; do not repurpose its `id`.

---

## Building the catalog and CI

`catalog.json` is **generated**. It contains every entry under
`configurations/`, sorted by base path, with two added fields:

- `base`: the entry folder, used to resolve `source` URLs;
- `size`: the total size of the entry's files.

```bash
python3 tools/catalog.py          # validate and rewrite catalog.json
python3 tools/catalog.py --check  # validate and fail if catalog.json is stale
```

The script uses only the Python standard library. It checks JSON syntax,
required fields, enums, identifiers, versions, paths, file existence, SHA-256
digests, sizes, duplicate ids and `requires`/`conflicts` self-references. It
also validates `examples/`, without publishing them.

The catalog repository runs the same check on every push and pull request. A
pull request that forgets to regenerate `catalog.json` fails. The entry folders
live in that repository: the Lapinux tree only ships the schema, the validator,
the example and the embedded `catalog.json`.

The Lapinux binary embeds its own copy of `catalog.json` at build time; the
network catalog replaces it at runtime, entry by entry. A published entry with
the same `id` wins, which is how a fix reaches players without a Lapinux
release.

---

## Pull request checklist

- [ ] Folder under `configurations/<game>/<variant>/`, lowercase and dashed.
- [ ] `id` unique, stable, and not reused from another entry.
- [ ] `description` states the symptom; `solution` states the change.
- [ ] `author` is you; `license` matches the content.
- [ ] `game` has an identifier when one exists; names alone are fragile.
- [ ] `tested` filled with real hardware and a date; `status` set honestly.
- [ ] File digests recomputed, `catalog.json` regenerated.
- [ ] README explains the fix and how to undo it.
- [ ] Installed, played, removed, game restored.
- [ ] No executable, no redistributed game asset, no secret.

---

## FAQ

**My configuration does not appear in Lapinux.**
Check that the game is recognised (`steam_appids`, `store_ids`, or the exact
name), that `catalog.json` contains the entry, and that the catalog is
reachable. Restart Lapinux after changing the local catalog URL: the catalog is
read at startup and cached for six hours.

**Can a configuration run a command?**
No. Settings and file copies only. This is deliberate: a catalog that runs
code is a supply-chain attack waiting to happen.

**Why is my SHA-256 rejected?**
The published bytes differ from the file you hashed. Recompute with
`sha256sum`, and make sure the file is committed (not ignored by Git).

**The game updated and my fix no longer works.**
Bump `version`, update the files or settings, fill `tested` with the new
version, and set `status` back to `proposed` until you have reproduced it. If
nothing can be salvaged, set `retired` or `broken` and explain why.

**Can I target a specific Proton version?**
Not directly: `layers` and `tested.proton` are informational. A configuration
applies to the game, not to one runtime. If a fix only works on one Proton
family, say so in `description` and the README.

**Can two entries change the same file?**
Technically yes, but declare `conflicts` so Lapinux refuses the second one.
Removal of the first would otherwise restore a file the second expects.

**Who reviews entries?**
The maintainer. Every entry is read before merge; CI catches format and hash
mistakes, not a bad fix. Be precise in your README — it is what reviewers use.

---

## License

The catalog and documentation are released under **CC0 1.0 Universal** unless
an entry states otherwise. Each entry keeps its own `license`; content
contributed to this repository must be redistributable under that license.

See [`LICENSE`](LICENSE).

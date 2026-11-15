# Lapinux Configurations

Community catalog for [Lapinux](../README.md): per-game fixes, declarative
themes and translations, published as `catalog.json` and consumed by the app.

Nothing here launches a game or a store client. A configuration only edits
settings and files that the player chooses to apply, and every file is verified
against its published SHA-256 before it is written.

## Layout

```
catalog.json                  generated catalog, never hand-edited
configurations/<game>/<variant>/
  manifest.json               the configuration itself
  README.md                   what it fixes, how it was tested
  files/…                     optional files copied into the game
themes/<id>.theme             declarative theme: colours, layout, two texts
schema/catalog.schema.json    catalog validation
schema/configuration.schema.json  manifest validation
tools/catalog.py              validator and catalog builder
```

A theme is TOML syntax in a `.theme` file. It carries no image, no script and
no game action: Lapinux validates it, installs it in
`~/.config/lapinux/themes/`, and the player picks it in Settings → Appearance.
Native plugins live in the app; this repository publishes only declarative
themes and configuration manifests.

## Publish a fix

1. Create `configurations/<game>/<variant>/` with `manifest.json`, a short
   `README.md` and, if needed, the files under `files/`.
2. Fill the fields — `schema/configuration.schema.json` is the reference:
   `id`, `title`, `description`, `solution`, `type`, `platform`, `layers`,
   `author`, `version`, `license`, `status`, `tested`, `game` and `actions`.
3. Describe the game by `game.names`, `game.steam_appids` or
   `game.store_ids`; the app matches on all three.
4. Run the builder and check the result:

```bash
cd configurations
python3 tools/catalog.py          # validate, rewrite catalog.json
python3 tools/catalog.py --check  # CI: fails if catalog.json is stale
```

The builder computes every `sha256` and `size` from the files on disk, so never
edit those by hand. A manifest that fails validation is reported and left out.

## Themes

A theme is a native plugin: a manifest plus its `.so`, exactly like any other
plugin (`native.theme = true`). It draws its own layout and palette, so it can
change as much of the interface as the plugin API exposes. This repository
publishes no declarative theme files.

## Actions

- `settings`: `env`, `dxvk`, `args`, Proton or GameScope options applied at
  launch.
- `files`: files copied into the install folder, with `backup` (default true)
  so **Remove** restores the original bytes.

Every file entry needs `source`, `target` and `sha256`; a mismatch aborts the
installation instead of writing unknown bytes.

## Versions and retirement

Each entry carries its own `version`. To withdraw one, mark the manifest
`"status": "broken"` and keep a retired entry in `catalog.json` (the builder
preserves retired entries whose folder is gone) so the published version is not
offered again.

## Safety rules

- Never touch a file outside the game's install folder.
- Keep `backup` on for anything you overwrite.
- Do not ship game assets, copyrighted files or credentials.
- A configuration that needs a kernel driver, a store client or an anti-cheat
  bypass is out of scope and will be refused.

## License

CC0-1.0 for the catalog metadata — see [LICENSE](LICENSE). Files you add keep
their own rights; say so in the entry's `README.md`.

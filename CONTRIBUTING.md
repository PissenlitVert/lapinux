# Contributing to Lapinux

Thanks for helping. This is a Rust workspace: the UI, daemon, overlay and CLI
live in `crates/`, the AppImage is built by `scripts/build-appimage.sh`.

## Before you open a pull request

```bash
cargo fmt
cargo check --workspace
cargo test --workspace
./scripts/check-i18n.sh        # FR/EN catalogs must stay in sync
```

Some tests reach the network or the system. Read them before running the full
suite on a machine holding personal data.

## What a good change looks like

- **One purpose per pull request.** A fix and a refactor in the same diff make
  the review slower, not faster.
- **Explain the cause, not just the diff.** The commit message says why the
  change was needed.
- **Keep the promise.** Lapinux is the launcher: no store client is started or
  required, and a DRM or anti-cheat limit is reported instead of bypassed.
- **User-facing text goes through `crates/core/locales/`.** Add the key in
  French and English; the i18n guard fails otherwise.
- **Comments and docs are welcome in French or English**, but the same file
  should stay in one language.

## Code layout

| Path | Contents |
| --- | --- |
| `crates/core` | config, IPC, i18n, paths, model |
| `crates/daemon` | scanning, launching, sessions, Steam, stores |
| `crates/ui` | egui interface, pages, settings |
| `crates/overlay` | layer-shell overlay, HUD, recording |
| `crates/input` | evdev, pad profiles, uinput |
| `crates/plugin-api` | native plugin ABI (`HOST_ABI_VERSION`) |
| `configurations/` | bundled catalog of per-game fixes |
| `docs/` | guide, installation, architecture, limits |

Themes, plugins and colour palettes are published separately in
[`PissenlitVert/lapinux-configurations`](https://github.com/PissenlitVert/lapinux-configurations);
its `THEMES.md` documents the plugin ABI.

## Reporting a bug

Open an issue with the output of:

```bash
lapinux cli doctor
```

Remove tokens, account names and paths you do not want public. For a security
problem, follow [SECURITY.md](SECURITY.md) instead.

## License

By contributing you agree that your code is published under the MIT license.
Third-party assets keep their own terms.

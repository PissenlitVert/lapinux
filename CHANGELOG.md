# Changelog

All notable changes to Lapinux are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
semantic versioning.

## 0.1.1 — 2026-11-15

A deep release: native plugins and colour palettes, rebuilt controller profiles
with a live test, a windowless Steam client, Discord presence, and an interface
tightened wherever it wasted space.

### Added

- **Native themes and plugins** — one `.so` file, ABI 6 entry point, plugin pages
  (chat, Steam Family), themes that draw their own interface. The catalog
  publishes `abi`, `arch` and `min_lapinux_version`: an incompatible binary is
  refused before download, and the ABI is documented in the catalog's
  `THEMES.md`.
- **Colour palettes** — nine colours published as catalog data, laid over the
  active theme without replacing it: download, update, remove, or export the
  current palette to propose it upstream.
- **Windowless Steam client** — games whose protection goes through
  `steamclient` start after a `-silent` client actually logs on, switchable
  globally or per game.
- **Discord presence** — current page and running game (cover, elapsed time,
  store), reusing icons already cached.
- **NX home** — horizontal library, carousel and dock in the Switch spirit,
  shipped as a native theme from the community catalog.
- **Social** — friends panel, presence, unread badges and Steam notifications in
  the top bar.
- **Anti-cheat** — game sheets name Easy Anti-Cheat and BattlEye next to DRM.
- **Per-game settings** — headless client and Steamworks bridge overridable per
  game, reachable from the cover context menu.
- **Steam Controller 2026** — lizard mode, bridge control and live input tests
  from the controllers page.

### Changed

- **Controllers** — compact profile list with group headers, collapsible
  sections, a "Global profile" banner and an ACTIVE badge; the profile sheet
  carries a live test where the face-button diamond lights up and each position
  names its function, with Nintendo swapping confirm and back correctly.
- **Settings** — palettes above themes, nine colours on one line, a tight HUD
  card (200 px of dead space removed), the rail title centred on its icon, and
  the General/Devices switch at the top of its column.
- **Library** — reading one game no longer clones the whole store: 200 game-sheet
  reads drop from 72 ms to 2.4 ms.
- **i18n** — complete FR/EN catalogs, parity enforced in CI.
- **README** — rewritten short and task-oriented.

### Removed

- **steam-integrity plugin** — the DRM and anti-cheat report in the game sheet
  covers the same need.
- **Native plugin manifests** (`theme-aurora`, `chat-de-bibliotheque`) — the
  catalog no longer ships manifests for binaries that were never published.
- **Tracked screenshots** — 111 MB of committed images dropped from the
  repository.

### Fixed

- **Freshly installed native theme** — loaded immediately instead of after a
  restart.
- **Stale catalog** — a forced reload bypasses the CDN cache and the palette
  card retries until the remote list arrives, so a palette published minutes ago
  shows up right away.
- **Headless Steam start** — waits for `Logged On`, not just the process, so a
  game no longer starts before the session exists.
- **Locale detection** — derived from the real locale instead of a fixed value.
- **Slow daemon test** — no longer sensitive to the calendar.
- **AppImage** — the update check targets the published 0.1.0 asset, and the
  compatibility notes in tests match reality.

### Compatibility

The 0.1.0 AppImage was extracted and started in containers: Ubuntu 22.04 and
24.04, Debian 12, recent Fedora and Arch Linux work. Debian 11 and
AlmaLinux/Rocky 9 are refused (glibc older than 2.35). The graphical interface,
games and controllers were not tested inside those containers.

## 0.1.0

Initial release: library, Proton launching, controllers, overlay, stores,
downloads, configurations and built-in updates.

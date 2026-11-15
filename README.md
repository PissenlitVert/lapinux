<p align="center">
  <img src="assets/lapinux-letters.png" alt="Lapinux" width="420">
</p>

<p align="center">
  <b>A lightweight Rust launcher for Linux.</b><br>
  Steam, Epic, GOG and itch.io in one grid — no store client, no Electron.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2021-orange.svg">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Linux-blueviolet.svg">
  <img alt="Version" src="https://img.shields.io/badge/version-0.1.1-success.svg">
</p>

> **One rule:** Lapinux *is* the launcher. It never starts or requires Steam,
> the Epic Games Launcher, GOG Galaxy or any other store client. Accounts and
> store services are used directly; a game that still needs its own DRM or a
> third-party component gets a clear notice instead of a silent fallback.

## Features

- **One library** — Steam (personal and family), Epic, GOG and itch.io, with
  search, filters and provenance badges.
- **No Steam client** — password, Steam Guard, QR code or pasted token; catalog,
  licences, achievements, playtime and cloud saves over one shared session.
- **Native installs** — depots, manifests and chunks fetched by Lapinux,
  updates detected by `buildid`.
- **Launching** — `proton run`, umu prefixes, GameScope, waiting screen,
  per-game overrides and session logs.
- **Controllers** — evdev, face-button profiles with a live test, virtual
  uinput pad, Steam Controller 2026, Steam Input profiles, battery status.
- **Overlay** — `wlr-layer-shell` quick menu (Guide or `Shift+Tab`), FPS cap,
  customizable HUD, screenshots and recording.
- **Themes and plugins** — native `.so` add-ons plus downloadable colour
  palettes, from the [community catalog](https://github.com/PissenlitVert/lapinux-configurations).

## Quick start

```bash
cargo build --release
./target/release/lapinux cli doctor    # environment check
./target/release/lapinux cli scan      # scan Steam libraries
./target/release/lapinux               # UI
```

The daemon starts on demand. For a user service, see
[`packaging/lapinux.service`](packaging/lapinux.service) and the installation
guide.

Requirements: Rust 2021 toolchain, GTK/WebKitGTK for the store window, and a
Wayland compositor with `wlr-layer-shell` for the overlay.

## Documentation

| Page | Contents |
| --- | --- |
| [Guide](docs/GUIDE.md) | install, develop, test, build the AppImage, publish |
| [Installation](docs/INSTALLATION.md) | dependencies, service, udev, storage |
| [Configuration](docs/CONFIGURATION.md) | paths, `LAPINUX_*` variables, `config.toml` |
| [Features](docs/FEATURES.md) | complete reference, by area |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | symptom → check → fix |
| [Limits](docs/LIMITS.md) | what is not supported, and why |
| [Architecture](docs/ARCHITECTURE.md) | crates, IPC, persistence |

Settings, catalog and tokens live in `~/.config/lapinux` and
`~/.local/share/lapinux`; games, covers and backups live under `~/Lapinux`.

## Status

0.1.1. Features depend on the distribution, drivers, games and accounts in use:
automated tests do not replace a real launch on your desktop. Some tests reach
the network or the system, so check their scope before running them on a
machine holding personal data.

MIT licensed — see [LICENSE](LICENSE). Third-party images, trademarks and fonts
keep their own terms; font notices live in `assets/fonts`.

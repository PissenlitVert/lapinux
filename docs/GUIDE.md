# Everything to do

This page is the ordered list of project steps: install, develop, verify, build
the AppImage and publish. Details for one area stay on its own page; here we
follow the order.

## 1. Prepare the machine

```sh
rustup toolchain install stable
sudo dnf install pkg-config mesa-libGL-devel libxkbcommon-devel wayland-devel \
    libX11-devel libXcursor-devel libXrandr-devel libXi-devel libXinerama-devel \
    squashfs-tools ImageMagick
```

On Debian/Ubuntu the same packages are called `libgl1-mesa-dev`,
`libegl1-mesa-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`,
`libxcursor-dev`, `libxrandr-dev`, `libxi-dev`, `libxinerama-dev`,
`squashfs-tools` and `imagemagick`. The exact list used by CI lives in
`.github/workflows/ci.yml`.

## 2. Run from source

```sh
cargo run -p lapinux          # UI + daemon
cargo run -p lapinux -- cli doctor
```

Development data goes into the normal XDG folders. To leave your real profile
alone, use an isolated root:

```sh
LAPINUX_HOME=/tmp/lapinux-test cargo run -p lapinux
```

`LAPINUX_HOME` holds `config`, `data`, `Cache` and `run`; no migration of the
real profile runs in this mode. `LAPINUX_ROOT` only moves the three visible
folders (`LapinuxLibrary`, `Cache`, `Sauvegardes`).

## 3. Understand the repository

| Item | Role |
| --- | --- |
| `crates/ui` | egui window, pages, settings, log viewer |
| `crates/daemon` | service: jobs, launching, controllers, backups |
| `crates/core` | models, JSON IPC, paths, language catalogs |
| `crates/steam`, `stores`, `proton`, `input`, `overlay`, `perf` | building blocks by area |
| `scripts/` | AppImage, i18n guard, smoke test, probes |
| `docs/` | this documentation, audit and limits |

The interface does not touch game files: it sends JSON requests to the daemon
over the Unix socket (`$XDG_RUNTIME_DIR/lapinux/control.sock`) and reads events
back. Every new action therefore goes through a `Req` in core, handling in the
daemon, then display in the UI.

## 4. Work on a test branch

The `test` branch is for experiments that must not go into a release:

```sh
git switch test            # free experiments
git switch master          # what will be published
git merge test             # once an experiment is validated
```

The `test` branch stays **local**: do not push it to GitHub. Do not publish from
`test` either: releases are built from `master`, with tests and the i18n guard
green.

## 5. Change without breaking

- Code comments, test names and internal notes stay in French; the interface is
  translated through `crates/core/locales/{fr,en}.toml`.
- Every new key must exist in both catalogs, with the same `{name}`
  placeholders.
- A new data path is declared in `crates/core/src/paths.rs`, not in a business
  module.
- A folder migration happens by rename, never by merging, and stays inert in
  `LAPINUX_HOME` mode (`LapinuxPaths::under`).

## 6. Verify

```sh
cargo check --workspace
cargo test -p lapinux-core --lib
cargo test -p lapinux-daemon --lib
cargo test -p lapinux-ui --lib
./scripts/check-i18n.sh
```

For a localized change, run the test that covers it first
(`cargo test -p lapinux-daemon --lib saves`, for example). CI adds the English
suites and the contents of a test AppImage. `scripts/smoke.sh` acts on the real
machine and creates a virtual pad: do not run it by default. See
[TESTS.md](TESTS.md).

## 7. Build the AppImage

```sh
LAPINUX_APPIMAGE_RUNTIME=/path/to/runtime-x86_64 \
LAPINUX_APPIMAGE_RUNTIME_SHA256=<verified-digest> \
./scripts/build-appimage.sh
```

The script refuses a runtime without a digest, does not overwrite an existing
image and writes `dist/Lapinux-<version>-x86_64.AppImage`. Building on a recent
distribution produces a binary that requires its glibc: for a broad image,
build in a container of the oldest distribution you target (Ubuntu 22.04 for
GLIBC 2.35), then check with `readelf --version-info`.

Minimal checks after building:

```sh
dist/Lapinux-<version>-x86_64.AppImage --appimage-extract-and-run --version
LAPINUX_HOME=/tmp/lapinux-appimage dist/Lapinux-<version>-x86_64.AppImage --appimage-extract-and-run cli doctor
sha256sum dist/Lapinux-<version>-x86_64.AppImage
```

## 8. Publish

1. Everything must be committed on `master`, with tests and i18n green.
2. Re-read [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md): licences, runtime,
   glibc compatibility and personal data in the sources.
3. Make the repository public only after that review.
4. Create a `v<version>` tag and a GitHub release containing the AppImage, its
   SHA-256 and the known limits (tested distributions, FUSE, desktop).
5. Never attach a user profile, log, token or conversation screenshot to a
   release.

## 9. Fix a user problem

**Settings › Logs** is no longer primarily about reading files: it offers
**Copy anonymous report**. That JSON contains the version, kernel, desktop and
session, language, memory, CPU, GPUs with their driver, daemon state, checks and
download counters. It includes no raw log, path, game name, account or network
address. Raw logs remain available in the collapsed "Raw logs (advanced)"
section.

Ask for that report first, then only the logs if the report is not enough. The
[TROUBLESHOOTING.md](TROUBLESHOOTING.md) page lists known symptoms.

## 10. Before saying "it's done"

- [ ] `cargo check --workspace` passes.
- [ ] The tests you touched pass, and so does the rest of that area's suite.
- [ ] `./scripts/check-i18n.sh` passes.
- [ ] The documented `~/Lapinux` tree matches the code.
- [ ] The AppImage starts outside the sources (`--appimage-extract-and-run`).
- [ ] The release states the limits you verified, not only the successes.

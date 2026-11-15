# Installation

Build, dependencies, user service and permissions: everything you do once on a
machine.

## Getting started

```bash
cargo build --release
./target/release/lapinux cli doctor    # environment check
./target/release/lapinux cli scan      # Steam libraries
./target/release/lapinux cli protons   # installed Protons
./target/release/lapinux-probe             # native Steam host probe
./target/release/lapinux-probe --session   # isolated local session
./target/release/lapinux-probe account     # accounts: local engine + anonymous CM
./target/release/lapinux-probe account --login <account>  # named CM (token, Steam Guard)
./target/release/lapinux               # interface
```

The daemon is started automatically by the UI and by the CLI when needed. For a
user service, see `packaging/lapinux.service`.

## Building and dependencies

You need a Rust/Cargo toolchain, a C compiler and `pkg-config`, plus the GL/EGL,
X11, Xcursor, Xrandr, Xi, Xinerama, Wayland and xkbcommon graphics libraries
and headers. Package names vary by distribution; the
[list used by CI](../.github/workflows/ci.yml) is a Debian/Ubuntu example. At
runtime, the desktop, graphics drivers and optional tools (Proton, GameScope,
WebKitGTK and others depending on the feature) still have to be provided or
configured as needed.

`cargo build` uses the `dev` profile (`target/debug`) for development;
`cargo build --release` uses the incremental optimised profile
(`target/release`). `cargo build --profile dist` uses the distribution profile
with full LTO (`target/dist`); that choice guarantees neither compatibility with
other glibc versions nor reproducibility. `cargo test --workspace` compiles and
runs the tests; see [TESTS.md](TESTS.md) for their effects and limits.

## Building an AppImage

The current build uses `cargo build --release` (incremental). The `dist` profile
enables full LTO for a release and can take much longer. Building an AppImage
requires Cargo, `mksquashfs`, ImageMagick (`convert`) and `sha256sum`. It
includes the experimental native bridge; other system libraries and optional
tools remain those of the machine.

Download the x86-64 AppImage runtime from its official source and verify its
origin and SHA-256 through a trusted channel. Then give its path and that digest
to the script:

```bash
LAPINUX_APPIMAGE_RUNTIME=/path/to/runtime-x86_64 \
LAPINUX_APPIMAGE_RUNTIME_SHA256=<verified-sha256> \
./scripts/build-appimage.sh
```

The result is written into `dist/` without overwriting an existing image;
another path can be set with `LAPINUX_APPIMAGE_OUTPUT`. A digest printed by the
script identifies the result but does not replace a signature. The glibc version
and dependencies of the build machine determine which systems work: build on a
suitable base and test the targeted systems before distributing. An existing
`AppImage` file is not proof of compatibility or publication.

## Native Steamworks bridge

The bridge is experimental and disabled by default for each Linux Steam game.
It is enabled in the game settings, Display tab. To build the distribution
artifacts and install them into a new folder:

```bash
cargo build --profile dist -p lapinux -p lapinux-steamshim
bash scripts/install-artifacts.sh target/dist /path/to/new-folder
```

The parent folder must exist; the script refuses any existing destination folder
and copies `lapinux`, `lapinux-probe` and `libsteam_api.so` together. A
`cargo build --release` also produces the library in `target/release`; it must
be distributed **with** the `lapinux` binary, in the same folder.
`cargo install` does not copy the library automatically. Another option is to
place it in the Lapinux data folder under the same name. `LAPINUX_STEAMSHIM` can
point the daemon at an explicit path, but does not replace enabling it per game.
A missing library, a non-UTF-8 path or one containing spaces or `:`, a truncated
ELF, a wrong SONAME or an architecture other than x86-64 explicitly blocks
launching. These checks guarantee neither the library's identity nor the
resolution of all its dependencies. Script launchers are accepted, but the
architecture of their final executable cannot be checked in advance. No game
file is replaced; other `LD_PRELOAD` values are preserved. Preloading does not
necessarily cover a game that uses `dlopen` with an absolute path, an isolated
namespace, a static binary or a Steamworks library under another name.

A Steam session recorded by Lapinux is required. The bridge supports part of
`ISteamUserStats` versions 012 and 013: local read/write of integer and float
stats and achievements, and queueing achievements confirmed by `StoreStats`.
Flat initialisation, C++ contexts and some User, Utils, Apps and Friends calls
also have partial answers; this covers neither every method nor every version.
The queue is tied to the session's SteamID and the daemon forwards it to Steam
when it can; `StoreStats` therefore does not prove Steam accepted the upload. If
the local write fails, `StoreStats` returns `false` and the game may retry.
Batches are retried on error, account change or an achievement unknown to the
schema; a sudden machine shutdown is not covered by a transactional guarantee.
Numeric stats stay in memory and are not synchronised; previous Steam values are
not read back, Steam callbacks are not implemented and other interfaces are
partial or neutral. Some methods, versions, protections or dependencies may fail
or crash a game: disable the bridge for that game in that case. No universal
behaviour is guaranteed.

Reproducible test of the production install, on Linux x86-64 with a C compiler
and `bwrap` (user, PID and network namespaces allowed):

```bash
cargo build --profile dist -p lapinux -p lapinux-steamshim
LAPINUX_TEST_INSTALLED_DIST=1 cargo test -p lapinux-daemon --test native_steamshim -- --nocapture
```

The install script runs in a temporary folder under `target`. The installed
`lapinux --daemon` starts a fake C client and Steam library there in an isolated
environment; the test checks the symbol-provider path, bridge enabled and
disabled, pre-existing variables, queueing with a fake session and missing or
incompatible library errors. It tests neither a real game nor a Steam sync. For
queue and launch regressions:

```bash
cargo build -p lapinux-steamshim
cargo test -p lapinux-daemon --test native_steamshim
cargo test -p lapinux-daemon native_spool_keeps_batches_when_account_changes_or_session_is_unavailable
cargo test -p lapinux-perf native_preload_is_applied_to_game_not_scene_or_gamescope
```

Use the same `CARGO_TARGET_DIR` for all commands; the standard test expects the
bridge built in its own profile, and the install test expects artifacts in the
`dist` profile.

Everything carries the Lapinux name: binary, probe, service, udev rules and
environment variables. Settings, catalog and libraries live in
`~/.config/lapinux`, `~/.local/share/lapinux` and `$XDG_RUNTIME_DIR/lapinux`, and
all variables are prefixed `LAPINUX_` (`LAPINUX_HOME`, `LAPINUX_LOG`,
`LAPINUX_STEAMCLIENT`, `LAPINUX_STEAM_CM`).

`Shift+Tab` is captured by KWin during the session (a small script loaded by the
daemon) and removed at the end: it works without any `/dev/input` permission, as
`lapinux cli doctor` shows. Only the controller **Guide** button and the virtual
pad read `/dev/input` through evdev. If that access is refused, install the udev
rule once:

```bash
sudo cp packaging/99-lapinux-input.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input
```

## Storage and cache

By default everything is stored under `~/Lapinux`: `LapinuxLibrary` for managed
games (a path chosen in settings takes precedence), `Cache` for anything that can
be deleted without consequence, and `Sauvegardes` for game copies
(`<game> (<appid>)`) and controller profiles (`Manettes/`). Original saves stay
in the games' folders or prefixes. `LAPINUX_ROOT` moves the parent;
`LAPINUX_HOME` and the XDG variables move the internal folders.

Newly chosen covers live in `data/artwork`, outside the cache; older ones stay
readable in the old cache folder and are preserved when it is cleared. The
cleanup button also keeps store installers in `cache/stores`, so an install or
resume is not interrupted. Some caches are not included in that button; a
failure is reported in settings. Always back up important data separately.

## Steam Controller 2026 native

The `steam-puck-bridge` bridge is not needed with a compatible `hid-steam`
driver that exposes the controller as a gamepad. Linux 7.3 brings that support;
distributions may also backport it. The controller must be on and accessible and
no external service may monopolise the device; Lapinux does not stop it by
itself. If the `steam-puck-bridge.service` user service is installed, the
**Steam Controller bridge** card in **Controllers → Devices** can stop it
temporarily or restart it. That stop does not change its automatic start on
connection; without a compatible native driver, the controller may stop working.
The value `lizard_mode=1` allows emulation at rest: it does not prove the pad is
silent while a program uses it.

Lapinux remapping can present a virtual Xbox pad to the game; it is enabled when
capture, a game profile or a Steam Input profile is active. To let the game
access the native device, disable capture **and** remove active profiles for that
game. In remapped mode, the native d-pad and triggers are converted; the rear
buttons only produce output when assigned to a compatible Xbox button. Trackpad
gestures, advanced Steam Input actions, the gyroscope and rumble feedback are not
forwarded by that virtual pad. The physical assignment of the rear buttons in
Steam profiles still has to be confirmed on the controller. Check the game's
compatibility.

In remapped mode, Lapinux initialises input before launch and only hides the
sources it opened. If a capture is refused at startup, the remapping service
stops and the physical devices remain available to the game. Virtual pad buttons
and axes are neutralised when the overlay opens and read again when it closes or
reconnects. Hiding relies on `bwrap` and the nodes present at launch: access and
reconnection also depend on the driver, permissions and the game.

To also disable keyboard/mouse emulation at rest, `hid-steam` can be configured
at boot:

```bash
sudo cp packaging/99-steam-controller.conf /etc/modprobe.d/
```

Then reboot, or reload the driver if no program holds the controller:
`sudo modprobe -r hid_steam && sudo modprobe hid_steam`.

To switch that mode live from Controllers › controller sheet, use the
**Keyboard/mouse mode (lizard)** switch. If it is greyed out, the **Install the
rule** button asks the desktop for administrator authorisation and installs the
udev rule without a terminal. That rule gives the `wheel` group write access to
this parameter (members of `wheel` can already elevate their privileges).

Without a graphical interface, the same rule can be installed by hand:

```bash
sudo cp packaging/99-steam-controller.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=input
```

The **Keyboard/mouse mode (lizard)** switch then works without root and applies
immediately, without a restart. The **Make native at boot** button installs
`99-steam-controller.conf` with the same authorisation: `hid_steam` will then
start in native pad mode on every boot. Without that file, native mode only
applies to the current session.

To get the rabbit back in the application menu and the taskbar:

```bash
install -Dm644 assets/lapinux-icon.png \
  ~/.local/share/icons/hicolor/512x512/apps/lapinux.png
install -Dm644 packaging/lapinux.desktop \
  ~/.local/share/applications/lapinux.desktop
update-desktop-database ~/.local/share/applications
```

---

[Documentation](README.md) · [Project README](../README.md)

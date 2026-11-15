# Architecture

Lapinux is a Cargo workspace: one main binary (`lapinux`), a separate probe
(`lapinux-probe`), and crates that separate protocols from interfaces.

```
crates/core     XDG paths, TOML config, models, JSON store, IPC, processes
crates/steam    VDF, library scanning, manifests, DRM, catalog,
                Lapinux native downloads, CM client and accounts
crates/stores   Epic Games, GOG, itch.io: accounts, libraries, cloud, achievements
crates/proton   Proton scanning, GitHub releases, checksums, install, prefixes, launching
crates/perf     GameScope, launch preparation, session supervision
crates/input    evdev, profiles, uinput virtual pad, navigation
crates/overlay  menu, software rendering, Wayland shm, layer-shell
crates/daemon   shared state, jobs, launching, diagnostics, IPC socket
crates/ui       egui interface (console, big picture), neutral grey theme;
                lib.rs = shared pages, downloads.rs = Downloads page
                (`Downloads` state, cards; its tests live in the same file)
assets/         logos, fonts, store marks, shaders and embedded runtimes
src/            `lapinux` binary, `lapinux-probe` probe, CLI, dispatch
packaging/      .desktop, systemd user service, udev rules
scripts/        AppImage and runtime builds, checks and probes
```

`crates/steamshim` and `crates/steamshim-win` are separate shims (the second is
built outside the workspace, for Windows): they implement the `steam_api`
surface a Steamworks game expects and relay to the daemon.

## Project map

| Folder | Contents and precautions |
| --- | --- |
| `src/` | Binary entry points, CLI and experimental probe. |
| `crates/core/` | Models, configuration, IPC, translation and persistence. |
| `crates/steam/`, `crates/stores/` | Store protocols and services, without requiring a store client. |
| `crates/steamshim/`, `crates/steamshim-win/` | Experimental Linux and Windows Steamworks bridges; the latter is a separate Cargo project. |
| `crates/proton/`, `crates/perf/` | Proton, launching, GameScope and performance. |
| `crates/input/`, `crates/overlay/`, `crates/ui/`, `crates/daemon/` | Controllers, overlay, interface and job state. |
| `assets/` | Images, fonts and components embedded in the binary; some are required to compile. Their notices must be kept. |
| `configurations/` | Community fix catalog and schema. |
| `packaging/` | Desktop entry, user service and optional system rules. |
| `scripts/` | Build, diagnostics and tests; read each script before running it. |
| `tests/` and `crates/*/tests/` | Integration tests; unit tests also live in the sources. |
| `tools/` | Auxiliary graphical tooling code. |
| `vendor/` | Locally patched egui dependency, required by the Cargo manifest. |
| `docs/` | Guides, reference, research and pre-publication audit. |
| `.github/` | Continuous integration configuration. |
| `target/` | Generated Cargo output and caches, ignored by Git; removing them forces rebuilds and can disturb a running build. |
| `dist/` | AppImages, binaries and local build files, ignored by Git; may contain a deliverable to keep. |

## Who does what

- **The interface** sends catalog and job requests to the daemon and displays
  the answers. Some images or remote pages do their own loading; their spinners
  use the Lapinux indicators.
- **The daemon** owns the state: scanned library, jobs, sessions, Steam session,
  social presence, overlay. Long tasks run there in the background and the
  interface polls.
- **Protocol crates** (`steam`, `stores`, `proton`) know neither egui nor the
  daemon: they are libraries testable on their own.
- **The overlay** is a separate process, started by the daemon, drawing a
  `wlr-layer-shell` surface above the game.

## IPC

Unix socket in `$XDG_RUNTIME_DIR/lapinux/control.sock`, mode `0600`: another user
on the machine cannot connect. Messages are length-prefixed JSON
(`lapinux_core::ipc`), with a frame capped at 16 MiB.

Each request occupies a thread and a socket until it is answered: the daemon
serves **at most 64**, and replies "daemon busy" to the next ones rather than
stacking another thread. A slow Steam read therefore cannot leave thousands of
connections open; the two socket timeouts (read and write) bound the wait, and
closing is explicit for every request, refusals included.

A socket timeout is distinguished from a connection refusal (`IpcTimeout` versus
`Ipc`): a busy daemon must not look dead, or a second process would be started on
top, unable to take the socket.

`Ping` returns the modification date and version of the daemon binary: the
interface compares with its own binary and asks for an old daemon to be replaced
only through `ShutdownIfIdle`, when that daemon supports protected shutdown. Older
versions without that protocol are kept; once every operation is finished, they
may need to be stopped by hand. An old daemon could ignore recent settings and
erase them by saving the configuration again.

## Persistence

| Medium | Contents |
| --- | --- |
| `lapinux.redb` | games, sessions, history, jobs; transactional access |
| `config.toml` | settings, rewritten in full when a key is missing |
| `steam-catalog.json` | cached Steam catalog (re-read in the background) |
| `steam-session.json`, `stores/<store>.json` | tokens as `0600`; the Steam session is written in place, store tokens by rename |
| `cache/` | covers, artwork, installers: disposable |

The logging paths reviewed do not directly record authentication secrets; this
does not guarantee that every log and error message is free of personal data.
The Steam session is written as `0600`, but its replacement is not atomic; see the
[audit](AUDIT-PUBLICATION.md). Covers chosen by hand live in `data/artwork`,
outside the cache, so they survive a cleanup.

## Conventions

- **A single Steam session** for all reads, on a network engine shared by the
  process: a session placed on a disposable runtime died with it, hence one logon
  per read. The social session is separate and lives as long as the daemon runs.
- **Errors surface** up to the relevant card or screen: a network failure on
  cloud saves does not block launching, it goes to the log.
- **Diagnostics**: some Steam refusals are logged (`eresult`) and some game sheets
  explain missing fields; some errors are still swallowed or lack a message and
  must be reviewed before publication.
- **Comments explain the why**, in French, including the measurements behind a
  choice ("169.7 GB announced for 83.4 GB actual"). Tests carry French names and
  check behaviour, not implementation.
- **Translations**: no duplicated string in the code; a stable key names the text
  and the catalogs in `crates/core/locales` translate it.

## Adding a feature

1. **A new request**: a variant in `lapinux_core::ipc::Req`, its handling in the
   daemon, the call in the interface, and a JSON round-trip test in `ipc.rs` —
   that is the only contract between the two processes, a renamed field would
   break the screen with no other sign.
2. **A new store**: the contract is in `crates/stores` (account, library,
   install, cloud, achievements); the white-label mark goes into `assets/stores`,
   and the store is declared in `lapinux_core::stores`.
3. **A new string**: a stable key in **all** catalogs, called through `i18n::t`
   or `t_args`; tests reject a missing key.
4. **A new setting**: a field in the relevant `Config` section, a default value
   compatible with older files, and a line in
   [CONFIGURATION.md](CONFIGURATION.md).

See [TESTS.md](TESTS.md) for the checks to run before delivering.

---

[Documentation](README.md) · [Project README](../README.md)

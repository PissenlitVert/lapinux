# Tests and checks

```bash
./scripts/check-i18n.sh
cargo test --workspace
```

Careful: `scripts/smoke.sh` acts on the real environment and creates a virtual
pad; `scripts/perf.sh` measures and also uses the real machine. Do not run them
without reviewing their effects and getting the necessary approval.

## What the suite covers

- **Inline unit tests** (~1,300): each module tests its contract — JSON
  round-trip of IPC requests, VDF and manifest parsing, CM protocol encryption
  and messages, token file permissions, `config.toml` normalisation, vector
  loading indicators and native HUD placement.
- **Integration tests**: `tests/install_flow.rs` mounts a complete library with
  a fake Proton, installs a managed game, actually launches it, re-reads it
  after deleting the manifest and after restarting the daemon, and checks that a
  Steam manifest without files is never "installed".
  `crates/daemon/tests/configurations.rs` covers installing and removing a
  community configuration, including saved files.
- **Translations**: `cargo test -p lapinux-core` rejects a missing key, a
  diverging placeholder, an empty string, and checks the fallback to French.
- **Native HUD bench**: the computation of the nine anchors is checked offscreen
  (605 × 40 window on 1920 × 1080) before building the layer.

## Network and hardware probes

The binaries in `examples/` are only built with the `probes` feature, so that
`cargo test` does not recompile them:

```bash
cargo run -p lapinux-steam --example qr_timing --features probes
cargo run -p lapinux-steam --example login_probe --features probes
```

They measure reality (QR latency, licence contents, depot sizes), not validate
a delivery: their answers depend on the account and the machine.

## Chat history probe (offline)

`./scripts/steam-chat-history-audit.sh` runs the
`FriendMessages.GetRecentMessages#1` contract tests then counts messages in the
local archive, without text or contact identifiers. It does not start the Steam
client and contacts no service; it therefore cannot measure a real account's
retention, only check the contract and what is already stored.

```bash
./scripts/steam-chat-history-audit.sh
```

## Warning

Features depend on the distribution, drivers, permissions, games and accounts in
use. Automated tests do not replace trying a game launch, the graphical desktop
or real hardware. Some tests reach the network or the system: check their scope
before running the suite on a machine holding personal data.

`LAPINUX_HOME` isolates the Lapinux profile (configuration, data, cache, socket),
but does not replace isolating `HOME`, the desktop or the devices: Steam roots
and personal folders also use `HOME`. For client-free tests, isolate both and the
devices; also inspect child commands. `tests/install_flow.rs` uses a fake Proton
and a temporary profile.

## Audit results from 13 November 2026

- `./scripts/check-i18n.sh`: passed after a concurrent change to the guard by the
  translation agent; a first run had wrongly flagged the `aucun` configuration
  value.
- `cargo test --workspace` with an isolated profile, HOME and devices: exit code
  139 in parallel UI tests; tests in the other crates run up to that point
  passed. Cause identified since: two snapshot tests create a GPU instance in
  parallel and `wgpu::Instance::new` segfaults in `loader_icd_scan` (libvulkan);
  `RUST_MIN_STACK=64MiB` changes nothing, so it is not the stack. Fixed with a
  process lock around GPU creation in captures
  (`crates/ui/src/snapshot_gpu.rs`).
- `cargo test -p lapinux-ui --lib -- --test-threads=1`: 530 passed, 1 ignored.
  With `LAPINUX_LANG=en`, 530 passed, 1 ignored: assertions now compare against
  the catalogs (`t`, `t_args`, `t_fmt`) instead of French literals, and the
  English references for `news` and `steam-input` were regenerated. Only key
  added: `game_sheet.download_version`; existing keys were untouched.
  `LAPINUX_LANG=en cargo test -p lapinux-core --lib -- --test-threads=1`: 148
  passed.
- Targeted: settings and indicator launch/compatibility (7), SteamCMD refusal
  (1), interrupted/paused cancellation (1), tray (8), saves (26), Steam daemon
  tests (43), pause (1), fake install flow (2): passed in the same isolated
  profile. This does not validate a real download.
- Ignored: two KWin tests, five network Steam tests and one UI bench; tests
  located after the workspace failure are not reported as passing by that
  command.
- AppImage 0.1.0: contents, notices and extraction checked. In containers,
  `--version` passes on Ubuntu 22.04 and 24.04, Debian 12, recent Fedora and Arch
  Linux; it fails on Debian 11 (`GLIBC_2.33` and `GLIBC_2.34` missing) and on
  AlmaLinux/Rocky 9 (`GLIBC_2.35` missing). Normal FUSE mounting passes in
  Ubuntu 22.04 with `/dev/fuse` and `libfuse2`, without
  `--appimage-extract-and-run`. `cli doctor` and `cli config` also pass in
  Ubuntu 24.04 and Fedora. The local runtime used was not independently
  authenticated; the image remains unsigned. See
  [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md).

## Addendum: diagnostics and security (13 November 2026)

In the isolated profile described above, the targeted tests check the JSON
export without a daemon or file overwrite, the exclusion of free-form fields
from the report, `0600` mode, concurrent token writes and the refusal to follow
the final link. They also check refusal to shut down with a running, paused or
queued job, a cancelled engine still active or a request in flight, and the
refusal to send an unprotected shutdown order to an old daemon.

ZIP extraction was tested with intermediate and final symlinks, a pre-existing
hard link and a normal archive, then with `..` entries, absolute entries, a
regular file where the archive expects a folder, a linked destination, and a
setuid bit in the archive: every case is refused or neutralised without writing
outside the destination. A truncated local HTTP download does not overwrite the
previous file.

Tokens are written as `0600` in a folder created as `0700` whatever the umask,
an existing file becomes private again, and a failed rename leaves no temporary
file. On the daemon side, a thread panicking while holding the shutdown gate no
longer makes the daemon mute: subsequent requests pass and shutdown remains
decidable. These tests do not validate other archive formats, executable
installers or real downloads.

The UI suite was rerun after these changes, serially and in parallel, in the
isolated profile: French **530 passed, 1 ignored**; English **530 passed, 1
ignored**. Exit code 139 came from concurrent GPU creation in the two snapshot
tests, serialised since; English assertions compare against the catalogs, and
the two stale English visual references (layout from before the page redesign)
were regenerated with `UPDATE_SNAPSHOTS=1` in a sandbox whose sources are
writable. These tests do not replace a human review of both languages.

With no store client: Steam manifest tests (8), Epic manifests (6), GOG library
entry and installer choice (2), itch library and archive choice (2). These are
offline contracts, not login or real game install attempts. The UI i18n guard
passes (3 tests).

The exportable diagnostic is available in the Diagnostic page or with
`lapinux cli diagnostic-export <new-file.json>`. It does not start the daemon
from the CLI and does not replace an existing file. It contains technical states
and aggregated job counts, without raw logs, game names, paths or account
information. Absent values are null or `unknown`. These restrictions reduce
personal data but are not an absolute anonymity guarantee, nor coverage of every
incident: re-read the report before sharing it.

## AppImage from the cleanup

A separate AppImage was then built in an Ubuntu 22.04 container with the
official type-2 runtime version `20251108`, verified against its published
SHA-256. It contains the binaries, the native bridge and the identified notices.
In an isolated profile, `--appimage-extract-and-run --version` and an empty scan
passed. The extracted binary starts in Ubuntu 22.04 and 24.04, Debian 12, recent
Fedora and Arch Linux containers; it is refused on Debian 11 (`GLIBC_2.33` and
`GLIBC_2.34` missing) and on AlmaLinux/Rocky 9 (`GLIBC_2.35` missing), which sets
the floor at GLIBC 2.35. Normal FUSE mounting was checked in an Ubuntu 22.04
container (`/dev/fuse` and `libfuse2`); the graphical interface, games and
controllers were not checked. A literal search for the project's local path in
the three binaries found nothing, but is not a complete personal-data audit.

## Useful manual checks

| What is checked | Command |
| --- | --- |
| environment and permissions | `lapinux cli doctor` |
| real evdev access | `lapinux cli access` |
| launch plan without launching | `lapinux cli launch <appid> --dry-run` |
| uinput virtual pad | `lapinux cli virtual-pad` |
| quick menu outside a game | `lapinux cli overlay-demo --seconds 1` |
| effective configuration | `lapinux cli config` |

## What is still missing

Continuous integration (`.github/workflows/ci.yml`) runs the i18n guard,
`cargo check --workspace`, the whole workspace suite, then the English UI and
core suites. The binary and text parsers (`vdf`, `manifest`, `cm/wire`,
`cm/proto`, `epic_manifest`) have no fuzzing yet, even though they are the most
exposed inputs.

---

[Documentation](README.md) · [Project README](../README.md)

## i18n guard

CI runs this script before the full suite. `scripts/check-i18n.sh` runs the
`crates/ui/tests/i18n_guard.rs` guard then the core catalog tests. It fails if
the fr/en catalogs diverge, if a key called by the interface is missing, or if
an untranslated visible UI literal (an accent, a French word, or text passed
straight to a display helper) reappears. The test's exception list is short and
every entry is justified: external data, internal identifiers, command
arguments. Proper nouns and symbols are allowed separately.

Also run both languages serially:

```sh
cargo test -p lapinux-ui --lib
LAPINUX_LANG=en cargo test -p lapinux-ui --lib -- --test-threads=1
cargo test -p lapinux-core --lib
LAPINUX_LANG=en cargo test -p lapinux-core --lib -- --test-threads=1
```

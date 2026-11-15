# Troubleshooting

Every section follows the same order: what you observe, what to check, then the
fix. Start with the general diagnostic.

## General diagnostic

```bash
lapinux cli doctor      # environment, permissions, Proton, GameScope, controllers
lapinux cli access      # real access to /dev/input
lapinux cli devices     # controllers and evdev devices
lapinux cli config      # effective configuration, after normalisation
```

The four logs live in `~/.local/share/lapinux/logs/` (`daemon.log`, `ui.log`,
`games.log`, `overlay.log`), rotated beyond 8 MB; **Settings › Logs** shows the
last 400 lines without leaving the application. The daemon logs scanning,
launching and installation in particular. These files may contain personal data:
check and anonymise them before sharing, and never disclose tokens.

A daemon older than the binary is reported and **replaced automatically at
startup**: otherwise it would ignore recent settings and erase them by saving the
configuration again. **Settings › System** shows the running daemon version, the
build of the binary on disk, and can restart it by hand. A restart interrupts
downloads in progress: received files are kept and jobs ask to resume. Hiding the
window is not the same as stopping the daemon.

A "daemon busy" message means all 64 concurrent requests are taken: a slow Steam
read occupies its thread until its timeout (30 s by default). This is not a
failure; retrying is enough.

## The daemon does not answer

| Check | Expected |
| --- | --- |
| `$XDG_RUNTIME_DIR/lapinux/control.sock` exists | yes |
| socket permissions | `0600`, owner = user |
| `LAPINUX_LOG=debug ./target/release/lapinux --daemon` | the daemon says what it does |

A socket timeout is **not** a dead daemon: the client distinguishes it from a
connection refusal, precisely to avoid starting a second process on top. If the
socket exists but nothing answers, restart the daemon from **Settings › System**.

The user service is in `packaging/lapinux.service`:

```bash
systemctl --user daemon-reload
systemctl --user enable --now lapinux.service
journalctl --user -u lapinux -n 100
```

## Controllers and buttons

### No controller detected

1. `lapinux cli devices`: does the controller appear?
2. `lapinux cli access`: is `/dev/input` readable?
3. If access is refused, install the udev rule once:

```bash
sudo cp packaging/99-lapinux-input.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input
```

Only the **Guide** button and the virtual pad read `/dev/input`; pad navigation
and `Shift+Tab` do not depend on it.

### The quick menu does not open

- **Shift+Tab** is captured by KWin during the session: the daemon loads a small
  script at startup and removes it at the end. Run `lapinux cli doctor` to see
  whether the script is in place.
- On the pad, the default shortcut is **Guide** (`overlay.pad_hotkey`). An
  `L1+R1+Start` recorded by an older configuration is converted to `Guide` when
  read.
- While the overlay is open, the input service **releases the grab** on
  controllers: the panel is navigated directly with the d-pad and A/B buttons.

### Steam Controller 2026

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| the page reports the Puck but no pad | the dongle answers, the controller is off | turn the controller on; Lapinux does not infer its presence from the Puck |
| the **Keyboard/mouse mode (lizard)** switch is greyed out | udev rule missing | **Install the rule** button in the sheet, or `packaging/99-steam-controller.rules` by hand |
| the switch stays greyed out despite the rule | the rule gives write access to the `wheel` group | on a distribution whose admin group is `sudo`, adapt the rule |
| the controller stops working after the bridge is stopped | `steam-puck-bridge` was stopped temporarily | restart the service from the **Steam Controller bridge** card |
| the emulated pad reports axes that do not exist | some 2.4 GHz dongles report them wrongly | `input.pad_hide_dongle_axes` (the sheet hides those tests) |

`lizard_mode=1` allows keyboard/mouse emulation **at rest**; it does not prove
the pad is silent while a program uses it. To disable it permanently:

```bash
sudo cp packaging/99-steam-controller.conf /etc/modprobe.d/
# then reboot, or reload hid_steam if no one holds the controller
```

## Steam login

| Answer | What it means | What to do |
| --- | --- | --- |
| "invalid password", eresult **5** | not necessarily a wrong password: Steam answers the same when another session just opened | wait, retry; do not conclude the token is dead |
| refused by token or by QR | Steam keeps **one session per account** | a refusal is remembered for five minutes instead of being asked again on every click |
| "the QR challenge did not complete" | two-minute challenge expired, or CM channel dropped | restart the login; Lapinux renews the challenge within a five-minute budget |
| token marked **expired** | authentication refused | the screen returns to "signed out" and offers **Renew by QR** |
| token **unverified** | network failure | do not condemn a healthy token: retry later |

The token is written as `0600` in `steam-session.json`, never the password. It
lasts about 200 days; Steam may revoke it earlier if the account logs in
elsewhere or from "sign out of all devices". A token obtained on another install
can be pasted into the provided field.

A single session serves **all** reads (sheet, achievements, playtime, stats,
catalog, token check): that is what avoids cascading logouts. Per-game reads are
kept for five minutes.

## Store window

The Stores page is rendered by **WebKitGTK 6** (GTK 4) in its own window, driven
by a small embedded Python host.

| Symptom | Cause | Fix |
| --- | --- | --- |
| the panel states a reason instead of the page | `python3-gobject` or WebKitGTK 6 missing | install the packages; the panel offers to open the site in the browser |
| green videos (often NVIDIA) | WebKit DMA-BUF/GL sinks | Lapinux forces the software sink; decoding moves to the CPU, which may cost a core |
| the Steam page costs a whole core | storefront home loop | close the store window when it is not needed |
| an Epic offer stays "waiting for checkout" | Epic requires a checkout | the card joins the queue of offers to finish, opened one by one; the WebKit session keeps cookies |

## Game does not start

1. `lapinux cli launch <appid> --dry-run` shows the plan: binary, Proton,
   prefix, variables. It is the first place to read.
2. **Steam DRM**: Lapinux does not start the Steam client. Detected DRM is
   reported; a game requiring a client or unsupported DRM may not work. The
   native Steamworks bridge remains experimental and limited.
3. **Partial Proton**: an incomplete local build (missing `toolmanifest.vdf`) is
   launched by its own script, not by `umu-run`.
4. **Steam prefix**: an existing Steam prefix **never** goes through umu — umu
   rewrites the `version` file and Steam then considers the prefix invalid.
5. **GE-Proton 11**: its script does not run under the Python 3.9 of the sniper
   runtime; Lapinux uses the host's Python (validated on Bazzite/Fedora).
6. **GameScope missing**: the interface reports it and launching stays direct,
   without an intermediate layer.

## Saves

- A store game needs its **save folder** set: neither Epic nor GOG publishes the
  path where the game writes under Proton.
- A Steam game without a configured folder uses its prefix; the sheet's
  **Saves** tab lists the cloud files and their state.
- An upload is refused if `change_number` moved since the launch snapshot:
  another machine saved in the meantime, and **nothing is sent**. The log says
  so; re-read the sheet before forcing.
- A file identical to the cloud is not sent again (Steam answers
  `DuplicateRequest`).
- The automatic copy on exit can be skipped when the cloud already keeps the
  save (`general.saves_skip_cloud`, checked by default): uncheck it to also keep
  a local copy.

## Sheet images and videos

- Without an embedded AVIF decoder, AVIF artwork and animated banner posters are
  **skipped** rather than shown empty. `ffmpeg` converts other artwork when
  needed.
- Store covers are requested at 600 px from the CDN: at full resolution a row of
  tiles saturated the connection and downloads expired.
- If a cover does not match the game, **right click › Automatic cover** clears
  the manual choice and downloaded covers, then starts again from the preferred
  source.

## Network and quotas

- Steam answers `429` on public profile pages: the penalty protects later
  requests and the sheet re-reads itself some tens of seconds later.
- The first Steam catalog sync takes ~40 s for a thousand games, in the
  background, once.
- A store library is kept for fifteen minutes; **Reload** bypasses that.
- Epic and GOG saves go through endpoints that free launchers documented, not
  public APIs: they may change without notice, and the error appears in the card
  instead of being swallowed.
- A failing cloud sync does not block launching the game: the message goes to
  the log.

## Reporting a problem

Include: the output of `lapinux cli doctor`, the binary and daemon versions
(**Settings › System**), the distribution and graphical session (Wayland/X11,
KDE/GNOME), and the relevant lines from the concerned log. Removing personal
paths is not necessary — logs contain neither password nor token.

---

[Documentation](README.md) · [Project README](../README.md)

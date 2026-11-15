# Features

Complete reference, by area. Every paragraph describes observable behaviour; the
reasoning behind the choices is in [ARCHITECTURE.md](ARCHITECTURE.md) and the
uncovered cases in [LIMITS.md](LIMITS.md).

## Contents

1. [Steam account and installation](#steam-account-and-installation)
2. [Library and stores](#library-and-stores)
3. [Downloads: cards and disk space](#downloads-cards-and-disk-space)
4. [Built-in update](#built-in-update)
5. [Launching, game mode and overlay](#launching-game-mode-and-overlay)
6. [Controllers](#controllers)
7. [Interface, game sheet and settings](#interface-game-sheet-and-settings)
8. [Profile and social](#profile-and-social)

## Steam account and installation

- **Steam login**: the login screen uses Lapinux token auth (the
  `Authentication` service, Steam Guard, a refresh token stored as `0600`): a
  successful login later repeats without a password, and a token obtained
  elsewhere can be pasted into the provided field to sign in directly. A **QR
  code** login is offered in Settings › Accounts: the code is displayed, the
  Steam app on the phone approves it, no password is typed. The resulting token
  is reused as-is on every start, without asking anything again: Steam considers
  it valid for about 200 days. The screen reports an already saved token and
  lets you forget it. There is no fallback to SteamCMD or to the Steam client.

  Steam keeps **one session per account**: two close logons close each other,
  and too many attempts end in a temporary refusal (`invalid password`, eresult
  5) even with a valid token. Hence four safeguards: a token **rotated** by
  Steam is saved again immediately; a refusal by token or by QR is **remembered
  for five minutes** instead of being asked again on every click (a password
  attempt, however, stays immediate); per-game reads (sheet, achievements) are
  **kept for five minutes**; and above all **a single session** serves every
  read — sheet, achievements, playtime, stats, catalog and token check. It lives
  on a **network engine shared by the process**: kept on a sheet's disposable
  runtime, its connection died with it and the "reused" session was already
  disconnected, hence one logon per read (two sheets in a row: 1 logon measured,
  against 2 before).

- **The account's Steam library**: after a single login, the CM session returns
  the account's licences, then PICS expands packages into appids and provides
  name, type and full sheet (in batches of 50). Each licence's owner
  distinguishes owned games from those shared by the Steam family, and the
  **personal account wins**: a game the account owns stays its own even if
  another family member also shares it. PICS does not describe every package — a
  shared licence often answers with an empty sheet, and the application itself
  may refuse its sheet to an account that does not own it: the **public Steam
  store**, without key or account, then completes the list (`packagedetails` for
  a package's appids and their names, `appdetails` for the type). That is what
  makes family games that were missing from the library appear. The **family
  group library** is additionally requested from
  `FamilyGroups.GetSharedLibraryApps` with the session's access token: the
  licence list does not carry every shared game — Steam only adds a licence at
  the moment of borrowing — and those never appeared, not even under their name.
  That service gives the group's appids and names; sheets are then requested
  from PICS and the store like other games, and its name serves as a last resort.
  Weight is computed depot by depot from public manifests, avoiding regional
  editions that were not bought: restricting to the account's known depots
  applies **game by game**. A game whose package PICS does not describe (the
  family one) has no such list: its **anonymous** depots — no platform, language
  or DLC, the only ones that could be counted twice — are then submitted to the
  protocol, which only returns a depot's decryption key to the licences covering
  it (`ClientGetDepotDecryptionKey`). Depots from another edition are excluded
  from the weight: Resident Evil Requiem announced 169.7 GB for 83.4 GB actual,
  Elden Ring 56.2 for 55.1, Nightreign 28.1 for 27.4. With no authorised depot,
  the refusal proves nothing (free tools such as Proton refuse all their keys):
  the sheet then keeps the published weight instead of announcing zero bytes.
  The sheet shows everything Steam publishes: developer, publisher, genres,
  features, descriptors, dates, platforms, controller, Deck, Metacritic, reviews,
  languages and dubbing, DLC, public build, branches, cloud, default language,
  depot weight per platform, and the banner, capsule, logo and icon from the
  Steam CDN. The **default language** is the one Steam would use, read from the
  client registry (`HKCU/Software/Valve/Steam/language`) and not from the desktop
  locale: the protocol publishes the language inventory (`supported_languages`,
  `baselanguages`, `language` depots) but not the player's choice, which the
  client sends itself to the CM (`CMsgClientLogon.client_language`). That choice
  decides which dubbing depot is kept: the announced weight is therefore that of
  the actual download, falling back to English when the game does not publish it.
  When the store page is not public (delisted or unreleased game), that artwork
  is read from the local Steam client cache (`appcache/librarycache`), the only
  official source left. Games are merged directly into the library, with search
  and filters (to install / installed / all, then sources Disk / Steam / Steam
  Family / All Steam); there is no separate Steam tab. Clicking "Download" starts
  the native install, "Play" starts the Proton session. DLC and tools are
  excluded, the catalog is cached in
  `~/.local/share/lapinux/steam-catalog.json` and synchronisation runs in the
  background (the interface polls, nothing blocks).

- **Native installation**: single login with 2FA, password never persisted,
  depots, manifests and chunks downloaded directly by Lapinux, progress tracked
  per job. Only owned games can be downloaded. The version chosen in the sheet's
  arrow is the one whose depots are fetched, additional content included; the
  job keeps it, so the queue and a pause/resume install the same one, and a
  platform the game does not publish simply lets the engine fall back to what it
  knows how to download. The manifest written by Lapinux carries an origin
  marker. In a managed library, Steam games live under `Steam/<game>`, their
  manifests next to them, and prefixes in `CompatData/<game> (<appid>)`; store
  shelves stay at the root. A real Steam library keeps its organisation
  (`steamapps/common`, `steamapps/compatdata`). An update to an installed game
  happens **in its original library**; a Steam game keeps its manifest, its
  `compatdata` and its prefix, only the installed `buildid` is rewritten.

- **Steam updates**: the public `buildid` read in PICS is compared with each
  installed game's manifest; outdated games carry an "UPDATE" badge and an
  "Updates" section lists them in Downloads, with the download weight and an
  "Update" button per game. The incremental download reuses depot verification
  and never starts the Steam client.

## Library and stores

- **Library**: reads `libraryfolders.vdf`, `appmanifest_*.acf`, resolves the
  executable, detects Steam DRM (PE `.bind` sections + `steam_api64.dll`), caches
  covers. `general.library_root` is a library of its own: it is scanned like
  Steam roots, its origin (`Steam` or managed install) is persisted, and games
  missing from a momentary scan are kept instead of being erased.

- **Stores**: Settings › Accounts gathers Steam, **Epic Games**, **GOG** and
  **itch.io** in one list, one account per store, each opened through its
  **white-label mark** (`assets/stores/`). Those marks are silhouettes taken
  from the original logos: at twenty pixels, the full wordmarks ("EPIC GAMES",
  "gog.com", "itch.io") were just mush — the Epic badge, the GOG frame and the
  itch beanie remain readable. Epic and GOG authorise in the browser (the
  authorisation code is pasted afterwards), itch.io signs in with an **API
  key** — the page to create one opens from the account card; saving an account
  answers **immediately**, and re-reading its library runs in the background (it
  took minutes at Epic, beyond the interface timeout: the login looked like it
  failed while the token was written); tokens are written as `0600` under
  `data/stores/<store>.json`, replaced in one go and refreshed when they expire.
  Each store only advertises what it actually exposes — these capabilities are
  API facts, shown as chips, not settings:
  - **Epic** publishes the account library, **cloud saves**
    (`datastorage…/api/v1/access/egstore/savesync/…`, the service Legendary and
    Heroic use) and **achievements** (the store's GraphQL: definitions, account
    progress, global rarity). It also **installs**: the launcher API resolves the
    build, its **binary manifest** lists files split into chunks, and chunks are
    downloaded from the CDN then reassembled at their offsets — the same path as
    Legendary. On Linux, a game without a Linux build takes its Windows build
    (Proton).
  - **GOG** adds offline installation to its **saves**
    (`cloudstorage.gog.com/v1/<account>/<game client>`, with the client
    credentials read from the build metadata — what `gogdl` does) and to its
    **achievements**
    (`gameplay.gog.com/clients/<game>/users/<account>/achievements`).
  - **itch.io** has the library and installation: neither cloud nor
    achievements, its API exposes none.

  A store game's achievements fill the same strip as Steam's, icons and
  percentages included, and the reload link names the store ("Reload from Epic
  Games"). Saves are read and sent from the game settings, with the same buttons
  as Steam: a store game designates its **save folder** (dedicated field,
  `sauvegardes/` in its folder by default) because, unlike Steam, neither Epic
  nor GOG publishes where the game writes under Proton. Once that folder is set,
  sync can happen **by itself**: before launching, cloud files absent locally are
  fetched. If both sides agree, Lapinux takes a snapshot; on exit, files created
  or changed during the session are sent only if the cloud has not changed since
  that snapshot. A divergence asks for a choice in the Saves tab; no remote
  deletion is automatic. An impossible snapshot or upload is reported in the log
  and the local copy is kept. The APIs used do not offer verified conditional
  writes here: a remote change between the last check and the upload can still
  be overwritten, even though Lapinux checks again after sending.

  The library that is read is kept for fifteen minutes then re-read; the
  "Reload" button bypasses that. A store game installs into
  `general.library_root`, like a game managed by Lapinux: a download job with
  progress and cancellation, then a `.lapinux-store.json` marker in the folder —
  that is what stops the next scan from taking a GOG game for a Steam game.
  Executable and version (`linux`, `windows`, `macos`) are kept at install time:
  the game then launches like the others, Proton included for a Windows version.

- **Stores page**: opened from the top bar, it leads to the store itself, not to
  the list of owned games — those stay in the Library. The list of **four
  stores** sits on the left, one line per brand with its mark and the green or
  red dot of its account; on the right, its identity, the account state and the
  door to **the store's website**. egui cannot render HTML, and an offscreen
  rendered view receives no refresh: animations do not advance there, and every
  gesture had to go through a capture — jerky scrolling for a permanent CPU cost.
  The page is therefore handed to **WebKitGTK 6** (GTK 4), GNOME Web's engine, in
  **its own window**: a small Python host (`crates/ui/src/webview_helper.py`,
  embedded in the binary and written to the runtime folder) opens and drives it
  through JSON lines — load, back, reload, foreground. Scrolling, animations,
  keyboard and forms there are those of a browser, since it is one; nothing is
  captured or forwarded, and the stack is **native Wayland** — no XWayland
  needed, the application stays native. The window closes with its own button,
  Lapinux sees it and resets its panel; a single window serves all four stores,
  switching store changes the page. Videos play there like in a browser,
  autoplay included; Steam's DASH/HLS trailers are enabled by two WebKit
  variables, with the GStreamer plugins already present, no extra package. On
  NVIDIA, WebKit's DMA-BUF and GL video sinks paint green frames: the software
  sink is forced, at the cost of CPU decoding. On the Steam page, the home loop
  can therefore cost a whole core while the window stays open. Without
  python3-gobject or WebKitGTK 6, the panel states the reason and offers to open
  the site in the browser.

- **Stores › Free games**: under the four brands, a separate entry gathers the
  offers worth keeping from **Steam**, **Epic Games**, **GOG** and **itch.io** —
  artwork, publisher, original value and local end date ("Ends 13/11/2026 at
  17:00") — grouped by store, with offers announced for later. Free-to-play
  titles and free weekends are excluded: the page keeps only what can be claimed
  and kept. **Epic cart** opens a single free checkout with every still-available
  offer — PC, Android and iOS — in the URL
  (`purchase?offers=…-#/free-checkout`); it opens in the desktop browser, because
  Cloudflare blocks verification of that page in the embedded WebKit window;
  already-owned offers are removed one by one, including when only one platform
  of a mobile game is already on the account; **Claim on Steam** adds the Steam
  licences directly, when there are any. GOG and itch.io offers never open by
  themselves: their card is clickable and opens the chosen offer in the store
  window. The list shows without a signed-in account, and Epic offers already on
  the account are marked "Claimed". PC offers come from the store feed; the
  week's **Android and iOS versions** come from the mobile service and appear
  after them, with their platform chip. Offers already on the account are read
  from Epic entitlements and show "Claimed", with no order or error. A mobile
  offer always ends on its page, hence its "On the store" button. The window
  keeps its login cookies from one opening to the next: an Epic session opened
  once is enough to fill the cart. The offer is tied to the account, not to a
  machine.

- **Covers and artwork**: Steam covers are completed by SteamGridDB when a
  personal API key is saved (Settings → Accounts → SteamGridDB); the API is
  queried with `Authorization: Bearer`, every proposed dimension is accepted and
  centre-cropped rather than distorted, and images are cached. Sources follow
  the Lapinux then SteamGridDB priority order, for covers as for banners,
  capsules, logos and icons. The first source that provides an image wins; the
  next serves as fallback if the game is missing, the key refused or the image
  not found. The Steam client's local cache counts as a Lapinux image: a source
  placed before it is therefore always tried first. On the Lapinux side, the
  Valve CDN is queried in the order `library_600x900_2x` (600×900),
  `library_600x900` (300×450, the simple version is only half), `library_capsule_2x`
  and `library_capsule`, on `cdn.` then `shared.`: some games lack the first
  file, and that is what made their cover fall back to a community source —
  sometimes another game's poster when SteamGridDB did not know the appid. Every
  downloaded image is **decoded before being accepted**: a landscape visual
  (hero, banner) is never cropped into a cover. When no artwork exists, a
  thumbnail shows the game name. Right-clicking a cover opens a gallery of the
  game's SteamGridDB covers: the chosen image is saved locally and takes
  precedence over automatic selection. "Automatic cover" clears the chosen image
  **and** the downloaded covers (Steam and SteamGridDB): the sheet starts again
  from the preferred source, enough to replace a cover that did not match the
  game. The key is never shown in clear.

## Downloads: cards and disk space

- **Download cards**: each job is a game banner — blurred background taken from
  the hero (SteamGridDB, Steam CDN or the client cache), icon and title top left,
  percentage and CDN host top right. The progress bar reuses a gradient of the
  two dominant tones of the blurred background, and **network** throughput
  (chunks actually requested from the CDN, without pieces reused from disk) and
  **write** throughput are measured separately, with processed volume and
  remaining time. A card can be paused then resumed at the same point. A job
  stopped by a restart keeps its files and waits in **Paused** — its card offers
  "Resume" — and never in "Finished", where it would look like a completed
  transfer. Background and palette are computed once per game then kept in
  memory.

- **Downloads folder**: a compact bar in the Downloads header shows the used
  percentage and, hatched in transparency, the space that running and queued
  downloads will take (byte detail and path are in the tooltip). A separate
  "Speed" card, next to the folder, offers a 10 MB test and shows the measured
  throughput. A "Limits" card caps throughput — one global cap, then one per
  launcher — with a hand-drawn track, readable on the card background and
  aligned with its icons; the cap applies after a short pause of the slider,
  without interrupting downloads. The folder can be changed directly from that
  page and is created on the first download.

## Built-in update

- **Check**: Settings → Updates shows the installed version with its build date,
  then the project's latest GitHub release with its publication date. Comparison
  is numeric (`1.10` comes after `1.9`), and a draft or pre-release is never
  offered.
- **Download**: the button downloads the release's x86-64 AppImage into
  `~/Lapinux/Cache/Updates/<tag>/`, with a percentage progress bar and the
  received volume. The SHA-256 published by the release is checked when present;
  without it, the file is accepted as-is.
- **Install**: once the file is complete, "Install the update" writes it next to
  the current AppImage then renames it over — the running instance keeps the old
  file open, an interrupted copy breaks nothing. The operation requires Lapinux
  to run from an AppImage (`$APPIMAGE`): a binary started from `target/` is
  replaced by hand. "Restart now" stops the daemon and restarts the AppImage to
  switch to the new version.

## Launching, game mode and overlay

### Per-game graphics compatibility

In a Windows game's settings, the Proton, DXVK and VKD3D options are optional
and disabled by default: WineD3D, DXVK D3D8, the OpenGL extension limit,
disabling fsync/esync/ntsync, three D3D9 workarounds and disabling VKD3D's DXR
ray tracing. They are passed to the game process only, without modifying its
files or its prefix. A setting explicitly disabled stops being passed; variables
typed by the player or inherited from the session are preserved. Detected
conflicts are reported before launching. Old Proton/DXVK may ignore some
options: esync in particular is obsolete in Proton 11.

ReShade 6.8.0 or OptiScaler 0.9.4 can be installed separately next to the game's
launch executable: proxy choice and confirmation, pinned SHA-256 digests, no
overwrite, and tracking for removal. Only one injector is tracked at a time.
ReShade requires `d3dcompiler_47.dll` in the prefix or near the executable and
does not provide an effects pack; OptiScaler requires `7z`, a 64-bit executable
and a compatible Proton/GPU. Also close the game in Steam or any other launcher:
Lapinux only detects its own sessions. The folder's proxy can also be loaded
outside Lapinux. Anti-cheats may forbid these tools. After a tracked file is
modified, removal keeps it and stays incomplete; an extraction interrupted
before journaling may leave a temporary folder to inspect. No in-game rendering
is guaranteed.

For old games using **software DirectDraw**, the explicit **cnc-ddraw 7.1**
option is offered in the game's graphics settings. The interface reuses the
already configured launch executable — the only accepted location — reads its
architecture without launching it, and enables cnc-ddraw only for a 32-bit
executable (OptiScaler, for its part, is only offered for a 64-bit executable).
The proxy is fixed to `ddraw.dll` and explained, with no useless dropdown. This
option requires an installed 32-bit Windows PE executable, identical to the one
launched by Lapinux, and a working Proton/Wine. The game must usually use its own
fullscreen mode; this wrapper does not support Direct3D games and does not
guarantee they work. Some games require title-specific steps in the project
documentation. A game that launches another executable may need a different
configuration and is not guaranteed.

After confirmation, Lapinux downloads the
[official 7.1 archive](https://github.com/FunkyFr3sh/cnc-ddraw/releases/tag/v7.1.0.0),
checks its maximum size and SHA-256
(`0b13ab89a64c9918189b1dadd449ef6ed3cb3b7b19cabd96d8adbd95505bb908`, also
published by the [SourceForge mirror](https://sourceforge.net/projects/cnc-ddraw.mirror/files/v7.1.0.0/cnc-ddraw.zip/download)),
then extracts only `ddraw.dll` and `ddraw.ini`, each verified individually,
without running the installer. The MIT licence is kept in the tracking folder.
The archive's GLSL shader is not installed: without it, cnc-ddraw's OpenGL
rendering uses its built-in fallback shader; rendering may differ. No existing
`Shaders` folder is modified.

No existing file is overwritten: an existing `ddraw` DLL or INI, a current
injector or a conflicting tracked configuration blocks installation. Only one
injector is tracked at a time. WineD3D, vkBasalt and frame generators are refused
with this wrapper; a pre-existing `CNC_DDRAW_CONFIG_FILE` variable or `ddraw`
override blocks the operation. For launching through Lapinux, `ddraw=n,b` is
added to the other overrides without modifying the Wine registry; outside
Lapinux, loading depends on the Wine configuration and may still happen. Close
the game in Steam or any other launcher before installing/removing. The hard
links used require a compatible filesystem.

The wrapper or the player may modify `ddraw.ini`. On removal, any modified or
replaced file is kept, a warning is shown and tracking stays incomplete; the
unchanged DLL may already have been removed. To finish, back up the wanted data
outside the tracking folder, inspect the reported paths and remove the remaining
copies or links yourself, then retry removal. Do not blindly delete a user file.
An interruption before journaling may leave the untracked temporary folder
`.lapinux-cnc-ddraw-v1` to inspect. Launching outside Lapinux and Wine/game
specific settings are not all controllable by Lapinux.

Once an injector is installed, the same section shows a **diagnostic computed
from disk and settings**, recomputed after every action and as soon as a setting
or the launch executable changes: file state (intact, missing, modified, identical
copy to link, customised `ddraw.ini`), tracked copy state, override actually
applied at launch (added by Lapinux, already provided by `WINEDLLOVERRIDES`, or
blocked with its cause) and incomplete installation. A modified file is never
silently overwritten: "Repair" moves it into
`.lapinux-<tool>-v1/user-modified/` before restoring the original, and a
customised `ddraw.ini` stays intact. "Repair" restores missing files from the
tracked copy, which it rebuilds from the verified archive if it disappeared; it
always targets the recorded installation, even if the launch executable changed.
"Reinstall" is only offered if that executable changed: it backs up modified
DLLs into `.lapinux-<tool>-user-modified/`, puts the customised `ddraw.ini` back
after installation, and rebuilds the official installation even when the tracked
copy is incomplete or corrupted. In that case, the old copy is kept under
`.lapinux-<tool>-stage-ancienne/` instead of being erased.

The sheet lists the backup folders actually present — modified files and old
tracked copy — with a button that opens them in the file manager. Only existing,
non-empty folders are offered; the list disappears when nothing is kept. "Fix
settings" is only offered for conflicts Lapinux can lift: a conflict caused by a
player variable is explained, but does not show a button that would fail. "Fix
settings" disables WineD3D, frame generation and vkBasalt for that game, without
ever deleting a variable typed by the player: if `CNC_DDRAW_CONFIG_FILE`,
`WINEDLLOVERRIDES` or `PROTON_USE_OPTISCALER` still blocks, the message names the
variable to remove.

WineD3D remains a separate lead for some old games, not a universal DirectDraw
fix. DirectPlay and redistributables are not installed automatically. Sources:
[cnc-ddraw instructions](https://github.com/FunkyFr3sh/cnc-ddraw/tree/v7.1.0.0),
[OpenGL fallback code](https://github.com/FunkyFr3sh/cnc-ddraw/blob/v7.1.0.0/src/render_ogl.c).

Sources: [Proton options](https://github.com/ValveSoftware/Proton/blob/proton_11.0/README.md#runtime-config-options),
[DXVK options](https://github.com/doitsujin/dxvk/blob/master/dxvk.conf),
[VKD3D-Proton options](https://github.com/HansKristian-Work/vkd3d-proton#environment-variables).

- **Proton**: detection in Steam and `compatibilitytools.d`, download of
  GE-Proton / Proton-CachyOS GitHub releases with SHA-512 verification, pure Rust
  extraction and installation into `compatibilitytools.d`.

- **Launching**: direct `proton run` with the Steam environment
  (`STEAM_COMPAT_*`), `umu-run` for Lapinux prefixes when the chosen Proton is
  complete (`toolmanifest.vdf`); a partial local Proton is launched by its own
  script. Existing Steam prefixes **never** go through umu (umu rewrites the
  `version` file, Steam then considers the prefix invalid). For Steamworks games,
  Lapinux does not start the Steam client. A native game receives its appid and
  can use the experimental bridge enabled per game: details and limits in
  [Installation](INSTALLATION.md#native-steamworks-bridge). The request runs in
  the background: as soon as the command is ready and the game has not yet shown
  its first frame, the window is covered and the **animated rabbit**
  ([launch.rs](../crates/ui/src/launch.rs)) waits with the game name — fullscreen
  included. This is true for **all games**, Steam and stores alike: a quick
  daemon answer (store game) no longer removes the wait, it is only painted after
  a 0.3 s grace period so it does not blink if the game arrives immediately. The
  animated indicator is drawn by Lapinux: the five vector variants retained are
  also used for loading cards (summary, achievements, news), with no external
  sprite sheet.

- **Game mode**: GameScope (display chosen in settings, resolution and FPS cap
  taken by default from the main display's current mode, HDR, adaptive sync),
  native HUD customisable per game, restoration on exit, per-session log.

- **Quick menu**: a `wlr-layer-shell` surface (overlay) inspired by the Steam
  Deck quick menu (side panel, sections, switches), opened with the controller's
  **Guide** button or **Shift+Tab** on the keyboard by default, with keyboard,
  mouse and controller navigation, session actions (profile, capture, shutdown
  and friends). The **Performance** page groups the HUD switch, the FPS limit,
  the **Simple**, **Basic**, **Detailed** profiles and the nine positions. Its
  **Advanced options** give access to the native HUD: layout (**Inline** or
  **Block**), frametime graph, CPU cores and HUD size. **Inline** mode stops
  after the last statistic instead of stretching the HUD across the whole screen
  width. Colours, order and units are set in Settings → In-game; the HUD re-reads
  its configuration live. The FPS limit also changes live under GameScope, through
  `gamescopectl`; outside it, it waits for the next launch. While the overlay is
  open, the input service forwards navigation to the d-pad and A/B buttons;
  holding a direction repeats the move. In friends, right sends an invitation to
  the selected friend.

- **Achievement notifications**: during a game, a **banner** lands at the top of
  the screen as soon as an achievement unlocks — the achievement icon on the
  left, the game name and its store, its title, and the share of players who
  earned it. The icon drops to the centre then slides left to reveal the text;
  the lines enter one after another. After six seconds, the text fades, the icon
  returns to the centre and rises off-screen; the surface takes neither mouse nor
  keyboard, the game is not interrupted. The watcher re-reads the game's
  achievements **every 45 seconds** without going through the sheet cache, for
  Steam, Epic Games and GOG (itch.io publishes none); an achievement earned
  before launch but synced late is not announced. Banners show one after another,
  at most four queued. The full achievement list of a game carries a **Test the
  notification** button that shows a real achievement's banner, icon and rarity
  included.

## Controllers

- **Controllers**: evdev reading, mapping profiles, uinput virtual pad, library
  navigation on the pad. The Controllers page shows the list on the left
  (controller icon, name, USB wired, Bluetooth or 2.4 GHz dongle connection,
  battery level in a circle) and, on the right, the sheet and live test of the
  selected controller; connected ones come before paired offline ones. The brand
  (PlayStation, Xbox, Nintendo, Steam) is recognised by USB identifiers, its logo
  follows the name and the model is renamed when certain ("Wireless Controller"
  becomes "DualShock 4", "Pro Controller" becomes "Switch Pro Controller"). Any
  controller whose brand cannot be identified is treated as an Xbox controller
  (A/B/X/Y, View/Menu, Xbox logo) without losing its raw name: a PS1/PS2 adapter
  or an exotic clone therefore remains usable. A `045e:028e` pad whose USB
  vendor is not Microsoft (clone or bridge) keeps the Xbox 360 family but is named
  after its connection, "2.4 GHz Controller", instead of being presented as an
  official Xbox 360. Pressed buttons also appear in the list, to the right of the
  brand. Live test cards only show for inputs actually exposed: no invented
  triggers or sticks on a clone. Known dongles and adapters are recognised by
  their USB identifier (Steam Puck, Xbox 360/Xbox wireless receivers, Logitech
  F710, DualShock 4, 8BitDo, Mayflash, Brook, Pelican) and, failing that, by the
  strings "receiver", "dongle", "puck", "wireless adapter" or "2.4 GHz". The
  Steam Controller 2026 ("Triton") is recognised on its four Valve identifiers
  (`28de:1302` wired, `1303` Bluetooth LE, `1304` Puck, `1305` Nereid receiver)
  and displayed as "Steam Controller 2026": the kernel 7.3 `hid-steam` driver
  exposes its sticks, its trackpads, its M1–M4 rear buttons, its quick access
  button and its gyroscope; its battery arrives through `power_supply` and its
  rumble through evdev force feedback, as on other controllers. By default,
  `hid-steam` allows keyboard/mouse mode at rest; a client that opens the gamepad
  can disable it automatically. The `packaging/99-steam-controller.conf`
  configuration (`options hid_steam lizard_mode=0`) disables that emulation at
  rest. As long as no gamepad is detected, the page reports the Puck; it does not
  infer from its presence that the controller is on or that the driver is
  installed. The historical `steam-puck-bridge` bridge, if it takes the Puck's
  hidraw, creates a uinput pad `045e:028e` named "Microsoft X-Box 360 pad":
  Lapinux recognises that virtual pad and also displays it as "Steam Controller
  2026", with a note about the bridge, instead of presenting it as an Xbox 360.
  Battery is read from `power_supply`, in HID reports (DualSense, Switch) and
  through BlueZ; rear buttons, manufacturer buttons and trackpads exposed by
  evdev are listed and testable, even on an unknown controller. If the driver
  provides no name, the card uses the connection as a fallback name: "USB
  Controller", "Bluetooth Controller", "2.4 GHz Controller" for a dongle, or
  "Virtual Controller" for a uinput pad. The profile (deadzone, curve,
  inversions, shortcut) is edited under the test, and a game can choose its own.

## Interface, game sheet and settings

- **Interface**: main screens Library, Profile, Downloads and Controllers;
  Protons, System and Diagnostic are tabs of the Settings window, with no
  separate menu. Steam games appear in the library, with no dedicated tab. Store
  games — **Epic Games, GOG and itch.io** — enter the same way, with the cover the
  store publishes and the same sheet; the source bar carries each store's mark
  next to Steam and disk, one chip per store, and each is checked like the others
  — a store game belongs to none of the Steam sources. GOG provides its vertical
  cover (`boxArtImage`), which its product sheet does not publish. Missing from
  disk, a store game opens a reduced sheet: cover, weight and **Install** button;
  once installed, the local sheet takes over.

  Collapsed or expanded, a store game's sheet is painted by **the same cards as
  the Steam sheet**, not by cousins: the compact line keeps the identity, the
  **Summary** card (description and three screenshots, rabbit while loading,
  clickable screenshots) then achievements; expanded, **News** and account cards
  move to the left, the **banner** then "About" to the centre, and the media card
  — trailer poster on top, screenshots below — to the right, exactly in the place
  and at the height of Steam's. The tag strip closes the sheet with developer,
  publisher, release, store, version and weight. The game card also states the
  state of **cloud saves** in words, "Cloud" green or crossed grey, with a
  tooltip that explains; a crossed icon alone reads badly. **Announcements** come
  from the store when it publishes any: GOG exposes its patch notes in the
  product changelog (`api.gog.com/products/{id}?expand=changelog`, split into one
  note per title, date removed from the title); Epic has no per-game
  announcements, its public blog (`store-content…/content/blog-posts`) is filtered
  on the title — a post that does not carry it in its title must mention it twice
  — and itch publishes none. Without announcements, the card disappears, as on a
  Steam sheet. Each store provides its wide banner — catalog artwork for Epic,
  `backgroundImage` for GOG, cover for itch —, failing that its first screenshot.
  Content is read from the store: the written description and screenshot gallery,
  publisher, developer and release date. Epic goes through the **Store
  storefront** (its public GraphQL), whose library catalog keeps only a title and
  a cover — its description often only repeats the game name. GOG is read from
  its public games API, tags included, and itch.io from the account API, with the
  long text rather than the subtitle. Text follows the language chosen in
  settings. A library shows only **one line per game**: the same title published
  under several offers (trademarks, punctuation) appears once, the installed copy
  winning.

  On the tile, the provenance chip takes the **store's mark** instead of disk,
  and the tooltip says where the game comes from. Store covers are requested at
  600 px from the CDN: at full resolution (up to 2.5 MB per thumbnail), a row of
  tiles saturated the connection and downloads expired — hence the missing
  covers. They are **pre-downloaded** when the library is read, three at a time,
  so no tile waits as a letter on first scroll. The Epic marketplace drops its
  **assets, plugins, engines and projects** into the same library as games; their
  catalog categories say so ("assets", "plugins", "engines", "projects"…). They
  are **hidden** by default and shown again with the "Tools and assets" switch in
  Settings › Accounts; a title in the "games" category always stays a game.
  Records filed under marketplace sands ("UE Marketplace", Fab) are recognised as
  tools even when the catalog says nothing about their categories. **itch.io**
  does the same with its classification: only `game` (and `game_mod`) entries
  stay in the grid, assets, soundtracks and tools follow the same switch.

  **Store marks** are drawn from 512 px logos rendered at the exact screen size
  (density included): the GPU no longer crushes a large image into 30 px, and the
  "EPIC GAMES" or "gog.com" wordmarks stay sharp — list marks move to 48 px, sheet
  marks to 38-42, and Profile tiles to 30 px in a 36 px square. The library shows
  only covers; a click raises from the bottom of the window a compact one-line
  sheet: close and chevron at the far left, then an **identity card** — cover,
  name and provenance as icons (Steam logo, then one person for the personal
  account, two for family, a disk for a local game; appid and state live in the
  tooltip) — in a card with a distinct background. The cover rises to the card
  height and **a line of actions according to the game state** sits to its right,
  under the name; when the text column is too narrow to carry it, it moves to the
  card footer. Its size does not depend on the game state: it stays in place the
  moment a download starts, and the three states occupy the **same block** — a
  button stretched to the block width, narrower only on a tight sheet — instead
  of each shrinking to its label. Installed: stretched Play and two **square
  buttons** of the same height, carrying their own frame: a gear that opens
  settings in the fixed panel, like Settings and content selection — **grouped
  cards** (launch, display, language, environment) that save on every change,
  with the **game language** when you want to force it for that game alone; each
  inherited value is **named** rather than left to a mute "default": the general
  settings' Proton, the FPS limit, GameScope, the native HUD, resolution and
  controller profile are shown under the control, in one line "Inherited from
  general settings: …" — and the Proton choice entry carries that Proton's name;
  a **Cloud saves** card lists the account's files there (size, date, "on disk" or
  "to fetch") with the prefix folder where the game reads them, and a **Fetch from
  cloud** button that downloads them through the protocol and writes them there —
  without a Steam client, a game launched by Lapinux would never have received
  its saves — a bin that asks for confirmation — plus a **circular arrow** when
  Steam publishes a build newer than the one on disk; missing from disk, a button
  carrying the **weight to download**, attached to the **chosen version's
  emblem** — penguin, windows or apple, drawn in outline —: it opens the versions
  Steam publishes (Linux first, the one the machine launches without Proton, then
  Windows, then macOS), each with its own weight, and the button takes the chosen
  version's; **both halves light up together** on hover, the version emblem
  turning bright, so the whole reads as one button up to its right edge. Then, if
  the game has any, a **DLC** button that stacks under it when the column is
  narrow; while downloading, the game's **library icon** (not its cover,
  unreadable cropped into 22 px), the bar, the percentage and the cross that
  cancels the job. For an **already installed** game, the same content selection
  lives in the sheet's maintenance, with a button that installs the checked ones.
  Steam achievements (account, progress bar and icons, read by the account
  session, hidden achievement names included) keep a measured column, the free
  space opening between the card and them: the word "Achievements", the icon
  strip to its right, the count at the edge, and the progress bar below. A game
  completed at 100% carries the **platinum trophy** just before its count (and
  announces it in the full list). A "+ N" tile announces the achievements the
  strip does not show and opens the **full list** (unlocked from newest to
  oldest, then everything left). That list, like additional-content selection,
  opens in the application's **fixed panel** — same frame as Settings: dark
  veil, centred panel that does not move, title and subtitle **centred** on the
  band (the cross keeps the right edge, the same width is reserved on the left),
  scrolling content. No information rail on the right: under the name, a line of
  facts carries the account's **playtime** and the **weight**, each before its
  outlined pictogram (**dial**, then **disk**) — duration alone, "Never played"
  with no minutes, nothing when nobody said. Hours come from the account, so they
  also show for a game **missing from disk** as soon as its sheet is read; weight
  is the local manifest's when the game is installed, and the one Steam publishes
  for the chosen version — the space it will take — when it is not. Both hold a
  separate line under the provenance icons. Hovering the hours details **what the
  account publishes**: Steam total, last two weeks, Linux, Windows, macOS and
  Steam Deck, zero lines staying silent; without an account read, the line says
  "local files" rather than passing a file for a total. A **cloud** joins the
  provenance marks when the game syncs its cloud saves (Steam category read
  through the protocol). **Steam Details** states what the account really keeps
  there: `Cloud.GetAppFileChangelist` returns each file with its size and date,
  `ClientGetAppQuotaUsage` the consumed quota — no byte goes through the Web API
  key. **Launching takes a snapshot** before the game writes, and session exit
  compares the two lists: the sheet then announces what the session **added,
  changed or removed** in the cloud ("Cloud after the session"), instead of
  letting it look like nothing was saved. Game settings take those files one by
  one and can **restore** them: the full name declared by Steam
  (`%WinAppDataLocal%MotorSlice/Saved/SaveGames/GameSave.sav`) is requested from
  `ClientFileDownload`, the CDN archive is expanded when it arrives compressed,
  and the save is written into the game's Proton prefix, at the place the Windows
  token designates (`%GameInstall%`, `%WinAppDataLocal%`, `%WinMyDocuments%`…),
  with the cloud date. **Launching the game first fetches** what the cloud has
  newer than disk (25 s at most, a failure does not block startup): a fresher
  local progress is never overwritten, the button stays there to force the rest.
  The sheet's **tag strip** carries the state: "Cloud 3 files · up to date · sync
  24/10 · on this PC", "to fetch" when a file is missing or the account has a
  newer one, "no save" for a game that advertises cloud with nothing in it,
  "unavailable" when the session did not answer.
  **Session exit sends** what the game wrote: `BeginAppUploadBatch` opens a
  batch, `ClientBeginFileUpload` returns the blocks (address, method, headers),
  the file leaves over HTTP, `ClientCommitFileUpload` validates it and
  `CompleteAppUploadBatchBlocking` publishes the batch — an unclosed batch
  publishes nothing. Only files **newer than the cloud and with different
  content** leave (the list's SHA-1 is authoritative; Steam answers
  `DuplicateRequest` to an identical upload), and if `change_number` moved since
  the launch snapshot, another machine saved: **nothing is sent** and the log says
  so. Game settings offer both directions ("Fetch", "Send"), each with its
  confirmation.

  **Three save slots** double that mechanism, cloud or not: when the game closes,
  the save folder goes into `saves/<game name> (<appid>)/<slot> (<date>)/` — for
  example `1 (31-10-2026 02-13)/` — with a manifest of the same name in `.json`
  (slot, date, file count, weight, original folder, digest). The name makes the
  folder readable without losing uniqueness: the identifier is Lapinux's (the real
  Steam AppID, a stable identifier derived from the store's for Epic, GOG and
  itch), and an old `<appid>` folder follows its game on first access, corrected
  title included; copies from before the readable name, which were only a series
  of Unix seconds, take their "slot + date" on first read, without losing a file.
  The fourth state replaces the oldest copy: three slots per game, not ten
  versions, which bounds disk space and makes old series disappear on first read.
  The digest covers **content** (paths and bytes), not dates: a save rewritten in
  the same second counts as a new state, an identical copy is not stacked. A game
  without cloud therefore keeps something to roll back to; a game with cloud
  receives a local copy only if the setting asks for it — "Do not copy games that
  have a cloud save", checked by default in **Settings › Saves**, skips the
  automatic copy when the store already keeps the session online. The **Saves**
  tab presents the three slots: a click on an **empty square takes a copy
  immediately**, a click on an occupied square selects it, "Load" replaces — the
  replaced state is copied first, so the gesture stays reversible — and lets the
  next launch read the chosen save: a game only reads its saves at startup. The
  **Settings › Saves** setting "Relaunch the game after loading a slot", unchecked
  by default, can be checked so the button says "Load … and play" and starts the
  game immediately. The copy returns into the prefix the game uses today, even if
  it was reinstalled or moved since. A **Current save** card says which copy
  matches the game state and where it comes from — automatic copy on exit, manual
  copy, or loaded slot — and opens its folder. The sheet takes one on demand,
  **restores** one, **deletes** one — the bin becomes a confirmation check and the
  game's saves are never touched —, or **exports** all copies of a game to a
  chosen folder, without touching local copies. The root folder is set in
  **Settings › Saves**, which also shows the total weight.

  **The expanded sheet has up to four tabs**, under its compact line: "General"
  (banner, description, news, screenshots and game settings), "Configurations"
  (only when the catalog knows the game — the community catalog of fixes,
  translations and declarative mods, published at
  `github.com/PissenlitVert/lapinux-configurations`: each card names its author
  and category, installs in one click, removes leaving the game in its previous
  state, and offers **Update** when the catalog publishes a newer version;
  replaced files are backed up and touched settings are restored), "Saves" (the
  chosen folder, versioned local copies and the account cloud) and "Details"
  (everything the store publishes — developer, publisher, genres, dates,
  languages, weight, achievements, cloud, account capabilities — then the
  **minimum and recommended** configurations, notices, DRM and the installation
  on this machine). Changing game returns to "General". An owned Steam game
  missing from disk carries the same tabs: the catalog sheet omits only the
  installation settings. A **tag strip** closes the sheet under every card: the
  recommended age, developer, publisher, price, year, but also **connected
  players**, the **review count** and the **share of positive reviews** each take
  their rectangle (small label, value after it, full value in a tooltip).
  Thousands are spaced with a thin space, tags that do not fit the width are
  announced by a "+ N", and the strip is **centred** in the sheet: its width is
  measured before being painted.

  Collapsed, the sheet fits a **Summary** card into the free space between the
  identity card and achievements: the store's short text, stacked under its
  title, then **three screenshots as thumbnails** in the remaining space — they
  open large in one click. The card is stopped at the exact height of the game
  card: the three cards of the row end together, without touching the tag strip.
  All three exist **from the start**: while the sheet loads, the summary card
  carries the **animated rabbit in the centre**, its label below, and
  achievements announce their own loading. The rabbit is painted at an integer
  multiple of its 16 px, without smoothing: it stays sharp. The
  **achievements** column also runs down to the same line: under the icon strip
  and its progress bar, the free space lists the **latest unlocked achievements**
  (name, unlock date, newest to oldest) or, while none is unlocked, the **rarest
  still to earn** with the share of players who own them — hidden achievements
  keep their secret name. Each line opens the full list, like the "+ N" tile. A
  game publishing none shows a **crossed trophy** in the centre of the card, with
  no text line. The identity card keeps exactly the same size collapsed and
  expanded: opening the body does not change the game card.

  A chevron expands the full body, in **three columns** — **news** on the left,
  the **game** in the centre, **images** on the right (the **trailer** on top,
  then the **screenshots**). The two side columns have exactly the same width: the
  centre is therefore **centred** between them. The game's **banner** opens that
  centre, and the description follows **on the sheet itself**, under it, without
  a card: an **About** title, the short summary, the long text, the **visuals**
  the store places in that text (up to four, at column width), then the
  configurations and notices collapsed. For a store game, the whole description
  fills the long text: the card therefore also exists when it fits in two
  sentences (Epic, GOG, itch), the **Summary** card handling truncation. These
  visuals are the text's images (PNG, JPEG, WebP). Stores increasingly publish
  them as AVIF, and their animated banners are videos (`.webm`, `.mp4`) whose
  poster is also an AVIF: without an embedded AVIF decoder, the sheet skips those
  addresses rather than showing an empty thumbnail. With no video player in the
  interface, the trailer poster opens the Steam page where it plays. Every image
  — banner, screenshot, text visual — opens **inside the application**, in the
  same fixed panel as settings: the browser no longer opens for a screenshot. The
  two arrows over the image, or the ← and → keys, move from one screenshot to the
  next (the series loops); the subtitle says which one is shown. The banner keeps
  a height capped at 170 px, so "About" stays close to it. Maintenance —
  additional content and update — stays under announcements for an installed
  game; **game settings** and **uninstall** live only in the card (gear and bin),
  no longer duplicated here. Below 844 px, cards stack in that order; the three
  columns always fit the sheet, implicit row gaps included. The top line — game
  card, summary, achievements — does not move from one state to the other:
  expanding only opens the body **under** it.

  The body is re-read on every opening in the background (`Req::GameInfo`, ~4 s):
  through the protocol and the single session, total playtime, last two weeks
  and per system (Windows, Linux, macOS, Steam Deck), last played, account level,
  game badge, unlocked achievements and **rarest achievements** with the share
  of players who own them, published stats with their labels, and the number of
  connected players (the only value requested over HTTP for the account, a public
  address without key); on the store side, price with discount, review count,
  genres, summary, screenshots, website and support — the description, minimum
  and recommended configurations and notices live in the centre card, scrolling.
  The store and announcements are requested in the **language chosen in
  settings** (`&l=` of `appdetails` and `ISteamNews`), French by default; Steam
  returns the original text when a game has no translated page. Sources that did
  not answer are listed at the end and the **Reload** button repeats the read.
  **AVIF images**, **small GIFs** and **WebM/MP4 loops** in the description are
  kept with the other visuals, converted by ffmpeg when needed and rendered **in
  text order**: image, paragraph, image. Animated visuals play at their natural
  size.

  The **News** card shows the publisher's three latest announcements — patch
  notes and news — with their **visual** (image from the `{STEAM_CLAN_IMAGE}`
  BBCode, `og:image` as fallback), title, beginning of the text and date. The
  whole card, like the "See all N announcements" button, opens a full-screen
  reader: a narrow column of sections on a translucent dark veil that lets the
  launcher show behind, with its **scrollbar at the right edge of the column**
  and a **fixed dated table of contents just to the right of that bar**. Each
  contents line carries an announcement's date and title and scrolls straight to
  it; hovering an article fades as soon as you click elsewhere. The reader has no
  title bar; Escape or a click on the veil, next to the column and contents,
  closes it. The reader keeps **every publication** from the publisher's
  dedicated feed — up to about a hundred, 98 for 007 First Light — in full text,
  paragraphs preserved and the visual **full width on top**, with no frame around
  the image. They come from the public `ISteamNews/GetNewsForApp` feed, without a
  key, where press releases mix with publisher posts: only the latter are kept,
  and Steam markup (`[img]`, `[url]`, artwork paths) disappears from the preview,
  rejoined sentences included. Cards sit side by side when the column is wide and
  stack otherwise. A game publishing nothing keeps its sheet without the card.
  The **Updates** card no longer exists: the public build, branches and depot
  count remain readable in the centre's **Steam Details** block.

  The library shows neither title nor sync button: its sources (Disk, Steam,
  Steam Family, All Steam) are centred at the top and can be deselected
  independently. The **magnifier and the die** form a single segmented chip, like
  the Steam sources; the magnifier opens the search bar (the field then shows its
  own magnifier, only the die remains), and the segmented filters (All /
  Installed / To install) show their counts. Syncs live in Settings → Library,
  with thumbnail size and hiding Steam tools. A screen icon switches between
  window and fullscreen, Lapinux startup animation. Steam tools and runtimes
  (Proton, Steam Linux Runtime, Steamworks Common Redistributables) can be
  removed from the grid without touching their installation.

- **Settings and logs**: the "Settings" button (or `Ctrl+,`) opens a centred
  window with eight tabs (general, library, appearance, accounts, logs, then
  Protons, System and Diagnostic tools), without leaving the library. _General_
  keeps a draft until "Save" (periodic refresh no longer overwrites typing); its
  _Launching_ card chooses the **default Proton in a list** — "Automatic
  (recommended)" then each installed Proton, with the sentence saying what the
  choice commits to — and the **FPS limit among steps**, from unlimited (no
  argument passed to GameScope, hence the compositor rate) to 360 FPS, plus a
  free value; a _Language_ card sets the **game language** — a Steam name, or
  "Like Steam / the system": it chooses the dubbing depots downloaded (hence the
  announced weight) and the locale passed to the game at launch; a _Waiting
  screen_ card — and the switch of the same name next to GameScope — chooses what
  covers **the whole screen** while a game starts: the **game capsule** (without
  a logo crossing it), the Lapinux **animated indicator**, or **no** veil. The
  window goes fullscreen while preparing then returns to its state, and the veil
  appears as it fades, without blinking when the game takes over; _Appearance_
  sets the order of top-bar sections, applied immediately; _Accounts_ contains
  the whole Lapinux flow (password, pasted token, Steam Guard, mobile
  confirmation, QR code, saved token to forget) and the SteamGridDB key; the
  account sheet fits in a header (avatar, name, status chip) and three tiles —
  account, session, token age — with "Renew by QR" and "Forget the token" within
  reach; _System_ shows the running daemon version, the build of the binary on
  disk and can restart it — a daemon older than the binary is reported and
  replaced automatically at startup, otherwise it would erase recent settings;
  _Logs_ lists four stable logs in the data folder — `daemon.log`, `ui.log`,
  `games.log` and `overlay.log`, rotated beyond 8 MB — shows the last 400 lines
  of the chosen file and re-reads them while the tab is open. The daemon logs its
  actions (scan, launch, install, login) without ever writing a secret. No "Steam
  tools" screen: the download engine is internal and does not appear in the
  interface.

- **Protons page**: the build list fills **when the page opens**, repositories
  being queried in the background so the interface stays alive; "Check for
  updates" re-reads them on demand — GitHub for GE-Proton and Proton-CachyOS,
  Dawn Winery's Forgejo for DW-Proton, which no longer has a GitHub repository —
  and the status line says how old the list is. When "latest" builds are missing,
  a **proposal names them and "Install all" starts them at once**; otherwise a
  green chip says everything is up to date. The page then reads like a list, in
  the manner of ProtonPlus: a **band per source** — its name, what it brings, its
  state ("recommended", "up to date", "update" or "installed only") and a
  **chevron** that expands all its builds, newest on top and pre-releases behind
  stable ones; each build shows its date, weight and its own action — "Install",
  "installed", or the bar and percentage of the running download. Labels are
  **rounded solid fills** without outline, and buttons share three tones (action
  green, check white, local reload grey). Then come the **installed versions**,
  as a list: recommended on top, source, "incomplete" when the build cannot
  launch a game, path in a tooltip.

- **System**: Settings tab with CPU model, threads, load, used memory, GPUs,
  driver and the OpenGL renderer actually used. Direct reading of `/proc` and
  `/sys/class/drm`, with no extra dependency.

## Profile and social

- **Steam profile**: a social session lives with the daemon as long as Lapinux
  runs — the account therefore stays visible to friends, and the chosen presence
  (Online, Away, Invisible, Offline) is published to the CM then confirmed by
  Steam. The Profile page splits in two: the platform list on the left — **Steam,
  Epic Games, GOG and itch.io**, each with its white-label mark and its account
  state (the **username masked** behind dots, revealed by an eye at the edge of
  the line, or "Not signed in") — and on the right the sheet of the chosen
  platform. Steam has its own; a store line leads to its account in Settings ›
  Accounts, from where it is connected. Under a divider, two pages have their own
  line: **Discord** (Rich Presence: in a game and on a sheet, it publishes under
  the game's official application, which provides its name and cover — current
  framerate and its average, Proton version, elapsed time; a sheet announces
  "Viewing …" and adds its store and playtime; a download adds its progress,
  throughput and the game cover; the application's pages publish under the
  Lapinux application, with their sentence and the mark of the open store, the
  only title Discord gives them; a game missing from the catalog stays without
  in-game presence) and **Activity** (sessions and downloads, as game cards). The
  page has only one kind of category: the **day**, collapsible — today,
  yesterday, then the full date. A session and a download of the same day meet
  there together, the running game and the running transfer on top, then newest
  to oldest. Each dated session states its **launch and close** and carries the
  **session log**: copyable with one button, without opening the panel, or
  expanded in it. Dates are read on the local clock — midnight in Paris is not
  the day before in Greenwich, and the day header must not be either. Under each
  session, the **framerate curve** starts at the right of the cover — the title
  column — and runs to the **bounds on its right**, on the same line; it colours
  by framerate — green, yellow under 90% of the maximum, red under 70% — and the
  cursor reads there, in a label placed near the mouse, the time and frames per
  second of the hovered point. New sessions join the list as soon as they close,
  without a restart. The sheet arranges itself in two columns: on the left
  identity (username, presence menu, avatar, SteamID, real name and visibility,
  description) then comments; on the right the Steam family (group name and
  members) then friends (name, state, running game, last connection, avatars,
  offline collapsed). Notifications no longer occupy the sheet: a button opens
  them in their own window, with the detail of what is waiting. Achievements stay
  in a game's sheet. On a narrow window, the list moves above the sheet, and its
  two columns stack. The profile's public pages (XML sheet, comments, family
  names) are requested one at a time and spaced out: when Steam answers 429, the
  penalty protects later requests and the sheet re-reads itself some tens of
  seconds later, rather than staying empty. The friend list is requested from
  `IFriendsListService` with the session's access token: the CM only pushes its
  own once, grouped with the logon answer, and names and states follow through
  `ClientPersonaState` pushes. The refresh token is maintained by
  `Authentication.GenerateAccessTokenForApp` (Steam does not always return a new
  one: the one on disk then stays valid), remains `0600` and is never displayed.

---

[Documentation](README.md) · [Project README](../README.md)

# Configuration

Lapinux is configured first from the interface (**Settings**, `Ctrl+,`). This
page documents what lives on disk, for backups, scripts and troubleshooting.

The `config.toml` file is rewritten in full at startup when known keys are
missing: an absent key takes its default value. Editing the file by hand works,
but closing the application before changing it avoids a value in memory
overwriting your edit.

## Where files live

Three visible folders are enough for the user, under `~/Lapinux`:
`LapinuxLibrary` (games, `Mods`, `CompatData`, `Shaders`), `Cache` (anything that
can be deleted without consequence) and `Sauvegardes` (game copies and
controller profiles). `LAPINUX_ROOT` moves that parent. Configuration, tokens,
the database, logs and the WebKit profile (cookies) stay in the internal XDG
folders listed below.

| Path | Contents |
| --- | --- |
| `~/.config/lapinux/config.toml` | settings (`Config`) |
| `~/.config/lapinux/themes/` | themes created by the user or installed from the catalog |
| `~/.config/lapinux/plugins/` | declarative plugins (colours, layout, dock) |
| `~/.local/share/lapinux/lapinux.redb` | redb database: games, sessions, history |
| `~/.local/share/lapinux/logs/` | `daemon.log`, `ui.log`, `games.log`, `overlay.log` |
| `~/.local/share/lapinux/tools/` | downloaded tools (`innoextract`, `DepotDownloader`…) |
| `~/.local/share/lapinux/compatibility/protons/` | Proton builds installed by Lapinux |
| `~/.local/share/lapinux/artwork/` | covers chosen by hand (outside the cache) |
| `~/.local/share/lapinux/store-web/` | WebKitGTK profile of store windows (cookies, cache) |
| `~/.local/share/lapinux/stores/<store>.json` | store tokens, written as `0600` |
| `~/.local/share/lapinux/steam-session.json` | Steam token, `0600`, never the password |
| `~/.local/share/lapinux/steam-catalog.json` | cached Steam catalog |
| `~/Lapinux/Cache/Covers/` | downloaded covers |
| `~/Lapinux/Cache/Art/` | banner, logo and icon for game sheets |
| `~/Lapinux/Cache/Stores/` | store installers and artwork |
| `~/Lapinux/Cache/Thumbs/`, `Previews/` | screenshot thumbnails and previews |
| `~/Lapinux/Cache/Updates/<tag>/` | AppImage downloaded for an update, before installation |
| `~/Lapinux/Cache/` (root) | game-sheet pages, achievements, avatars, news |
| `$XDG_RUNTIME_DIR/lapinux/control.sock` | daemon IPC socket |
| `$XDG_RUNTIME_DIR/lapinux/ui.alive` | UI liveness marker |
| `$XDG_RUNTIME_DIR/lapinux/ui.pid` | UI process ID |
| `$XDG_RUNTIME_DIR/lapinux/launch.request` | launch request left by the tray |
| `$XDG_RUNTIME_DIR/lapinux/end.request` | UI wake-up when a game closes |
| `~/Lapinux/LapinuxLibrary/` | default managed library (`general.library_root`) |
| `~/Lapinux/LapinuxLibrary/Steam/`, `Epic Games/`, `GOG/`, `itch/` | one folder per launcher |
| `~/Lapinux/LapinuxLibrary/CompatData/<game> (<appid>)/` | Proton prefixes of managed games |
| `~/Lapinux/LapinuxLibrary/Mods/<game> (<appid>)/` | mods, including `SteamWorkshop/` |
| `~/Lapinux/LapinuxLibrary/Shaders/<game> (<appid>)/` | DXVK, Mesa and VKD3D shader caches |
| `~/Lapinux/Sauvegardes/<game> (<appid>)/` | dated backup copies |
| `~/Lapinux/Sauvegardes/Manettes/` | controller profiles and Steam Input copies |

Screenshots and videos follow the matching XDG folder (`~/Videos/Lapinux`,
`~/Pictures/Lapinux`) until another folder is chosen.

**Updates**: Settings → Updates downloads the new AppImage into
`~/Lapinux/Cache/Updates/<tag>/` then writes it over `$APPIMAGE`. The cache and
the AppImage file must be writable by the user, and installation is refused when
Lapinux is not running from an AppImage.

**Backing up**: the whole `~/Lapinux/Sauvegardes/` (game copies and
controllers), `config.toml`, `lapinux.redb`, `artwork/`, the tokens (or accept
logging in again) and, if you created them, `themes/` and `plugins/`.

## Environment variables

All are prefixed with `LAPINUX_` and read through a single point
(`lapinux_core::env_var`), except the standard XDG variables.

| Variable | Effect |
| --- | --- |
| `LAPINUX_ROOT` | parent of the three visible folders (default `~/Lapinux`) |
| `LAPINUX_HOME` | single root: `config`, `data`, `cache` and `run` are taken below it (tests, isolated profiles) |
| `LAPINUX_LOG` | logging filter (`error`, `info`, `debug`, or a `tracing` directive) |
| `LAPINUX_STEAMCLIENT` | library from another Steam client, to compare two trees |
| `LAPINUX_STEAM_CM` | imposed CM server list (`host:port,host:port`) instead of WebAPI discovery |
| `LAPINUX_CONFIGURATIONS_CATALOG` | another community configurations catalog |
| `LAPINUX_BIN` | binary checked by `scripts/smoke.sh` |

`LAPINUX_HOME` takes precedence over `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
`XDG_CACHE_HOME` and `XDG_RUNTIME_DIR`: this is what the tests use to never
touch the real profile.

## `config.toml`

Sections match the interface settings.

### `[general]`

| Key | Default | Role |
| --- | --- | --- |
| `lang` | desktop language | interface language (`fr`, `en`) |
| `game_language` | empty | language for games (Steam name, `french`…) ; empty = the Steam client's choice, otherwise the desktop's |
| `library_root` | `~/Lapinux/LapinuxLibrary` | root of managed libraries |
| `theme` | `sombre` | `classic`, or the identifier of a theme in `themes/` (`nx` once installed from the catalog) |
| `active_plugins` | empty | identifiers of active plugins |
| `topbar_order` | empty | order of top-bar sections (`library`, `downloads`, `controllers`, `profile`, `stores`, `captures`, `news`) |
| `animations`, `big_picture` | `true`, `false` | animations; console-style fullscreen startup |
| `grid_width` | `168` | cover width (clamped 72–240) |
| `launch_screen` | `capsule` | waiting screen: `capsule`, `lapin` or `aucun` |
| `end_screen` | `true` | end screen after the game closes |
| `saves_root` | empty | root for backup copies; empty = Lapinux data |
| `saves_auto`, `saves_skip_cloud`, `saves_relaunch` | `true`, `true`, `false` | copy on exit; skip it when the cloud keeps the save; relaunch after loading |
| `tray_idle_minutes` | `15` | idle minutes before hiding in the notification area (`0`: never) |

### `[library]`

Display and filters: `hide_tools`, `show_demos`, `hide_all_tools`,
`hide_epic_tools`, `hide_gog_tools`, `hide_itch_tools` — the last three are
**true by default** (marketplace assets, plugins and engines stay hidden) —
`show_badges`, `badge_scale`, `cover_radius`, `grid_spacing`, `hover_width`,
`sort` (`name`, `playtime`, `acquired`), `sources` (one checkbox per source:
`disk`, `steam`, `family`, `epic`, `gog`, `itch`) and `filter` (`all`,
`installed`, `missing`).

`[collections]` keeps collections: `items` with `id`, `name`, `field`
(`publisher`, `developer`, `store`, or empty for a manual category) and
`appids`.

### `[steam]`

`username` (never the password), `presence` (`online`, `away`, `busy`,
`invisible`, `offline`, plus the protocol values `snooze`, `trade` and `play`),
`social` (keeps the social session alive), `show_playing` (publishes the running
game), `offline_tools` (network tools in simulation mode).

### `[launch]`

`proton` (`auto` or a build name), `gamescope`, `scene`, `output` (display
connector), `width`, `height`, `fps_cap` (`0` = unlimited), `hdr`,
`adaptive_sync`, `fullscreen`. Without a saved `output`, Lapinux uses the current
mode of the main display.

### `[overlay]`

`hotkey` (`Shift+Tab`), `pad_hotkey` (`Guide`), `fps_max`.

### `[input]`

`grab_devices`, `deadzone` (0–0.6), `stick_curve` (0.5–4), `default_profile`,
`face_profile` (`xbox` or `nintendo`), `gyro`, `pad_names` (renaming by stable
identifier) and `pad_hide_dongle_axes` (hides axes a dongle reports wrongly).

### `[artwork]`

`steamgriddb_key` (personal, never displayed), `steamgriddb` and `sources`
(preference order `steam`, `steamgriddb`).

### `[downloads]`

`global_mbit_per_sec` and `store_mbit_per_sec` (0 = unlimited; the effective
limit is the stricter of the two), `parallel_installs`,
`cloud_saves_after_install`, `steam_server` (preferred Steam CDN, empty = Steam's
choice) and `epic_cdn` (`cloudfront`, `akamai`, `fastly` or empty).

### `[capture]`

Recording and screenshots: `record_on_launch`, `stop_on_exit`, `video_fps`
(15–240), `video_profile` (`performance`, `balanced`, `quality`),
`video_resolution` (`720p`, `1080p`, `1440p`, `native`), `video_encoder`
(`auto`, `nvenc`, `software`), `video_codec` (`auto`, `h264`, `hevc`, `av1`),
`record_audio`, `record_microphone`, `hud_on_screen`, `record_badge`,
`record_badge_position`, `record_badge_scale`, `hud_scale`, `hud_preset`
(empty = automatic), `hud_position`, `hud_layout` (`block` or `inline`),
`hud_graph`, `hud_cores`,
`video_name_format` (`{date}`, `{time}`, `{jeu}`, `{fps}`, `{appid}`, `{unix}`),
`video_dir` and `image_dir`.

### `[discord]`

`enabled`, `show_library`, `show_viewing`, `show_downloads`, `show_game`: what
Rich Presence publishes. An activity title comes from the Discord application
publishing it, hence the official game application while playing and the
Lapinux application on the application's own pages.

## Language

Translation catalogs live in `crates/core/locales`: one TOML per language,
embedded in the binary. `fr.toml` is the source language; English is the
universal fallback for an unhandled locale. To add a key or a language, see
`crates/core/locales/README.md` — the tests reject a missing key, a diverging
placeholder or an empty string.

## Reset

Close the application, then move `~/.config/lapinux/config.toml`: the next
start recreates a default configuration. Games, sessions and tokens are not in
that file.

---

[Documentation](README.md) · [Project README](../README.md)

# Configuration

Lapinux se règle d'abord depuis l'interface (**Réglages**, `Ctrl+,`). Cette page
documente ce qui se trouve sur le disque, pour les sauvegardes, les scripts et
le dépannage.

Le fichier `config.toml` est réécrit en entier au démarrage s'il lui manque des
clés connues : une clé absente prend sa valeur par défaut. Éditer le fichier à
la main fonctionne, mais fermer l'application avant de le modifier évite qu'un
réglage en mémoire ne l'écrase.

## Où vivent les fichiers

Trois dossiers visibles suffisent à l'utilisateur, sous `~/Lapinux` :
`LapinuxLibrary` (jeux, `Mods`, `CompatData`, `Shaders`), `Cache` (tout ce qui
se supprime sans conséquence) et `Sauvegardes` (copies de jeux et profils de
manettes). `LAPINUX_ROOT` déplace ce parent. La configuration, les jetons, la
base, les journaux et le profil WebKit (cookies) restent dans les dossiers XDG
internes, listés ensuite.

| Chemin | Contenu |
| --- | --- |
| `~/.config/lapinux/config.toml` | réglages (`Config`) |
| `~/.config/lapinux/themes/` | thèmes créés par l'utilisateur ou installés depuis le catalogue |
| `~/.config/lapinux/plugins/` | plugins déclaratifs (couleurs, disposition, dock) |
| `~/.local/share/lapinux/lapinux.redb` | base redb : jeux, sessions, historique |
| `~/.local/share/lapinux/logs/` | `daemon.log`, `ui.log`, `games.log`, `overlay.log` |
| `~/.local/share/lapinux/tools/` | outils téléchargés (`innoextract`, `DepotDownloader`…) |
| `~/.local/share/lapinux/compatibility/protons/` | builds Proton installés par Lapinux |
| `~/.local/share/lapinux/artwork/` | jaquettes choisies à la main (hors cache) |
| `~/.local/share/lapinux/store-web/` | profil WebKitGTK des fenêtres de boutique (cookies, cache) |
| `~/.local/share/lapinux/stores/<boutique>.json` | jetons des boutiques, écrits en `0600` |
| `~/.local/share/lapinux/steam-session.json` | jeton Steam, `0600`, jamais le mot de passe |
| `~/.local/share/lapinux/steam-catalog.json` | catalogue Steam mis en cache |
| `~/Lapinux/Cache/Covers/` | jaquettes téléchargées |
| `~/Lapinux/Cache/Art/` | bannière, logo et icône des fiches |
| `~/Lapinux/Cache/Stores/` | installateurs et visuels des boutiques |
| `~/Lapinux/Cache/Thumbs/`, `Previews/` | vignettes et aperçus des captures |
| `~/Lapinux/Cache/Updates/<tag>/` | AppImage téléchargée pour la mise à jour, avant installation |
| `~/Lapinux/Cache/` (racine) | pages de fiches, succès, avatars, actualités |
| `$XDG_RUNTIME_DIR/lapinux/control.sock` | socket IPC du daemon |
| `$XDG_RUNTIME_DIR/lapinux/ui.alive` | marqueur de vie de l'interface |
| `$XDG_RUNTIME_DIR/lapinux/ui.pid` | PID de l'interface |
| `$XDG_RUNTIME_DIR/lapinux/launch.request` | demande de lancement déposée par le tray |
| `$XDG_RUNTIME_DIR/lapinux/end.request` | réveil de l'interface à la fermeture d'un jeu |
| `~/Lapinux/LapinuxLibrary/` | bibliothèque gérée par défaut (`general.library_root`) |
| `~/Lapinux/LapinuxLibrary/Steam/`, `Epic Games/`, `GOG/`, `itch/` | un dossier par launcher |
| `~/Lapinux/LapinuxLibrary/CompatData/<jeu> (<appid>)/` | préfixes Proton des jeux gérés |
| `~/Lapinux/LapinuxLibrary/Mods/<jeu> (<appid>)/` | mods, dont `SteamWorkshop/` |
| `~/Lapinux/LapinuxLibrary/Shaders/<jeu> (<appid>)/` | caches de shaders DXVK, Mesa et VKD3D |
| `~/Lapinux/Sauvegardes/<jeu> (<appid>)/` | copies datées des sauvegardes |
| `~/Lapinux/Sauvegardes/Manettes/` | profils de manettes et copies Steam Input |

Les captures et vidéos suivent le dossier XDG correspondant (`~/Vidéos/Lapinux`,
`~/Images/Lapinux`) tant qu'aucun autre dossier n'est choisi.

**Mise à jour** : l'onglet Réglages → Mises à jour télécharge la nouvelle
AppImage dans `~/Lapinux/Cache/Updates/<tag>/` puis l'écrit par-dessus
`$APPIMAGE`. Le cache et le fichier de l'AppImage doivent être inscriptibles par
l'utilisateur, et l'installation est refusée quand Lapinux ne tourne pas depuis
une AppImage.

**Sauvegarder** : `~/Lapinux/Sauvegardes/` en entier (copies de jeux et
manettes), `config.toml`, `lapinux.redb`, `artwork/`, les jetons (ou accepter de
se reconnecter) et, si on les a créés, `themes/` et `plugins/`.

## Variables d'environnement

Toutes sont préfixées `LAPINUX_` et lues par un point unique
(`lapinux_core::env_var`), sauf les variables XDG standard.

| Variable | Effet |
| --- | --- |
| `LAPINUX_ROOT` | parent des trois dossiers visibles (défaut `~/Lapinux`) |
| `LAPINUX_HOME` | racine unique : `config`, `data`, `cache` et `run` sont pris dessous (tests, profils isolés) |
| `LAPINUX_LOG` | filtre de journalisation (`error`, `info`, `debug`, ou une directive `tracing`) |
| `LAPINUX_STEAMCLIENT` | bibliothèque d'un autre client Steam, pour comparer deux arbres |
| `LAPINUX_STEAM_CM` | liste de serveurs CM imposée (`hôte:port,hôte:port`) au lieu de la découverte WebAPI |
| `LAPINUX_CONFIGURATIONS_CATALOG` | autre catalogue de configurations communautaires |
| `LAPINUX_BIN` | binaire vérifié par `scripts/smoke.sh` |

`LAPINUX_HOME` l'emporte sur `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
`XDG_CACHE_HOME` et `XDG_RUNTIME_DIR` : c'est ce que les tests utilisent pour ne
jamais toucher au profil réel.

## `config.toml`

Les sections correspondent aux réglages de l'interface.

### `[general]`

| Clé | Défaut | Rôle |
| --- | --- | --- |
| `lang` | langue du bureau | langue de l'interface (`fr`, `en`) |
| `game_language` | vide | langue des jeux (nom Steam, `french`…) ; vide = celle du client Steam, sinon du bureau |
| `library_root` | `~/Lapinux/LapinuxLibrary` | racine des bibliothèques gérées |
| `theme` | `sombre` | `classic`, ou l'identifiant d'un thème de `themes/` (`nx` une fois installé depuis le catalogue) |
| `active_plugins` | vide | identifiants des plugins actifs |
| `topbar_order` | vide | ordre des sections de la barre du haut (`library`, `downloads`, `controllers`, `profile`, `boutiques`, `captures`, `news`) |
| `animations`, `big_picture` | `true`, `false` | animations ; démarrage en plein écran façon console |
| `grid_width` | `168` | largeur des pochettes (bornée 72–240) |
| `launch_screen` | `capsule` | écran d'attente : `capsule`, `lapin` ou `aucun` |
| `end_screen` | `true` | écran de fin après la fermeture du jeu |
| `saves_root` | vide | racine des copies de sauvegarde ; vide = données de Lapinux |
| `saves_auto`, `saves_skip_cloud`, `saves_relaunch` | `true`, `true`, `false` | copie à la fermeture ; la sauter quand le cloud garde la partie ; relancer après un chargement |
| `tray_idle_minutes` | `15` | minutes d'inactivité avant de se ranger dans la zone de notification (`0` : jamais) |

### `[library]`

Affichage et filtres : `hide_tools`, `show_demos`, `hide_all_tools`,
`hide_epic_tools`, `hide_gog_tools`, `hide_itch_tools` — ces trois derniers sont
**vrais par défaut** (les assets, greffons et moteurs du marketplace restent
cachés) — `show_badges`, `badge_scale`, `cover_radius`, `grid_spacing`,
`hover_width`, `sort` (`name`, `playtime`, `acquired`), `sources` (une case par
source : `disk`, `steam`, `family`, `epic`, `gog`, `itch`) et `filter` (`all`,
`installed`, `missing`).

`[collections]` garde les collections : `items` avec `id`, `name`, `field`
(`publisher`, `developer`, `store`, ou vide pour une catégorie manuelle) et
`appids`.

### `[steam]`

`username` (jamais le mot de passe), `presence` (`online`, `away`, `busy`,
`invisible`, `offline`, ainsi que les valeurs du protocole `snooze`, `trade` et
`play`), `social` (garde la session sociale en vie),
`show_playing` (publie le jeu en cours), `offline_tools` (outils réseau en mode
simulation).

### `[launch]`

`proton` (`auto` ou un nom de build), `gamescope`, `scene`, `output` (connecteur
d'écran), `width`, `height`, `fps_cap` (`0` = illimité), `hdr`,
`adaptive_sync`, `fullscreen`. Sans `output` enregistré, Lapinux
reprend le mode courant de l'écran principal.

### `[overlay]`

`hotkey` (`Shift+Tab`), `pad_hotkey` (`Guide`), `fps_max`.

### `[input]`

`grab_devices`, `deadzone` (0–0,6), `stick_curve` (0,5–4), `default_profile`,
`face_profile` (`xbox` ou `nintendo`), `gyro`, `pad_names` (renommage par
identifiant stable) et `pad_hide_dongle_axes` (masque les axes qu'un dongle
annonce à tort).

### `[artwork]`

`steamgriddb_key` (personnelle, jamais affichée), `steamgriddb` et `sources`
(ordre de préférence `steam`, `steamgriddb`).

### `[downloads]`

`global_mbit_per_sec` et `store_mbit_per_sec` (0 = illimité ; la limite
effective est la plus stricte des deux), `parallel_installs`,
`cloud_saves_after_install`, `steam_server` (CDN Steam préféré, vide = choix de
Steam) et `epic_cdn` (`cloudfront`, `akamai`, `fastly` ou vide).

### `[capture]`

Enregistrement et photos : `record_on_launch`, `stop_on_exit`, `video_fps`
(15–240), `video_profile` (`performance`, `balanced`, `quality`),
`video_resolution` (`720p`, `1080p`, `1440p`, `native`), `video_encoder`
(`auto`, `nvenc`, `software`), `video_codec` (`auto`, `h264`, `hevc`, `av1`),
`record_audio`, `record_microphone`, `hud_on_screen`, `record_badge`,
`record_badge_position`, `record_badge_scale`, `hud_scale`, `hud_preset`
(vide = automatique), `hud_position`, `hud_layout` (`block` ou `inline`),
`hud_graph`, `hud_cores`,
`video_name_format` (`{date}`, `{time}`, `{jeu}`, `{fps}`, `{appid}`, `{unix}`),
`video_dir` et `image_dir`.

### `[discord]`

`enabled`, `show_library`, `show_viewing`, `show_downloads`, `show_game` : ce
que la Rich Presence publie. Le titre d'une activité vient de l'application
Discord qui la publie, d'où l'application officielle du jeu en partie et
l'application Lapinux sur les pages de l'application.

## Langue

Les catalogues de traduction sont dans `crates/core/locales` : un TOML par
langue, embarqué dans le binaire. `fr.toml` est la langue source, l'anglais le
repli universel pour une locale non gérée. Pour ajouter une clé ou une langue,
voir `crates/core/locales/README.md` — les tests refusent une clé manquante, un
placeholder divergent ou un libellé vide.

## Réinitialiser

Fermer l'application, puis déplacer `~/.config/lapinux/config.toml` : le
prochain démarrage recrée une configuration par défaut. Les jeux, sessions et
jetons ne sont pas dans ce fichier.

---

[Documentation](README.md) · [README du projet](../README.md)

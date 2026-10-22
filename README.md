# Ludus

Launcher de jeux en Rust pour Linux (KDE Wayland / Bazzite), sans client Steam :
bibliothèque Steam, lancement via Proton, couche manettes, mode jeu et quick menu
en overlay.

Binaire unique `ludus` :

| Mode | Commande | Rôle |
|---|---|---|
| UI | `ludus` | bibliothèque, fiches de jeu, téléchargements, Proton, manettes, big picture |
| Daemon | `ludus --daemon` | scan, IPC, lancement, sessions, jobs, overlay |
| Overlay | `ludus overlay` | quick menu layer-shell au-dessus du jeu (lancé par le daemon) |
| CLI | `ludus cli …` | diagnostic, scan, installation, lancement, sessions |

## État vérifié sur cette machine

- Scan réel : **91 jeux** dans les bibliothèques Steam, pochettes et manifests lus.
- Lancement réel validé : **BONK THE BOYKISSER** via gamescope + GE-Proton 11-7,
  préfixe Steam existant réutilisé, session suivie puis arrêtée proprement
  (`ludus cli launch 4119610`, `ludus cli stop`).
- Overlay layer-shell ouvert pendant la session (KWin 6.7, Wayland).
- Coût en release mesuré par `scripts/perf.sh` : binaire **12,2 Mio**, daemon
  **9,2 Mio RSS, 0 % CPU au repos**, IPC **3,0 ms**, scan complet **248 ms**.
- `cargo test --workspace` : **129 tests**, tous verts.

## Démarrage

```bash
cargo build --release
./target/release/ludus cli doctor     # diagnostic de l'environnement
./target/release/ludus cli scan       # bibliothèques Steam
./target/release/ludus cli protons    # Protons installés
./target/release/ludus                # interface
```

Le daemon est démarré automatiquement par l'UI et par la CLI si besoin. Pour un
service utilisateur, voir `packaging/ludus.service`.

## Fonctionnalités

- **Bibliothèque** : lecture des `libraryfolders.vdf`, `appmanifest_*.acf`,
  résolution de l'exécutable, détection DRM Steam (sections PE `.bind` +
  `steam_api64.dll`), pochettes mises en cache.
- **Installation sans Steam** : `steamcmd` (officiel) et `DepotDownloader`
  (opt-in), connexion unique avec 2FA, mot de passe jamais persistant, suivi de
  progression par job. Seuls les jeux possédés sont téléchargeables.
- **Proton** : détection dans Steam et `compatibilitytools.d`, téléchargement des
  releases GitHub GE-Proton / Proton-CachyOS avec vérification SHA-512,
  extraction pure Rust et installation dans `compatibilitytools.d`.
- **Lancement** : `proton run` direct avec l'environnement Steam
  (`STEAM_COMPAT_*`), `umu-run` pour les préfixes gérés par Ludus. Les préfixes
  Steam existants ne passent **jamais** par umu (umu réécrit le fichier
  `version`, Steam considère alors le préfixe comme invalide).
- **Mode jeu** : gamescope (résolution, cap FPS, HDR, adaptive sync),
  `gamemoderun` si présent, MangoHud configurable par jeu, restauration à la
  sortie, journal par session.
- **Manettes** : lecture evdev, profils de mapping, pad virtuel uinput, navigation
  complète de l'UI au pad.
- **Quick menu** : surface `wlr-layer-shell` (overlay), navigation clavier et
  manette, actions session (cap FPS, MangoHud, profil, capture, arrêt).

## Limites assumées

- Les jeux protégés par un **DRM Steam** sont détectés et signalés ; aucun
  contournement n'est fourni. Il faut démarrer Steam en arrière-plan.
- Steam Input, succès, cloud, atelier et Remote Play dépendent du client Steam :
  hors périmètre. La couche manettes maison couvre boutons, axes, gyro et
  macros.
- `steamcmd`/`DepotDownloader` exigent un compte Steam et la 2FA ; seul le
  contenu possédé est accessible.
- `gamemode` et `gpu-screen-recorder` peuvent manquer : l'UI le signale et
  dégrade proprement (gamescope/MangoHud/nice restent actifs).
- Le runtime sniper de Steam embarque Python 3.9, trop ancien pour le script
  `proton` de GE-Proton 11 ; Ludus exécute donc `proton` avec le Python de
  l'hôte (validé sur Bazzite/Fedora).

## Architecture

```
crates/core     chemins XDG, config TOML, modèles, store JSON, IPC, processus
crates/steam    VDF, scan des bibliothèques, manifests, DRM, SteamCMD, DepotDownloader
crates/proton   scan des Proton, releases GitHub, checksums, installation, préfixes, lancement
crates/perf     gamescope, MangoHud, gamemode, supervision de session
crates/input    evdev, profils, pad virtuel uinput, navigation
crates/overlay  menu, rendu logiciel, shm Wayland, layer-shell
crates/daemon   état partagé, jobs, lancement, diagnostic, socket IPC
crates/ui       interface egui (console, big picture)
src/            binaire unique, CLI, dispatch
packaging/      .desktop, service systemd utilisateur, règles udev
scripts/        smoke test et rapport de performances
```

IPC : socket Unix dans `$XDG_RUNTIME_DIR/ludus/ludus.sock`, messages JSON
ligne par ligne (`ludus_core::ipc`).

## Tests

```bash
cargo test --workspace
./scripts/smoke.sh          # diagnostic + scan + protons + dry-run
./scripts/perf.sh           # RSS/CPU au repos, latence IPC, durée de scan
```

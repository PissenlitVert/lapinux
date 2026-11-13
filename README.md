# Lapinux

Launcher de jeux en Rust pour Linux (KDE Wayland / Bazzite), sans client Steam :
bibliothèque Steam et boutiques, lancement via Proton, couche manettes, mode jeu
et quick menu en overlay.

## Principe non négociable : aucun launcher tiers

Lapinux est le launcher : ses fonctionnalités ne doivent jamais lancer ni
imposer l'installation de Steam, Epic Games Launcher, GOG Galaxy ou d'un autre
launcher tiers. Les comptes et services des boutiques peuvent être utilisés
directement, sans leur client. Un jeu peut néanmoins imposer son propre DRM ou
un composant externe : dans ce cas, signaler la limite sans promettre sa prise
en charge ni ouvrir un launcher en secours. Proton, les outils techniques et les
composants propres à Lapinux ne sont pas des launchers de boutique.

Deux binaires : `lapinux` (interface, daemon, overlay, CLI) et `lapinux-probe`
(sonde de l'hôte natif Steam).

| Mode | Commande | Rôle |
|---|---|---|
| UI | `lapinux` | bibliothèque, fiches de jeu, téléchargements, Proton, manettes, big picture |
| Daemon | `lapinux --daemon` | scan, IPC, lancement, sessions, jobs, overlay |
| Overlay | `lapinux overlay` | quick menu layer-shell au-dessus du jeu (lancé par le daemon) |
| CLI | `lapinux cli …` | diagnostic, scan, installation, lancement, sessions |

## En bref

- **Bibliothèque unique** : Steam (compte personnel et famille), Epic Games, GOG
  et itch.io dans la même grille, avec recherche, filtres et pastilles de
  provenance.
- **Sans client Steam** : connexion par mot de passe, Steam Guard, QR code ou
  jeton collé ; catalogue, licences, fiches, succès, temps de jeu et cloud lus
  par le protocole, avec une seule session partagée.
- **Installation native** : dépôts, manifestes et chunks téléchargés par
  Lapinux ; mises à jour détectées par `buildid`.
- **Lancement** : `proton run`, umu pour les préfixes Lapinux, GameScope,
  écran d'attente, journal de session.
- **Manettes** : evdev, profils, pad virtuel uinput, Steam Controller 2026,
  Steam Input.
- **Overlay** : quick menu `wlr-layer-shell` (Guide ou `Shift+Tab`), limite de
  FPS, HUD natif personnalisable, captures et enregistrement.
- **Boutiques** : fenêtre WebKitGTK native, jeux gratuits Epic, sauvegardes et
  succès Epic/GOG, installation GOG et itch.

## Démarrage rapide

```bash
cargo build --release
./target/release/lapinux cli doctor    # diagnostic de l'environnement
./target/release/lapinux cli scan      # bibliothèques Steam
./target/release/lapinux cli protons   # Protons installés
./target/release/lapinux               # interface
```

Le daemon est démarré automatiquement par l'UI et par la CLI si besoin. Pour un
service utilisateur, voir `packaging/lapinux.service` et
[docs/INSTALLATION.md](docs/INSTALLATION.md).

## Mise à jour intégrée

Lapinux se met à jour depuis les releases GitHub du projet. Réglages → Mises à
jour affiche la version installée avec sa date de fabrication et la dernière
release publiée avec sa date, puis télécharge la nouvelle AppImage avec une
barre de progression avant de l'installer par-dessus celle qui tourne.

L'AppImage arrive dans `~/Lapinux/Cache/Updates/<tag>/`, est comparée à
l'empreinte SHA-256 publiée quand il y en a une, puis écrite à côté de la cible
et renommée dessus. L'installation exige que Lapinux tourne depuis une AppImage
(`$APPIMAGE`) : un binaire lancé depuis un terminal se remplace à la main.
Après l'installation, « Redémarrer maintenant » arrête le daemon et relance
l'AppImage.

### Ce qu'une release doit publier

L'application interroge
`https://api.github.com/repos/PissenlitVert/lapinux/releases/latest` : la release
doit donc être publiée — brouillons et pré-releases sont ignorés — avec :

- un tag du type `v0.2.0`, comparé numériquement à la version installée ;
- un fichier joint terminé par `.AppImage` et mentionnant `x86_64` ou `amd64` ;
- une empreinte SHA-256. GitHub renseigne le champ `digest` de l'asset
  (`sha256:…`) quand le fichier envoyé en porte une ; sans elle, le
  téléchargement fonctionne mais n'est pas vérifié.

## Documentation

| Page | Contenu |
| --- | --- |
| [Tout ce qu'il faut faire](docs/GUIDE.md) | l'ordre complet : installer, développer, tester, construire l'AppImage, publier |
| [Installation](docs/INSTALLATION.md) | build, dépendances, service, udev, stockage, Steam Controller |
| [Configuration](docs/CONFIGURATION.md) | chemins, variables `LAPINUX_*`, clés de `config.toml` |
| [Fonctionnalités](docs/FONCTIONNALITES.md) | référence complète, par domaine |
| [Dépannage](docs/DEPANNAGE.md) | symptôme → vérification → remède |
| [Limites assumées](docs/LIMITES.md) | ce qui n'est pas fait, et pourquoi |
| [Architecture](docs/ARCHITECTURE.md) | carte des dossiers, crates, IPC et persistance |
| [Tests](docs/TESTS.md) | suite, smoke test, sondes, vérifications manuelles |
| [Audit avant publication](docs/AUDIT-PUBLICATION.md) | risques prioritaires, nettoyage et matrice de validation |
| [Hôte natif Steam](docs/RECHERCHE-HOTE-NATIF.md) | carnet de mesures de `lapinux-probe` |

Tout porte le nom Lapinux : binaire, sonde, service, règles udev et variables
d'environnement. Trois dossiers visibles suffisent à l'utilisateur, sous
`~/Lapinux` : `LapinuxLibrary` (jeux, `Mods`, `CompatData`, `Shaders` et un
dossier par launcher), `Cache` (jaquettes, pages, vignettes : supprimable) et
`Sauvegardes` (copies de jeux et profils de manettes). Les réglages, le
catalogue, les jetons et la base restent dans `~/.config/lapinux`,
`~/.local/share/lapinux` et `$XDG_RUNTIME_DIR/lapinux`, et les variables sont
toutes préfixées `LAPINUX_` (`LAPINUX_ROOT`, `LAPINUX_HOME`, `LAPINUX_LOG`,
`LAPINUX_STEAMCLIENT`, `LAPINUX_STEAM_CM`).

## État et vérifications

Les fonctionnalités dépendent de la distribution, des pilotes, des permissions,
des jeux et des comptes utilisés. Les tests automatisés ne remplacent pas un
essai du lancement de jeu, du bureau graphique ou du matériel réel. Certains
tests accèdent au réseau ou au système : vérifier leur portée avant de lancer la
suite sur une machine contenant des données personnelles.

Avant publication, le titulaire doit préciser la licence du projet : les
manifestes indiquent MIT, mais aucun fichier de licence général n'est présent.
Vérifier séparément les droits des images, marques et autres ressources tierces ;
les polices possèdent leurs propres notices dans `assets/fonts`.

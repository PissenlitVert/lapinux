# Tout ce qu'il faut faire

Cette page est la liste ordonnée des gestes du projet : installer, développer,
vérifier, construire l'AppImage et publier. Les détails d'un domaine restent
dans sa page dédiée ; ici, on suit l'ordre.

## 1. Préparer la machine

```sh
rustup toolchain install stable
sudo dnf install pkg-config mesa-libGL-devel libxkbcommon-devel wayland-devel \
    libX11-devel libXcursor-devel libXrandr-devel libXi-devel libXinerama-devel \
    squashfs-tools ImageMagick
```

Sur Debian/Ubuntu, les mêmes paquets s'appellent `libgl1-mesa-dev`,
`libegl1-mesa-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`,
`libxcursor-dev`, `libxrandr-dev`, `libxi-dev`, `libxinerama-dev`,
`squashfs-tools` et `imagemagick`. La liste exacte utilisée par la CI est dans
`.github/workflows/ci.yml`.

## 2. Lancer depuis les sources

```sh
cargo run -p lapinux          # interface + daemon
cargo run -p lapinux -- cli doctor
```

Les données de développement vont dans les dossiers XDG normaux. Pour ne pas
toucher au profil réel, utiliser une racine isolée :

```sh
LAPINUX_HOME=/tmp/lapinux-test cargo run -p lapinux
```

`LAPINUX_HOME` héberge `config`, `data`, `Cache` et `run` ; aucune migration du
profil réel n'est lancée dans ce mode. `LAPINUX_ROOT` déplace seulement les
trois dossiers visibles (`LapinuxLibrary`, `Cache`, `Sauvegardes`).

## 3. Comprendre le dépôt

| Élément | Rôle |
| --- | --- |
| `crates/ui` | fenêtre egui, pages, réglages, journal |
| `crates/daemon` | service : tâches, lancement, manettes, sauvegardes |
| `crates/core` | modèles, IPC JSON, chemins, catalogues de langue |
| `crates/steam`, `stores`, `proton`, `input`, `overlay`, `perf` | briques par domaine |
| `scripts/` | AppImage, garde i18n, smoke test, sondes |
| `docs/` | cette documentation, audit et limites |

L'interface ne touche pas aux fichiers de jeu : elle envoie des requêtes JSON
au daemon par la socket Unix (`$XDG_RUNTIME_DIR/lapinux/control.sock`) et relit
les événements. Toute action nouvelle passe donc par un `Req` côté cœur, un
traitement côté daemon, puis un affichage côté UI.

## 4. Travailler sur une branche de test

La branche `test` sert aux essais qui ne doivent pas partir en release :

```sh
git switch test            # essais libres
git switch master          # ce qui sera publié
git merge test             # une fois un essai validé
```

La branche `test` reste **locale** : ne pas la pousser sur GitHub. Ne pas
publier directement depuis `test` non plus : la release se construit depuis
`master`, avec les tests et la garde i18n au vert.

## 5. Modifier sans casser

- Commentaires, noms de tests et documentation en français ; l'interface est
  traduite par `crates/core/locales/{fr,en}.toml`.
- Toute clé ajoutée doit exister dans les deux catalogues, avec les mêmes
  placeholders `{nom}`.
- Un nouveau chemin de données se déclare dans `crates/core/src/paths.rs`, pas
  dans un module métier.
- Une migration de dossier se fait par renommage, jamais par fusion, et reste
  inerte en mode `LAPINUX_HOME` (`LapinuxPaths::under`).

## 6. Vérifier

```sh
cargo check --workspace
cargo test -p lapinux-core --lib
cargo test -p lapinux-daemon --lib
cargo test -p lapinux-ui --lib
./scripts/check-i18n.sh
```

Pour une modification localisée, lancer d'abord le test qui la couvre
(`cargo test -p lapinux-daemon --lib saves`, par exemple). La CI ajoute les
suites anglaises et le contenu d'une AppImage de test. `scripts/smoke.sh` agit
sur la machine réelle et crée un pad virtuel : ne pas le lancer par défaut.
Voir [TESTS.md](TESTS.md).

## 7. Construire l'AppImage

```sh
LAPINUX_APPIMAGE_RUNTIME=/chemin/vers/runtime-x86_64 \
LAPINUX_APPIMAGE_RUNTIME_SHA256=<empreinte-vérifiée> \
./scripts/build-appimage.sh
```

Le script refuse un runtime sans empreinte, n'écrase pas une image existante et
écrit `dist/Lapinux-<version>-x86_64.AppImage`. Une compilation sur une
distribution récente produit un binaire qui exige sa glibc : pour une image
large, compiler dans un conteneur de la distribution la plus ancienne visée
(Ubuntu 22.04 pour GLIBC 2.35) puis vérifier avec `readelf --version-info`.

Contrôles minimaux après construction :

```sh
dist/Lapinux-<version>-x86_64.AppImage --appimage-extract-and-run --version
LAPINUX_HOME=/tmp/lapinux-appimage dist/Lapinux-<version>-x86_64.AppImage --appimage-extract-and-run cli doctor
sha256sum dist/Lapinux-<version>-x86_64.AppImage
```

## 8. Publier

1. Tout doit être commité sur `master`, tests et i18n au vert.
2. Relire [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md) : licences, runtime,
   compatibilité glibc et données personnelles dans les sources.
3. Rendre le dépôt public seulement après cette relecture.
4. Créer une étiquette `v<version>` et une release GitHub contenant l'AppImage,
   son SHA-256 et les limites connues (distributions testées, FUSE, bureau).
5. Ne jamais joindre de profil utilisateur, journal, jeton ou capture de
   conversation à une release.

## 9. Réparer un problème utilisateur

L'onglet **Réglages › Journal** ne sert plus d'abord à lire des fichiers : il
propose **Copier le rapport anonyme**. Ce JSON contient la version, le noyau, le
bureau et la session, la langue, la mémoire, le processeur, les cartes
graphiques avec leur pilote, l'état du daemon, les vérifications et des
compteurs de téléchargements. Il n'inclut ni journal brut, ni chemin, ni nom de
jeu, ni compte, ni adresse réseau. Les journaux bruts restent disponibles dans
la section repliée « Journaux bruts (avancé) ».

Demander ce rapport en premier, puis seulement les journaux si le rapport ne
suffit pas. La page [DEPANNAGE.md](DEPANNAGE.md) donne les symptômes connus.

## 10. Avant de dire « c'est fini »

- [ ] `cargo check --workspace` passe.
- [ ] Les tests touchés passent, et la suite du domaine aussi.
- [ ] `./scripts/check-i18n.sh` passe.
- [ ] L'arborescence `~/Lapinux` documentée correspond au code.
- [ ] L'AppImage se lance hors des sources (`--appimage-extract-and-run`).
- [ ] La release indique les limites vérifiées, pas seulement les succès.

# Installation

Build, dépendances, service utilisateur et permissions. Tout ce qui doit être fait une fois sur la machine.

## Démarrage

```bash
cargo build --release
./target/release/lapinux cli doctor    # diagnostic de l'environnement
./target/release/lapinux cli scan      # bibliothèques Steam
./target/release/lapinux cli protons   # Protons installés
./target/release/lapinux-probe             # sonde de l'hôte natif Steam
./target/release/lapinux-probe --session   # session locale isolée
./target/release/lapinux-probe account     # comptes : moteur local + CM anonyme
./target/release/lapinux-probe account --login <compte>  # CM nominatif (jeton, Steam Guard)
./target/release/lapinux               # interface
```

Le daemon est démarré automatiquement par l'UI et par la CLI si besoin. Pour un
service utilisateur, voir `packaging/lapinux.service`.

## Compilation et dépendances

Il faut une chaîne Rust/Cargo, un compilateur C et `pkg-config`, ainsi que les
bibliothèques et en-têtes graphiques GL/EGL, X11, Xcursor, Xrandr, Xi, Xinerama,
Wayland et xkbcommon. Les noms de paquets varient selon la distribution ; la
[liste utilisée par la CI](../.github/workflows/ci.yml) est un exemple Debian/Ubuntu.
À l'exécution, le bureau, les pilotes graphiques et les outils facultatifs
(Proton, GameScope, WebKitGTK et autres selon les fonctions) restent à fournir
ou à configurer selon les besoins.

`cargo build` utilise le profil `dev` (`target/debug`) pour développer ;
`cargo build --release` utilise le profil optimisé incrémental (`target/release`).
`cargo build --profile dist` utilise le profil de distribution avec LTO complet
(`target/dist`) ; ce choix ne garantit ni compatibilité avec d'autres versions
de glibc ni reproductibilité. `cargo test --workspace` compile et lance les
tests ; consulter [TESTS.md](TESTS.md) pour leurs effets et limites.

## Construire une AppImage

La compilation courante utilise `cargo build --release` (incrémentale). Le
profil `dist` active le LTO complet pour une distribution et peut être beaucoup
plus long. La construction d'AppImage requiert Cargo, `mksquashfs`, ImageMagick
(`convert`) et `sha256sum`. Elle inclut le pont natif expérimental ; les autres
bibliothèques système et les outils facultatifs restent ceux de la machine.

Télécharger le runtime AppImage x86-64 depuis sa source officielle et vérifier
son origine et son SHA-256 par un canal de confiance. Fournir ensuite son chemin
et cette empreinte au script :

```bash
LAPINUX_APPIMAGE_RUNTIME=/chemin/vers/runtime-x86_64 \
LAPINUX_APPIMAGE_RUNTIME_SHA256=<sha256-verifie> \
./scripts/build-appimage.sh
```

Le résultat est écrit dans `dist/` sans écraser une image existante ; un autre
chemin peut être fixé avec `LAPINUX_APPIMAGE_OUTPUT`. Une empreinte affichée
par le script identifie le résultat mais ne remplace pas une signature. Les
versions glibc et dépendances de la machine de compilation conditionnent les
systèmes compatibles : construire sur une base adaptée et tester les systèmes
visés avant diffusion. Un fichier `AppImage` existant n'est pas une preuve de
compatibilité ou de publication.

## Pont Steamworks Linux natif

Le pont est expérimental et désactivé par défaut pour chaque jeu Linux Steam.
Il s'active dans les réglages du jeu, onglet Affichage. Pour construire les
artefacts de distribution et les installer dans un nouveau dossier :

```bash
cargo build --profile dist -p lapinux -p lapinux-steamshim
bash scripts/install-artifacts.sh target/dist /chemin/vers/nouveau-dossier
```

Le dossier parent doit exister ; le script refuse tout dossier de destination
existant et copie `lapinux`, `lapinux-probe` et `libsteam_api.so` ensemble.
Un `cargo build --release` produit aussi la bibliothèque dans `target/release` ;
il faut la distribuer **avec** le binaire `lapinux`, dans le même dossier.
`cargo install` ne copie pas automatiquement la bibliothèque. Autre possibilité : le placer dans le dossier
de données Lapinux sous le même nom. `LAPINUX_STEAMSHIM` peut indiquer un
chemin explicite au daemon, mais ne remplace pas l'activation par jeu. Une
bibliothèque absente, un chemin non UTF-8 ou contenant des espaces ou `:`, un
ELF tronqué, un SONAME incorrect ou une architecture autre que x86-64 bloque
explicitement le lancement. Ces contrôles ne garantissent ni l'identité de la
bibliothèque ni la résolution de toutes ses dépendances. Les lanceurs scripts sont acceptés,
mais l'architecture de leur exécutable final n'est pas vérifiable au préalable.
Aucun fichier du jeu n'est remplacé ; les autres valeurs `LD_PRELOAD` sont
conservées. Le préchargement ne couvre pas nécessairement un jeu qui utilise
`dlopen` avec un chemin absolu, un namespace isolé, un binaire statique ou une
bibliothèque Steamworks d'un autre nom.

Une session Steam enregistrée par Lapinux est nécessaire. Le pont prend en
charge une partie d'`ISteamUserStats` versions 012 et 013 : lecture/écriture
locale des statistiques entières et flottantes et des succès, et mise en file
des succès confirmés par `StoreStats`. L'initialisation plate, les contextes
C++ et certains appels User, Utils, Apps et Friends ont aussi des réponses
partielles ; cela ne couvre pas toutes les méthodes ni toutes leurs versions. La file
est liée au SteamID de la session et le daemon la transmet à Steam lorsqu'il
peut ; `StoreStats` ne prouve donc pas que Steam a accepté l'envoi. Si l'écriture
locale échoue, `StoreStats` renvoie `false` et le jeu peut réessayer. Les lots
sont retentés en cas d'erreur, compte différent ou succès inconnu du schéma ;
un arrêt brutal de la machine n'est pas couvert par une garantie transactionnelle.
Les stats numériques restent en mémoire et ne sont pas synchronisées ; les valeurs Steam
précédentes ne sont pas relues, les callbacks Steam ne sont pas implémentés et
les autres interfaces sont partielles ou neutres. Certaines méthodes, versions,
protections ou dépendances peuvent échouer ou faire planter un jeu : désactiver
le pont par jeu dans ce cas. Aucun fonctionnement universel n'est garanti.

Test reproductible de l'installation de production, sous Linux x86-64 avec un
compilateur C et `bwrap` (namespaces utilisateur, PID et réseau autorisés) :

```bash
cargo build --profile dist -p lapinux -p lapinux-steamshim
LAPINUX_TEST_INSTALLED_DIST=1 cargo test -p lapinux-daemon --test native_steamshim -- --nocapture
```

Le script d'installation est exécuté dans un dossier temporaire sous `target`.
Le daemon `lapinux --daemon` installé y lance un client C et une bibliothèque
Steam factices, en environnement isolé ; le test vérifie le chemin du fournisseur
des symboles, pont activé et désactivé, les variables déjà présentes, la mise en
file avec une session factice et les erreurs de bibliothèque absente ou
incompatible. Il ne teste ni un jeu réel ni une synchronisation Steam. Pour les régressions de la file et du lancement :

```bash
cargo build -p lapinux-steamshim
cargo test -p lapinux-daemon --test native_steamshim
cargo test -p lapinux-daemon native_spool_keeps_batches_when_account_changes_or_session_is_unavailable
cargo test -p lapinux-perf native_preload_is_applied_to_game_not_scene_or_gamescope
```

Utiliser le même `CARGO_TARGET_DIR` pour toutes les commandes ; le test
standard attend le pont compilé dans son profil, et le test d'installation
attend les artefacts dans le profil `dist`.

Tout porte le nom Lapinux : binaire, sonde, service, règles udev et variables
d'environnement. Les réglages, le catalogue et les bibliothèques vivent dans
`~/.config/lapinux`, `~/.local/share/lapinux` et `$XDG_RUNTIME_DIR/lapinux`, et
les variables sont toutes préfixées `LAPINUX_` (`LAPINUX_HOME`, `LAPINUX_LOG`,
`LAPINUX_STEAMCLIENT`, `LAPINUX_STEAM_CM`).

`Shift+Tab` est capturé par KWin pendant la session (petit script chargé par le
daemon) puis retiré à la fin : il fonctionne sans aucune permission `/dev/input`,
comme le montre `lapinux cli doctor`. Seuls le bouton **Guide** de la manette et
le pad virtuel lisent `/dev/input` via evdev. Si cet accès est refusé, installer
une fois la règle udev :

```bash
sudo cp packaging/99-lapinux-input.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input
```

## Stockage et cache

Par défaut, tout est rangé sous `~/Lapinux` : `LapinuxLibrary` pour les jeux
gérés (chemin choisi dans les réglages prioritaire), `Cache` pour ce qui se
supprime sans conséquence, et `Sauvegardes` pour les copies de jeux
(`<jeu> (<appid>)`) et les profils de manettes (`Manettes/`). Les sauvegardes
originales restent dans les dossiers des jeux ou préfixes. `LAPINUX_ROOT`
déplace le parent ; `LAPINUX_HOME` et les variables XDG déplacent les dossiers
internes.

Les nouvelles jaquettes choisies sont dans `data/artwork`, hors du cache ; les
anciennes restent lisibles dans l'ancien dossier de cache et sont préservées au
vidage.
Le bouton de nettoyage conserve aussi les installateurs de boutiques dans
`cache/stores`, pour ne pas interrompre une installation ou reprise. Certains
caches ne sont pas inclus dans ce bouton ; une erreur est signalée dans les
réglages. Toujours sauvegarder les données importantes séparément.

## Steam Controller 2026 native

Le pont `steam-puck-bridge` n'est pas nécessaire avec un pilote `hid-steam`
compatible qui expose la manette comme gamepad. Linux 7.3 apporte ce support ;
des distributions peuvent aussi le fournir par rétroportage. Il faut que la
manette soit allumée et accessible et qu'aucun service externe ne monopolise
le périphérique ; Lapinux ne l'arrête pas seul. Si le service utilisateur
`steam-puck-bridge.service` est installé, la carte **Pont Steam Controller**
dans **Manettes → Appareils** permet de l'arrêter temporairement ou de le redémarrer. Cet
arrêt ne modifie pas son activation automatique à la connexion ; sans pilote
natif compatible, la manette peut cesser de fonctionner. La valeur `lizard_mode=1`
autorise l'émulation au repos : elle ne prouve pas que le pad est muet pendant
qu'un programme l'utilise.

Le remappage Lapinux peut présenter un pad Xbox virtuel au jeu ; il est activé
si la capture, un profil de jeu ou un profil Steam Input est actif. Pour
laisser le jeu accéder au périphérique natif, désactive la capture **et**
retire les profils actifs pour ce jeu. En mode remappé, la croix et les
gâchettes natives sont converties ; les palettes ne produisent une sortie que
si elles sont affectées à un bouton Xbox compatible. Les gestes de pavés, les
actions avancées Steam Input, le gyroscope et le retour de vibration ne sont
pas transmis par ce pad virtuel. L'affectation physique des palettes dans les
profils Steam reste à confirmer sur la manette. Vérifie la compatibilité du jeu.

En mode remappé, Lapinux initialise les entrées avant le lancement et ne masque
que les sources ouvertes. Si une capture est refusée au démarrage, le service
de remappage s'arrête et les périphériques physiques restent accessibles au jeu.
Les boutons et axes du pad virtuel sont neutralisés à l'ouverture de l'overlay
puis relus à sa fermeture ou à la reconnexion. Le masquage repose sur `bwrap`
et les nœuds présents au lancement : l'accès et la reconnexion dépendent aussi
du pilote, des permissions et du jeu.

Pour désactiver aussi l'émulation clavier/souris au repos, il est possible de
configurer `hid-steam` au démarrage :

```bash
sudo cp packaging/99-steam-controller.conf /etc/modprobe.d/
```

Redémarrer ensuite la machine, ou recharger le pilote si aucun programme ne
tient la manette : `sudo modprobe -r hid_steam && sudo modprobe hid_steam`.

Pour basculer ce mode à chaud depuis Manettes › fiche de la manette, utiliser
l'interrupteur **Mode clavier/souris (lizard)**. S'il est grisé, le bouton
**Installer la règle** demande l'autorisation administrateur au bureau et
installe la règle udev sans terminal. Celle-ci donne au groupe `wheel`
l'écriture sur ce paramètre (les membres de `wheel` peuvent déjà élever leurs
privilèges).

Sans interface graphique, la même règle peut être installée à la main :

```bash
sudo cp packaging/99-steam-controller.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=input
```

L'interrupteur **Mode clavier/souris (lizard)** s'active alors sans root et
s'applique immédiatement, sans redémarrer. Le bouton **Rendre natif au
démarrage** installe `99-steam-controller.conf` avec la même autorisation :
`hid_steam` démarrera alors en pad natif à chaque redémarrage. Sans ce fichier,
le mode natif n'est que celui de la session courante.

Pour retrouver le lapin dans le menu d'applications et la barre des tâches :

```bash
install -Dm644 assets/lapinux-icon.png \
  ~/.local/share/icons/hicolor/512x512/apps/lapinux.png
install -Dm644 packaging/lapinux.desktop \
  ~/.local/share/applications/lapinux.desktop
update-desktop-database ~/.local/share/applications
```

---

[Documentation](README.md) · [README du projet](../README.md)

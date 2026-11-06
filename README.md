# Lapinux

Launcher de jeux en Rust pour Linux (KDE Wayland / Bazzite), sans client Steam :
bibliothèque Steam, lancement via Proton, couche manettes, mode jeu et quick menu
en overlay.

Binaire `lapinux` (la sonde de l'hôte natif Steam est `lapinux-probe`) :

| Mode | Commande | Rôle |
|---|---|---|
| UI | `lapinux` | bibliothèque, fiches de jeu, téléchargements, Proton, manettes, big picture |
| Daemon | `lapinux --daemon` | scan, IPC, lancement, sessions, jobs, overlay |
| Overlay | `lapinux overlay` | quick menu layer-shell au-dessus du jeu (lancé par le daemon) |
| CLI | `lapinux cli …` | diagnostic, scan, installation, lancement, sessions |

## État vérifié sur cette machine

- Scan réel : **91 jeux** dans les bibliothèques Steam, pochettes et manifests lus.
- Lancement réel validé : **BONK THE BOYKISSER** via GameScope + GE-Proton 11-7,
  préfixe Steam existant réutilisé, session suivie puis arrêtée proprement
  (`lapinux cli launch 4119610`, `lapinux cli stop`).
- Coût en release mesuré par `scripts/perf.sh` : binaire **12,2 Mio**, daemon
  **9,2 Mio RSS, 0 % CPU au repos**, IPC **3,0 ms**, scan complet **248 ms**.
- `cargo test --workspace` : tous verts (catalogue natif, manifestes, scan,
  dry-run, lancement Proton réel et rescan).
- Lanceur du menu d'applications testé (`gtk-launch lapinux`, binaire résolu via
  `~/.local/bin/lapinux`).
- Catalogue Steam vérifié de bout en bout avec un compte réel, sans SteamCMD :
  1 928 paquets de licences lus par le canal CM, 1 158 jeux retenus, DLC et
  outils écartés, cache `steam-catalog.json` écrit et relu par l'IPC. Chaque
  fiche PICS porte éditeur, genres, langues, dates, avis, Metacritic, DLC,
  build public, poids par plateforme et illustrations officielles du CDN
  Steam (aucun service tiers).
- Première synchronisation du catalogue mesurée : ~38 s pour 1 158 jeux, en
  arrière-plan ; les suivantes relisent le cache.
- Quick menu réel (KWin 6.7, Wayland) : `Shift+Tab` enregistré pendant la
  session (clavier physique illisible sous `/dev/input`) ouvre le menu, le
  referme et le rouvre ; rien ne s'ouvre au lancement du jeu, et `stop` ferme le
  menu puis retire le raccourci de kglobalaccel.

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

Pour la Steam Controller 2026, démarrer `hid-steam` en mode manette plutôt
qu'en émulation clavier/souris (sinon le pad natif et le gyroscope restent
muets) :

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

## Fonctionnalités

- **Connexion Steam** : l'écran de connexion utilise Lapinux par jeton
  (service `Authentication`, Steam Guard, jeton de refresh en `0600`) : une
  connexion réussie se refait ensuite sans mot de passe, et un jeton obtenu
  ailleurs peut être collé dans le champ prévu pour se connecter directement.
  Une connexion par **QR code** est proposée dans Réglages › Comptes : le code
  s'affiche, l'application Steam du téléphone l'approuve, aucun mot de passe
  n'est saisi. Le jeton obtenu est réutilisé tel quel à chaque lancement, sans
  redemander quoi que ce soit : Steam le considère valable environ 200 jours.
  L'écran signale un jeton déjà enregistré et permet de l'oublier. Aucun repli
  vers SteamCMD ni vers le client Steam.

  Steam ne garde **qu'une session par compte** : deux logons rapprochés se
  ferment l'un l'autre, et trop de tentatives finissent par un refus
  temporaire (`mot de passe invalide`, eresult 5) même avec un jeton valide.
  D'où quatre garde-fous : un jeton **tourné** par Steam est réenregistré
  aussitôt ; un refus par jeton ou par QR est **retenu cinq minutes** au lieu
  d'être redemandé à chaque clic (un essai par mot de passe, lui, reste
  immédiat) ; les lectures par jeu (fiche, succès) sont **gardées cinq
  minutes** ; et surtout **une seule session** sert toutes les lectures —
  fiche, succès, temps de jeu, statistiques, catalogue et vérification du
  jeton. Elle vit sur un **moteur réseau commun** au processus : gardée sur le
  runtime jetable d'une fiche, sa connexion mourait avec lui et la session
  « réutilisée » était déjà déconnectée, d'où un logon par lecture (deux fiches
  de suite : 1 logon mesuré, contre 2 avant).
- **Bibliothèque** : lecture des `libraryfolders.vdf`, `appmanifest_*.acf`,
  résolution de l'exécutable, détection DRM Steam (sections PE `.bind` +
  `steam_api64.dll`), pochettes mises en cache. `general.library_root` est une
  bibliothèque à part entière : elle est scannée comme les racines Steam, son
  origine (`Steam` ou installation gérée) est persistée, et les jeux absents d'un scan
  momentané sont conservés au lieu d'être effacés.
- **Boutiques** : Réglages › Comptes réunit Steam, **Epic Games**, **GOG** et
  **itch.io** dans une même liste, un compte par boutique, chacune ouverte par
  sa **marque blanche** (`assets/stores/`). Ces marques sont des silhouettes
  tirées des logos d'origine : à vingt pixels, les lettrages complets
  (« EPIC GAMES », « gog.com », « itch.io ») ne faisaient qu'une bouillie — le
  badge Epic, le cadre GOG et le bonnet itch restent, eux, lisibles. Epic et GOG
  s'autorisent dans le navigateur (code d'autorisation collé à la suite),
  itch.io se connecte par une **clé d'API** — la page où la créer s'ouvre
  depuis la carte du compte ; enregistrer un compte répond **tout de suite**,
  et la relecture de sa bibliothèque part en tâche de fond (elle prenait des
  minutes chez Epic, au-delà du délai de l'interface : la connexion semblait
  échouer alors que le jeton était écrit) ; les jetons sont écrits en `0600`
  sous `data/stores/<boutique>.json`, remplacés d'un bloc et rafraîchis quand
  ils expirent. Chaque boutique n'annonce que ce qu'elle expose réellement —
  ces capacités sont des faits de l'API, affichés en pastilles, pas des
  réglages :
  - **Epic** publie la bibliothèque du compte, les **sauvegardes cloud**
    (`datastorage…/api/v1/access/egstore/savesync/…`, le service qu'utilisent
    Legendary et Heroic) et les **succès** (GraphQL de la boutique :
    définitions, progression du compte, rareté mondiale). Il **s'installe**
    aussi : l'API du launcher résout le build, son **manifeste binaire** liste
    les fichiers découpés en chunks, et les chunks se téléchargent depuis le
    CDN puis se recollent à leurs décalages — le même chemin que Legendary.
    Sous Linux, un jeu sans build Linux prend son build Windows (Proton).
  - **GOG** ajoute l'installation hors ligne à ses **sauvegardes**
    (`cloudstorage.gog.com/v1/<compte>/<client du jeu>`, avec les identifiants
    de client lus dans les métadonnées du build — c'est ce que fait `gogdl`) et
    à ses **succès** (`gameplay.gog.com/clients/<jeu>/users/<compte>/achievements`).
  - **itch.io** a la bibliothèque et l'installation : ni cloud ni succès, son
    API n'en expose aucun.

  Les succès d'un jeu de boutique remplissent la même bande que ceux de Steam,
  icônes et pourcentages compris, et le lien de relecture nomme la boutique
  (« Relire depuis Epic Games »). Les sauvegardes se lisent et s'envoient
  depuis les réglages du jeu, avec les mêmes boutons que Steam : le jeu de
  boutique désigne son **dossier de sauvegarde** (champ dédié, `sauvegardes/`
  dans son dossier par défaut) puisque, contrairement à Steam, ni Epic ni GOG ne
  publient le chemin où le jeu écrit sous Proton. Une fois ce dossier
  renseigné, la synchronisation se fait aussi **d'elle-même** : le cloud est
  repris avant le lancement (seuls les fichiers absents ou plus anciens que lui)
  et ce que la partie a modifié repart à la sortie — sans bloquer le jeu en cas
  d'échec réseau, le message allant au journal.

  La bibliothèque lue est gardée quinze
  minutes puis relue ; le bouton « Relire » passe outre. Un jeu de boutique
  s'installe dans `general.library_root`, comme un jeu géré par Lapinux :
  une tâche de téléchargement avec sa progression et son annulation, puis une
  marque `.lapinux-store.json` dans le dossier — c'est elle qui empêche le
  scan suivant de prendre un jeu GOG pour un jeu Steam. Exécutable et version
  (`linux`, `windows`, `macos`) sont retenus à l'installation : le jeu se
  lance ensuite comme les autres, Proton compris pour une version Windows.
- **Notifications de succès** : pendant une partie, un **bandeau** se pose en
  haut de l'écran dès qu'un succès tombe — l'icône du succès à gauche, le nom
  du jeu et sa boutique, son titre, et la part des joueurs qui l'ont obtenu.
  L'icône tombe au centre puis glisse à gauche pour découvrir le texte ; les
  écritures entrent l'une après l'autre. Après six secondes, le texte s'efface,
  l'icône revient au centre et remonte hors de l'écran ; la surface ne prend ni
  la souris ni le clavier, le jeu n'est pas interrompu. La surveillance relit les succès du jeu **toutes les 45
  secondes** sans passer par le cache de la fiche, pour Steam, Epic Games et
  GOG (itch.io n'en publie aucun) ; un succès obtenu avant le lancement mais
  synchronisé en retard n'est pas annoncé. Les bandeaux s'affichent l'un après
  l'autre, quatre au plus en attente. La liste complète des succès d'un jeu
  porte un bouton **Tester la notification** qui affiche le bandeau d'un
  succès réel, icône et rareté comprises.
- **Bibliothèque Steam du compte** : après une connexion unique, la session CM
  rend les licences du compte, puis PICS étend les paquets en appids et fournit
  nom, type et fiche complète (par lots de 50). Le propriétaire de chaque
  licence permet de distinguer les jeux possédés de ceux prêtés par la famille
  Steam, et le **compte personnel l'emporte** : un jeu que le compte possède
  reste le sien même si un autre membre de la famille le partage aussi. PICS ne
  décrit pas tous les paquets — une licence partagée répond souvent par une
  fiche vide, et l'application elle-même peut refuser sa fiche au compte qui ne
  la possède pas : la **boutique publique de Steam**, sans clé ni compte,
  complète alors la liste (`packagedetails` pour les appids d'un paquet et
  leurs noms, `appdetails` pour le type). C'est ce qui fait apparaître les jeux
  de la famille qui manquaient à la bibliothèque. La **bibliothèque du groupe
  familial** est en plus demandée à `FamilyGroups.GetSharedLibraryApps` avec le
  jeton d'accès de la session : la liste de licences ne porte pas tous les jeux
  prêtés — Steam n'y ajoute une licence qu'au moment de l'emprunt — et ceux-là
  n'apparaissaient jamais, pas même sous leur nom. Ce service donne les appids
  et les noms du groupe ; les fiches se demandent ensuite à PICS et à la
  boutique comme pour les autres jeux, et son nom sert de dernier recours. Le
  poids se calcule dépôt par dépôt des manifestes publics, en s'écartant des
  éditions régionales non achetées : la restriction aux dépôts connus du compte
  vaut **jeu par jeu**. Un jeu dont le paquet n'est pas décrit par PICS (celui
  de la famille) n'a pas cette liste : ses dépôts **anonymes** — ni plateforme,
  ni langue, ni DLC, les seuls qui puissent être comptés deux fois — sont alors
  soumis au protocole, qui ne rend la clé de déchiffrement d'un dépôt qu'aux
  licences qui le couvrent (`ClientGetDepotDecryptionKey`). Les dépôts d'une
  autre édition sont écartés du poids : Resident Evil Requiem annonçait 169,7 Go
  pour 83,4 Go réels, Elden Ring 56,2 pour 55,1, Nightreign 28,1 pour 27,4.
  Sans aucun dépôt autorisé, le refus ne prouve rien (les outils gratuits comme
  Proton refusent toutes leurs clés) : la fiche garde alors le poids publié
  plutôt que d'annoncer zéro octet. La fiche affiche tout ce que Steam publie : développeur, éditeur,
  genres, fonctionnalités, descripteurs, dates, plateformes, manette, Deck,
  Metacritic, avis, langues et doublages, DLC, build public, branches, cloud,
  langue par défaut, poids des dépôts par plateforme et bannière, capsule,
  logo et icône du CDN Steam. La **langue par défaut** est celle que Steam
  utiliserait, lue dans le registre du client
  (`HKCU/Software/Valve/Steam/language`) et non dans la locale du bureau : le
  protocole publie l'inventaire des langues (`supported_languages`,
  `baselanguages`, dépôts `language`) mais pas le choix du joueur, que le
  client envoie lui-même au CM (`CMsgClientLogon.client_language`). C'est elle
  qui retient le dépôt de doublage : le poids annoncé est donc celui du
  téléchargement réel, repli sur l'anglais quand le jeu ne la publie pas. Quand la page magasin n'est pas publique (jeu retiré ou pas encore
  sorti), ces illustrations sont lues dans le cache local du client Steam
  (`appcache/librarycache`), seule source officielle restante. Les jeux sont
  fusionnés directement dans la bibliothèque, avec recherche et filtres
  (à installer / installés / tous, puis sources Disque / Steam / Famille
  Steam / Tout Steam) ; il n'y a pas d'onglet Steam séparé. Cliquer « Télécharger » lance
  l'installation native, « Jouer » démarre la session Proton. DLC et outils
  sont écartés, le catalogue est mis en cache dans
  `~/.local/share/lapinux/steam-catalog.json` et la synchronisation tourne en
  arrière-plan (l'interface sonde, rien ne se bloque).
- **Installation native** : connexion unique avec 2FA, mot de passe jamais
  persistant, téléchargement des dépôts, manifestes et chunks directement par
  Lapinux, suivi de progression par job. Seuls les jeux possédés sont
  téléchargeables. La version choisie dans la flèche de la fiche est celle
  dont les dépôts partent, contenus additionnels compris ; la tâche la
  conserve, si bien que la file d'attente et une pause/reprise installent la
  même, et une plateforme que le jeu ne publie pas laisse simplement le moteur
  revenir à ce qu'il sait télécharger. Le manifeste écrit par Lapinux porte un marqueur d'origine.
  Dans une bibliothèque gérée, les jeux Steam vivent sous `Steam/<jeu>`, leurs
  manifestes à côté, et les préfixes dans `CompatData/<jeu> (<appid>)` ; les
  rayons des boutiques restent à la racine. Une vraie bibliothèque Steam garde
  son organisation (`steamapps/common`, `steamapps/compatdata`). Une mise à jour
  d'un jeu déjà installé se fait **dans sa bibliothèque d'origine** ; un jeu
  Steam garde son manifeste, son `compatdata` et son préfixe, seul le `buildid`
  installé est réécrit.
- **Mises à jour Steam** : le `buildid` public lu dans PICS est comparé au
  manifeste de chaque jeu installé ; les jeux en retard portent un badge « MAJ »
  et une section « Mises à jour » les liste dans Téléchargements, avec le poids
  du téléchargement et un bouton « Mettre à jour » par jeu. Le téléchargement
  incrémental réutilise la vérification des dépôts et ne lance jamais le client
  Steam.
- **Proton** : détection dans Steam et `compatibilitytools.d`, téléchargement des
  releases GitHub GE-Proton / Proton-CachyOS avec vérification SHA-512,
  extraction pure Rust et installation dans `compatibilitytools.d`.
- **Lancement** : `proton run` direct avec l'environnement Steam
  (`STEAM_COMPAT_*`), `umu-run` pour les préfixes Lapinux quand le Proton choisi
  est complet (`toolmanifest.vdf`) ; un Proton local partiel est lancé par son
  script. Les préfixes Steam existants ne passent **jamais** par umu (umu réécrit
  le fichier `version`, Steam considère alors le préfixe comme invalide). Pour
  les jeux Steamworks, le client Steam officiel est démarré sans fenêtre
  (`-silent`) et Lapinux attend qu'il soit prêt avant de lancer le jeu ; un
  client déjà lancé est réutilisé. Un build Linux natif n'est pas pris pour
  un jeu sans DRM : `libsteam_api.so` vaut `steam_api64.dll`, et l'appid lui
  est transmis par `SteamAppId`. Il doit avoir une session enregistrée, sinon
  Steam ouvre sa fenêtre de connexion. La requête part en tâche de fond : dès
  que la commande est prête et que le jeu n'a pas encore affiché
  sa première image, la fenêtre se couvre et le **lapin animé**
  ([launch.rs](crates/ui/src/launch.rs)) attend avec le nom du jeu — plein
  écran compris. C'est vrai pour **tous les jeux**, Steam comme boutiques :
  une réponse rapide du daemon (jeu de boutique) ne supprime plus l'attente,
  elle est seulement peinte après une grâce de 0,3 s pour ne pas clignoter si
  le jeu arrive tout de suite. Sa planche (`assets/bunny-sheet.png`, copiée de
  `Bunny/`) range **une animation par ligne**, en cases de 32 px (11 colonnes,
  34 lignes) : toutes sont jouées, l’image avançant à sept par seconde, puis
  **un repos** d’1,6 s sur la première image avant l’animation suivante — une
  attente longue ne montre donc pas une boucle unique. Le même lapin remplit
  les encarts qui lisent Steam (résumé, succès, actualités) au lieu d’un point
  qui tourne. La case fait 32 px, pas 16 : découper au quart n'affichait
  qu'une oreille et un bout d'œil, et une case tranchée dans un sprite touche
  son bord, ce que le test vérifie sur toute la planche.
- **Mode jeu** : GameScope (écran choisi dans les réglages, résolution et cap
  FPS repris par défaut du mode courant de l'écran principal, HDR, adaptive
  sync), MangoHud configurable par jeu, restauration à la sortie, journal par
  session.
- **Manettes** : lecture evdev, profils de mapping, pad virtuel uinput, navigation
  de la bibliothèque au pad. La page Manettes affiche la liste à gauche (icône
  de manette, nom, connexion filaire USB, Bluetooth ou dongle 2,4 GHz, niveau de
  batterie en cercle) et, à droite, la fiche et le test en direct de la manette
  sélectionnée ; les connectées précèdent les appairées hors ligne. La marque
  (PlayStation, Xbox, Nintendo, Steam) est reconnue par les identifiants USB,
  son logo suit le nom et le modèle est renommé quand il est sûr
  (« Wireless Controller » devient « DualShock 4 », « Pro Controller » devient
  « Switch Pro Controller »). Toute manette dont la marque n'est pas
  identifiable est traitée comme une manette Xbox (A/B/X/Y, View/Menu, logo
  Xbox) sans perdre son nom brut : un adaptateur PS1/PS2 ou un clone exotique
  reste donc utilisable. Un pad `045e:028e` dont le fabricant USB n'est pas
  Microsoft (clone ou pont) garde la famille Xbox 360 mais est nommé d'après sa
  liaison, « Manette 2,4 GHz », au lieu d'être présenté comme une Xbox 360
  officielle. Les boutons enfoncés apparaissent aussi dans la liste, à droite de
  la marque. Les cartes du test en direct ne s'affichent que pour les entrées
  réellement exposées : pas de gâchettes ni de sticks inventés sur un clone.
  Les dongles et adaptateurs connus sont reconnus par
  leur identifiant USB (Steam Puck, récepteurs Xbox 360/Xbox sans fil, Logitech
  F710, DualShock 4, 8BitDo, Mayflash, Brook, Pelican) et, à défaut, par les
  chaînes « receiver », « dongle », « puck », « wireless adapter » ou « 2,4 GHz ».
  La Steam Controller 2026 (« Triton ») est reconnue sur ses quatre identifiants
  Valve (`28de:1302` filaire, `1303` Bluetooth LE, `1304` Puck, `1305` récepteur
  Nereid) et affichée « Steam Controller 2026 » : le pilote `hid-steam` du noyau
  7.3 expose ses sticks, ses pavés, ses palettes M1–M4, son bouton d'accès
  rapide et son gyroscope ; sa batterie arrive par `power_supply` et sa
  vibration par le force feedback evdev, comme sur les autres manettes. Par
  défaut, `hid-steam` la laisse toutefois en mode clavier/souris : elle apparaît
  alors comme un clavier sur le Puck et le pad natif ne reçoit ni boutons ni IMU.
  `packaging/99-steam-controller.conf` (`options hid_steam lizard_mode=0`) la
  démarre directement en mode manette. Tant
  qu'aucun gamepad n'est exposé, la page signale le Puck et le pont
  `steam-puck-bridge`. Quand ce pont prend le hidraw du Puck, il crée un pad
  uinput `045e:028e` nommé « Microsoft X-Box 360 pad » : Lapinux reconnaît ce
  pad virtuel et l'affiche aussi « Steam Controller 2026 », avec la mention du
  pont, au lieu de le présenter comme une Xbox 360.
  La batterie est lue sur `power_supply`, dans les rapports HID (DualSense,
  Switch) et via BlueZ ; les palettes, boutons constructeur et pavés tactiles
  exposés par evdev sont listés et testables, même sur une manette inconnue.
  Si le pilote ne fournit aucun nom, la carte utilise la liaison comme nom de
  secours : « Manette USB », « Manette Bluetooth », « Manette 2,4 GHz » pour un
  dongle, ou « Manette virtuelle » pour un pad uinput.
  Le profil
  (zone morte, courbe,
  inversions, raccourci) s'édite sous le test, et un jeu peut choisir le sien.
- **Page Boutiques** : ouverte depuis la barre du haut, elle mène à la
  boutique elle-même, pas à la liste des jeux possédés — ceux-ci restent dans
  la Bibliothèque. La liste des **quatre boutiques** tient à gauche, une ligne
  par enseigne avec sa marque et le point vert ou rouge de son compte ; à
  droite, son identité, l'état du compte et la porte vers **le site de la
  boutique**. egui ne sait pas rendre du HTML, et une vue rendue hors écran ne
  reçoit aucun rafraîchissement : les animations n'y avancent pas, et chaque
  geste devait passer par une capture — un défilement saccadé pour un coût de
  processeur permanent. La page est donc confiée à **WebKitGTK 6** (GTK 4), le
  moteur de GNOME Web, dans **sa propre fenêtre** : un petit hôte Python
  (`crates/ui/src/webview_helper.py`, embarqué dans le binaire et écrit dans
  le dossier d'exécution) l'ouvre et la pilote par lignes JSON — chargement,
  retour, rechargement, premier plan. Le défilement, les animations, le clavier
  et les formulaires y sont ceux d'un navigateur, puisque c'en est un ; rien
  n'est capturé ni transmis, et la pile est **native Wayland** — aucun XWayland
  n'est nécessaire, l'application reste native. La fenêtre se ferme par son
  propre bouton, Lapinux le voit et remet son panneau à zéro ; une seule
  fenêtre sert aux quatre boutiques, changer de magasin y change la page. Les
  vidéos y jouent comme dans un navigateur, autoplay compris ; les
  bandes-annonces DASH/HLS de Steam sont activées par deux variables WebKit,
  avec les greffons GStreamer déjà présents, sans paquet en plus. Sur NVIDIA,
  les sinks vidéo DMA-BUF et GL de WebKit peignent des images vertes : le
  sink logiciel est forcé, quitte à décoder sur le processeur. Sur la page
  Steam, la boucle d'accueil peut donc coûter un cœur entier, fenêtre laissée
  ouverte. Sans python3-gobject ni WebKitGTK 6, le panneau affiche la raison
  et propose d'ouvrir le site dans le navigateur.
- **Boutiques › Jeux gratuits** : sous les quatre enseignes, une entrée à part
  liste les offres du moment chez **Epic Games** — visuel, éditeur, valeur
  d'origine, échéance (« se termine dans 2 j 4 h ») — et celles annoncées pour
  la semaine suivante. La liste vient du flux public de la boutique : elle
  s'affiche sans compte connecté. **Panier** réunit toutes les offres encore
  récupérables — PC, Android et iOS dans la même liste — cochées par défaut :
  décocher ce qu'on ne veut pas, puis valider, rattache chaque offre au compte
  et rapporte le verdict de chacune (Epic peut en refuser une sans refuser les
  autres) ; un clic sur une carte ouvre l'offre dans la fenêtre de boutique.
  Les offres PC viennent du flux de la boutique ;
  les **versions Android et iOS** de la semaine viennent du service mobile et
  s'affichent à leur suite, avec leur pastille de plateforme. Les offres déjà
  au compte sont lues dans les droits Epic et s'affichent « Récupéré », sans
  commande ni erreur. Quand Epic répond qu'il attend un passage en caisse, la
  carte le dit et sa page rejoint la file des offres à terminer, ouverte dans
  la fenêtre de boutique **une par une** : à chaque fermeture, Lapinux relit les
  droits pour confirmer la récupération, puis ouvre l'offre suivante. Une offre
  mobile se termine toujours sur cette page, d'où son bouton « Sur la
  boutique ». La fenêtre garde ses témoins de connexion d'une ouverture à
  l'autre : une session Epic ouverte une fois suffit à terminer les passages en
  caisse. L'offre est liée au compte, pas à une machine.
- **Profil Steam** : une session sociale vit avec le daemon tant que Lapinux
  tourne — le compte reste donc visible de ses amis, et la présence choisie
  (En ligne, Absent, Invisible, Hors ligne) est publiée au CM puis confirmée
  par Steam. La page Profil se coupe en deux : la liste des plateformes à
  gauche — **Steam, Epic Games, GOG et itch.io**, chacune avec sa marque
  blanche et l'état de son compte (le **pseudo masqué** derrière des points,
  qu'un œil au bord de la ligne dévoile, ou « Non connecté ») — et à
  droite la fiche de la plateforme choisie. Steam a la sienne ; la ligne d'une
  boutique mène à son compte dans Réglages › Comptes, d'où elle se branche. Sous
  un filet, deux pages ont leur propre ligne : **Discord** (Rich Presence : en
  partie et sur une fiche, elle se publie sous l'application officielle du jeu,
  qui en donne le nom et la jaquette — cadence du moment et sa moyenne, version
  de Proton, temps écoulé ; une fiche annonce « Regarde le jeu … » et ajoute sa
  boutique et son temps de jeu ; un téléchargement y ajoute sa progression, son
  débit et la jaquette du jeu ; les pages de l'application se publient sous
  l'application Lapinux, avec leur phrase et la marque de la boutique ouverte,
  seul titre que Discord leur donne ; un jeu absent du catalogue reste sans
  présence en jeu)
  et **Activité** (les parties et les
  téléchargements, en cartes de jeu). La page n'a qu'une sorte de
  catégorie : le **jour**, repliable — aujourd'hui, hier, puis la date entière.
  Une partie et un téléchargement du même jour s'y retrouvent ensemble, le jeu
  en cours et le transfert en cours en tête, puis du plus récent au plus
  ancien. Chaque partie datée y dit son **lancement et sa fermeture** et porte
  le **journal de la session** : copiable d'un bouton, sans ouvrir le panneau,
  ou déplié dans celui-ci. Les dates se lisent sur l'horloge locale — minuit à Paris n'est pas
  la veille à Greenwich, et l'en-tête du jour ne doit pas non plus l'être. Sous
  chaque partie, la **courbe de cadence** commence à droite de la jaquette —
  la colonne du titre — et court jusqu'aux **bornes à sa droite**, sur la même
  ligne ; elle se colore selon la cadence — verte, jaune sous 90 % du maximum,
  rouge sous 70 % — et le curseur y lit, dans une étiquette posée près de la
  souris, l'heure et les images par seconde du point survolé. Les nouvelles
  parties rejoignent la liste dès leur fermeture, sans redémarrer. La fiche se range elle-même en
  deux colonnes : à gauche l'identité (pseudo, menu de présence, avatar,
  SteamID, nom réel et visibilité, description) puis les commentaires ; à
  droite la famille Steam (nom du groupe et membres) puis les amis (nom, état,
  jeu en cours, dernière connexion, avatars, hors ligne repliés). Les
  notifications n'occupent plus la fiche : un bouton les ouvre dans leur propre
  fenêtre, avec le détail de ce qui attend. Les succès restent dans la feuille
  d'un jeu. Sur une fenêtre étroite, la liste passe au-dessus de la fiche, et
  ses deux colonnes s'empilent. Les pages
  publiques du profil (fiche XML, commentaires, noms de la famille) sont
  demandées une à une et espacées : quand Steam répond 429, la pénalité protège
  les requêtes suivantes et la fiche se relit toute seule quelques dizaines de
  secondes plus tard, plutôt que de rester vide. La liste d'amis est
  demandée à `IFriendsListService` avec le jeton d'accès de la session : le CM
  ne pousse la sienne qu'une fois, groupée avec la réponse au logon, et les
  noms et états suivent par les poussées `ClientPersonaState`. Le jeton de
  refresh est entretenu par `Authentication.GenerateAccessTokenForApp` (Steam
  n'en rend pas toujours un neuf : celui du disque reste alors valable), reste
  en `0600` et n'est jamais affiché.
- **Interface** : écrans principaux Bibliothèque, Profil, Téléchargements et
  Manettes ; Protons, Système et Diagnostic sont des onglets de la fenêtre
  Réglages, sans menu séparé. Les jeux
  Steam apparaissent dans la bibliothèque, sans onglet dédié. Les jeux des
  boutiques — **Epic Games, GOG et itch.io** — y entrent de la même façon, avec
  la jaquette que publie la boutique et la même fiche ; la barre de sources
  porte la marque de chaque boutique à côté de Steam et du disque, une pastille
  par boutique, et chacune se coche comme les autres — un jeu de boutique
  n'appartenant à aucune des sources Steam. GOG livre sa jaquette verticale
  (`boxArtImage`), que sa fiche produit ne publie pas. Absent du disque, un jeu
  de boutique ouvre une fiche réduite :
  jaquette, poids et bouton **Installer** ; une fois installé, c'est la fiche
  locale qui le reprend.
  Repliée comme dépliée, la fiche d'un jeu de boutique est peinte par **les
  mêmes encarts que la fiche Steam**, pas par des cousins : la ligne compacte
  garde l'identité, l'encart **Résumé** (présentation et trois captures, lapin
  pendant la lecture, captures cliquables) puis les succès ; dépliée, les
  **Actualités** et les encarts de compte passent à gauche, la **bannière** puis
  le « À propos » au centre, et l'encart média — affiche de bande-annonce en
  tête, captures dessous — à droite, exactement à la place et à la hauteur de
  ceux de Steam. La bande de tags ferme la fiche avec le développeur,
  l'éditeur, la sortie, la boutique, la version et le poids. L'encart du jeu
  dit aussi l'état des **sauvegardes cloud** en toutes lettres, « Cloud » vert
  ou barré gris, avec l'infobulle qui précise ; une icône barrée seule se lit
  mal. Les **annonces** viennent de la boutique quand elle en publie : GOG
  expose ses notes de mise à jour dans le journal de modifications du produit
  (`api.gog.com/products/{id}?expand=changelog`, découpé en une note par titre,
  date retirée du titre) ; Epic n'a pas d'annonces par jeu, son blog public
  (`store-content…/content/blog-posts`) est filtré sur le titre — un billet qui
  ne le porte pas en titre doit le citer deux fois — et itch n'en publie
  aucune. Faute d'annonces, l'encart s'efface, comme sur une fiche Steam.
  Chaque boutique fournit sa bannière
  large — visuel du catalogue chez Epic, `backgroundImage` chez GOG,
  couverture chez itch —, à défaut sa première capture. Le contenu se lit chez
  la boutique : la présentation rédigée et la galerie de captures, l'éditeur,
  le développeur et la date de sortie. Epic passe par la **vitrine du Store** (son GraphQL
  public), dont le catalogue de la bibliothèque ne garde qu'un titre et une
  jaquette — sa description ne fait souvent que répéter le nom du jeu. GOG se
  lit dans l'API publique de ses jeux, étiquettes comprises, et itch.io dans
  celle du compte, avec le texte long plutôt que le sous-titre. Le texte suit
  la langue choisie dans les réglages. Une bibliothèque ne montre qu'**une
  ligne par jeu** : un même titre publié sous plusieurs offres (marques
  déposées, ponctuation) n'apparaît qu'une fois, la copie installée l'emportant.
  Sur la tuile, la pastille de provenance prend la **marque de la boutique** au
  lieu du disque, et l'infobulle dit d'où vient le jeu. Les jaquettes de
  boutique sont demandées en 600 px au CDN : en pleine résolution (jusqu'à
  2,5 Mo la vignette), une rangée de tuiles saturait la connexion et les
  téléchargements expiraient — d'où les pochettes manquantes. Elles se
  **téléchargent d'avance** à la lecture de la bibliothèque, trois à la fois,
  pour qu'aucune tuile n'attende en lettre au premier défilement.
  Le marketplace Epic dépose ses **assets, greffons, moteurs et projets** dans
  la même bibliothèque que les jeux ; leurs catégories de catalogue le disent
  (« assets », « plugins », « engines », « projects »…). Ils sont **cachés**
  par défaut et se remontrent par l'interrupteur « Outils et assets » de
  Réglages › Comptes ; un titre de la catégorie « games » reste toujours un
  jeu. Les enregistrements déposés sous les sables du marketplace (« UE
  Marketplace », Fab) sont reconnus comme des outils même quand le catalogue
  ne dit rien de leurs catégories. **itch.io** fait de même avec sa
  classification : seules les entrées `game` (et `game_mod`) restent dans la
  grille, les assets, bandes-son et outils suivent le même interrupteur.
  Les **marques des boutiques** sont dessinées depuis des logos de 512 px
  rendus à la taille exacte de l'écran (densité comprise) : la carte graphique
  n'écrase plus une grande image en 30 px, et le lettrage d'« EPIC GAMES » ou
  de « gog.com » reste net — les marques des listes passent à 48 px, celles des
  fiches à 38-42, et les tuiles du Profil à 30 px dans un carré de 36.
  La bibliothèque
  n'affiche que les pochettes ; un clic fait monter du bas de la fenêtre une
  feuille compacte d'une seule ligne : fermeture et chevron à l'extrême
  gauche, puis un **encart d'identité** — jaquette, nom et provenance en
  icônes (logo Steam, puis une personne pour le compte personnel, deux pour
  la famille, un disque pour un jeu local ; l'appid et l'état vivent dans
  l'infobulle) — dans une carte au fond distinct. La jaquette monte à la
  hauteur de la carte et **une ligne d'actions selon l'état du jeu** se range
  à sa droite, sous le nom ; quand la colonne de texte est trop étroite pour
  la porter, elle reprend le pied de l'encart. Sa taille ne dépend pas de
  l'état du jeu : elle reste en place à l'instant où un téléchargement
  démarre, et les trois états occupent le **même bloc** — un bouton étiré à
  la largeur du bloc, plus étroit seulement sur une fiche serrée — au lieu de
  se rétrécir chacun à son libellé. Installé, Jouer étiré et deux **boutons
  carrés** de la même hauteur, qui portent leur propre cadre : engrenage qui
  ouvre les réglages dans le panneau fixe, comme les Réglages et le choix des
  contenus — des **encarts groupés** (lancement, affichage, langue,
  environnement) qui s'enregistrent à chaque changement, avec la **langue du
  jeu** quand on veut la forcer pour lui seul ; chaque valeur héritée est
  **nommée** plutôt que laissée à un « par défaut » muet : le Proton des
  réglages généraux, la limite de FPS, GameScope, MangoHud, la résolution et
  le profil manettes s'affichent sous le contrôle, en une ligne « Hérité des
  réglages généraux : … » — et l'entrée du choix Proton porte le nom de ce
  Proton-là ; un encart **Sauvegardes cloud** y liste les fichiers du compte
  (taille, date, « sur le disque » ou « à récupérer ») avec le dossier du
  préfixe où le jeu les lit, et un bouton **Récupérer du cloud** qui les
  télécharge par le protocole et les écrit là — sans client Steam, un jeu
  lancé par Lapinux n'aurait jamais reçu ses sauvegardes — poubelle qui demande confirmation — plus une **flèche circulaire** quand Steam publie un
  build plus récent que celui du disque ; absent du disque, un bouton qui
  porte le **poids à télécharger**, accolé à l'**emblème de la version
  retenue** — manchot, fenêtres ou pomme, tracés au trait — : il ouvre les
  versions que Steam publie (Linux d'abord, celui que la machine lance sans
  Proton, puis Windows, puis macOS), chacune avec son propre poids, et  le bouton reprend celui de la version retenue ; **les deux moitiés s'allument
  ensemble** au survol, l'emblème de la version passant en clair, pour que
  l'ensemble se lise comme un seul bouton jusqu'à son bord droit. Viennent ensuite, s'il en possède, un
  bouton **DLC** qui s'empile sous lui quand la colonne est étroite ; en cours de
  téléchargement, l'**icône de bibliothèque** du jeu (et non sa jaquette,
  illisible recadrée dans 22 px), la barre, le pourcentage et la croix qui
  annule la tâche. Pour un jeu **déjà installé**, le même choix de contenus
  vit dans l'entretien de la fiche, avec un bouton qui installe ceux qui sont
  cochés. Les succès Steam (compte, barre de progression et icônes,
  lus par la session du compte, noms des succès cachés compris) gardent une colonne
  mesurée, l'espace libre s'ouvrant entre l'encart et eux :
  le mot « Succès », la bande d'icônes à sa droite, le compte au bord, et la
  barre de progression dessous. Un jeu terminé à 100 % porte le **trophée du
  platine** juste avant son compte (et l'annonce dans la liste complète). Une
  tuile « + N » annonce les succès que la
  bande ne montre pas et ouvre la **liste complète** (débloqués du plus récent
  au plus ancien, puis tout ce qui reste). Cette liste, comme le choix des
  contenus additionnels, s'ouvre dans le **panneau fixe** de l'application —
  même châssis que les Réglages : voile sombre, panneau centré qui ne se
  déplace pas, titre et sous-titre **centrés** sur le bandeau (la croix garde
  le bord droit, la même largeur est réservée à gauche), contenu qui défile. Aucun rail d'informations
  à droite : sous le nom, une ligne de faits porte le **temps de jeu** du
  compte et le **poids**, chacun devant son pictogramme tracé au trait
  (**cadran**, puis **disque**) — durée seule, « Jamais joué » sans aucune
  minute, rien quand personne ne l'a dit. Les heures viennent du compte, donc
  elles s'affichent aussi pour un jeu **absent du disque** dès que sa fiche est
  lue ; le poids est celui du manifeste local quand le jeu est installé, et
  celui que Steam publie pour la version retenue — la place qu'il prendra —
  quand il ne l'est pas. Les deux tiennent une ligne à part sous les icônes de
  provenance. Survoler les heures détaille **ce que le compte publie** : total
  Steam, deux dernières semaines, Linux, Windows, macOS et Steam Deck, les
  lignes à zéro restant tues ; à défaut de lecture du compte, la ligne dit
  « fichiers locaux » plutôt que de faire passer un fichier pour un total.
  Un **nuage** s'ajoute aux marques de provenance quand le jeu synchronise
  ses sauvegardes cloud (catégorie Steam lue par le protocole). Le **Détails
  Steam** dit ce que le compte y garde vraiment : `Cloud.GetAppFileChangelist`
  rend chaque fichier avec sa taille et sa date, `ClientGetAppQuotaUsage` le
  quota consommé — aucun octet ne passe par la clé Web API. Le **lancement
  prend un relevé** avant que le jeu n'écrive, et la sortie de session compare
  les deux listes : la fiche annonce alors ce que la partie a **ajouté,
  modifié ou retiré** dans le cloud (« Cloud après la partie »), au lieu de
  laisser croire que rien n'a été sauvegardé. Les réglages du jeu reprennent
  ces fichiers un par un et savent les **restaurer** : le nom complet déclaré
  par Steam (`%WinAppDataLocal%MotorSlice/Saved/SaveGames/GameSave.sav`) est
  demandé à `ClientFileDownload`, l'archive du CDN est dépliée quand elle
  arrive compressée, et la sauvegarde est écrite dans le préfixe Proton du
  jeu, à la place que le jeton Windows désigne (`%GameInstall%`,
  `%WinAppDataLocal%`, `%WinMyDocuments%`…), avec la date du cloud. **Lancer
  le jeu reprend d'abord** ce que le cloud a de plus récent que le disque
  (25 s au plus, un échec ne bloque pas le démarrage) : une progression locale
  plus fraîche n'est jamais écrasée, le bouton reste là pour forcer le reste.
  La **bande de tags de la fiche** porte l'état : « Cloud 3 fichiers · 24/10 ·
  à jour », « à récupérer » quand un fichier manque ou que le compte en a un
  plus récent, « aucune sauvegarde » pour un jeu qui annonce le cloud sans
  rien dedans, « indisponible » quand la session n'a pas répondu.
  **La sortie de session envoie** ce que la partie a écrit : `BeginAppUploadBatch`
  ouvre un lot, `ClientBeginFileUpload` rend les blocs (adresse, méthode,
  en-têtes), le fichier part en HTTP, `ClientCommitFileUpload` le valide et
  `CompleteAppUploadBatchBlocking` publie le lot — un lot non clos ne publie
  rien. Seuls les fichiers **plus récents que le cloud et de contenu différent**
  partent (l'empreinte SHA-1 de la liste fait foi ; Steam répond
  `DuplicateRequest` à un envoi identique), et si `change_number` a bougé
  depuis le relevé de lancement, une autre machine a sauvegardé : **rien n'est
  envoyé** et le journal le dit. Les réglages du jeu offrent les deux sens
  (« Récupérer », « Envoyer »), chacun avec sa confirmation.
  **Trois emplacements de sauvegarde** doublent ce dispositif, cloud ou pas :
  à la fermeture du jeu, le dossier de sauvegarde part dans
  `saves/<nom du jeu> (<appid>)/<emplacement> (<date>)/` — par exemple
  `1 (31-10-2026 02-13)/` — avec un manifeste du même nom en `.json`
  (emplacement, date, nombre de fichiers, poids, dossier d'origine,
  empreinte). Le nom rend le dossier lisible
  sans perdre l'unicité : l'identifiant est celui de Lapinux (le vrai AppID
  Steam, un identifiant stable dérivé de celui de la boutique pour Epic, GOG et
  itch), et un ancien dossier `<appid>` suit son jeu au premier accès, titre
  corrigé compris ; les copies d'avant le nom lisible, qui n'étaient qu'une
  suite de secondes Unix, prennent leur « emplacement + date » à la première
  lecture, sans perdre de fichier. Le quatrième état remplace la copie la
  plus ancienne : trois emplacements par jeu, pas dix versions, ce qui borne
  l'espace disque et fait disparaître les anciennes séries à la première
  lecture. L'empreinte porte sur le **contenu** (chemins et octets), pas sur les
  dates : une sauvegarde réécrite dans la même seconde compte comme un nouvel
  état, une copie identique n'est pas empilée. Un jeu sans cloud garde ainsi de
  quoi revenir en arrière ; un jeu qui en a reçoit une copie locale seulement
  si le réglage le demande — « Ne pas copier les jeux qui ont une sauvegarde
  cloud », coché par défaut dans **Réglages › Sauvegardes**, saute la copie
  automatique quand la boutique garde déjà la partie en ligne. L'onglet
  **Sauvegardes** présente les trois emplacements : un clic sur un carré
  **vide y prend immédiatement une copie**, un clic sur un carré occupé le
  choisit, « Charger » remplace — l'état remplacé est d'abord copié, donc le
  geste reste réversible — et laisse le prochain lancement lire la partie
  choisie : un jeu ne relit ses sauvegardes qu'au démarrage. Le réglage
  **Réglages › Sauvegardes** « Relancer le jeu après avoir chargé un
  emplacement », décoché par défaut, peut être coché pour que le bouton dise
  « Charger … et jouer » et démarre le jeu aussitôt. La copie revient dans le
  préfixe que le jeu utilise aujourd'hui, même s'il a été réinstallé ou
  déplacé depuis. Un encart **Sauvegarde actuelle** dit quelle copie correspond
  à l'état du jeu et d'où elle vient — copie automatique à la fermeture, copie
  manuelle, ou emplacement chargé — et ouvre son dossier. La fiche en prend une
  à la demande, en **restaure** une, en **supprime** une — la poubelle devient
  une coche de confirmation et les sauvegardes du jeu ne sont jamais touchées —,
  ou **exporte** toutes les copies d'un jeu vers un dossier choisi, sans
  toucher aux copies locales. Le dossier racine se règle dans
  **Réglages › Sauvegardes**, qui affiche aussi le poids de l'ensemble.
  **La fiche dépliée a jusqu'à quatre onglets**, sous sa ligne compacte :
  « Général » (bannière, description, actualités, captures et réglages du jeu),
  « Configurations » (seulement quand le catalogue connaît le jeu — le
  catalogue communautaire de correctifs, traductions et
  mods déclaratifs, publié sur `github.com/PissenlitVert/lapinux-configurations` :
  chaque carte nomme son auteur et sa catégorie, s'installe
  en un clic, se retire en laissant le jeu dans l'état d'avant, et propose
  **Mettre à jour** quand le catalogue publie une version plus récente ; les
  fichiers remplacés sont sauvegardés et les réglages touchés sont restaurés),
  « Sauvegardes » (le dossier choisi, les copies locales versionnées et le
  cloud du compte) et « Détails » (tout ce que la boutique publie —
  développeur, éditeur, genres, dates, langues, poids, succès, cloud,
  capacités du compte — puis les configurations **minimale et recommandée**,
  les mentions, le DRM et l'installation sur ce poste). Changer de jeu ramène
  à « Général ». Un jeu Steam possédé mais absent du disque porte les mêmes
  onglets : la fiche catalogue n'omet que les réglages d'installation.
  Une **bande de tags** ferme la fiche sous tous les encarts : l'âge
  recommandé, le développeur, l'éditeur, le prix, l'année, mais aussi les
  **joueurs connectés**, le **nombre d'avis** et la **part d'avis positifs**
  y prennent chacun leur rectangle (libellé en petit, valeur à sa suite,
  valeur entière en infobulle). Les milliers s'espacent d'une espace fine, les
  tags qui ne tiennent pas dans la largeur sont annoncés par un « + N », et la
  bande est **centrée** dans la fiche : sa largeur se mesure avant d'être
  peinte.
  Repliée, la fiche loge un encart **Résumé** dans l'espace libre entre
  l'encart d'identité et les succès : le texte court de la boutique, empilé
  sous son titre, puis **trois captures en vignettes** dans la place qui
  reste — elles s'ouvrent en grand d'un clic. La carte est arrêtée à la
  hauteur exacte de celle du jeu : les trois cartes de la ligne finissent
  ensemble, sans toucher la bande de tags.  Elles existent **  toutes les trois
  dès l'ouverture** : tant que la fiche se lit, la carte du résumé porte le
  **lapin animé au centre**, son libellé dessous, et les succès annoncent leur
  propre lecture. Le lapin est peint à un multiple entier de ses 16 px, sans
  lissage : il reste net.
  La colonne des **succès** descend elle aussi jusqu'à la même ligne : sous
  la bande d'icônes et sa barre de progression, la place libre liste les
  **derniers succès obtenus** (nom, date de déblocage, du plus récent au plus
  ancien) ou, tant qu'aucun n'est débloqué, les **plus rares encore à
  décrocher** avec la part de joueurs qui les possèdent — les succès cachés
  gardent leur nom secret. Chaque ligne ouvre la liste complète, comme la
  tuile « + N ». Un jeu qui n'en publie aucun montre un **trophée barré** au
  centre de l'encart, sans ligne de texte.
  L'encart d'identité garde exactement la même taille replié et déplié :
  ouvrir le corps ne fait pas changer la carte du jeu.
  Un chevron déplie le corps complet, en **trois colonnes** — les
  **actualités** à gauche, le **jeu** au centre, les **images** à droite (la
  **bande-annonce** en tête, puis les **captures**). Les deux colonnes
  latérales ont exactement la même largeur : le centre est donc **centré**
  entre elles. La **bannière** du jeu ouvre ce centre, et la description suit
  **à même la fiche**, sous elle, sans encart : un titre **À propos**, le
  résumé court, le texte long, les **visuels** que la boutique place dans ce
  texte (jusqu'à quatre, à la largeur de la colonne), puis les configurations
  et mentions repliées. Pour un jeu de boutique, la présentation entière
  remplit le texte long : l'encart existe donc aussi quand elle tient en deux
  phrases (Epic, GOG, itch), la carte **Résumé** se chargeant de la tronquer. Ces visuels sont les images du texte (PNG, JPEG,
  WebP). La boutique les publie de plus en plus en AVIF, et ses bannières
  animées sont des vidéos (`.webm`, `.mp4`) dont l'affiche est aussi un AVIF :
  sans décodeur AVIF embarqué, la fiche écarte ces adresses plutôt que
  d'afficher une vignette vide.
  Faute de lecteur vidéo dans l'interface, l'affiche de la bande-annonce ouvre
  la page Steam où elle se joue. Chaque image — bannière, capture, visuel du
  texte — s'ouvre **dans l'application**, dans le même panneau fixe que les
  réglages : le navigateur ne s'ouvre plus pour une capture. Les deux flèches
  posées sur l'image, ou les touches ← et →, passent d'une capture à l'autre
  (la série boucle) ; le sous-titre dit laquelle on regarde. La bannière garde
  une hauteur plafonnée à 170 px, pour que le « À propos » reste près d'elle.
  L'entretien — contenus additionnels et mise à jour — reste sous les annonces
  pour un jeu installé ; les **réglages du jeu** et la **désinstallation** ne
  vivent que dans l'encart (engrenage et poubelle), plus en double ici. Sous
  844 px, les encarts s'empilent dans cet ordre; les trois colonnes tiennent
  toujours dans la fiche, écarts implicites de la rangée compris.
  La ligne du haut — carte du jeu, résumé, succès — ne bouge pas d'un état à
  l'autre : la déplier ne fait qu'ouvrir le corps **sous** elle.
  Le corps est relu à chaque ouverture en tâche de fond (`Req::GameInfo`,
  ~4 s) : par le protocole et la seule session, temps de jeu total, des deux
  dernières semaines et par système (Windows, Linux, macOS, Steam Deck),
  dernière partie, niveau du compte, badge du jeu, succès débloqués et
  **succès les plus rares** avec la part de joueurs qui les possèdent,
  statistiques publiées avec leurs libellés, et le nombre de joueurs connectés
  (seule valeur demandée en HTTP pour le compte, adresse publique sans clé) ;
  côté boutique, prix avec remise, nombre d'avis, genres, résumé, captures,
  site et assistance — la description, les configurations minimale et
  recommandée et les mentions vivent dans l'encart du centre, défilantes.
  La boutique et les annonces sont demandées dans la **langue choisie dans les
  réglages** (`&l=` d'`appdetails` et d'`ISteamNews`), le français par défaut ;
  Steam renvoie le texte d'origine quand un jeu n'a pas de page traduite.
  Les sources qui n'ont pas répondu sont listées à la fin et le bouton
  **Relire** refait la lecture.
  Les **images AVIF**, les **petits GIF** et les **boucles WebM/MP4** de la
  description sont conservés avec les autres visuels, convertis par ffmpeg
  quand il le faut et rendus **dans l'ordre du texte** : image, paragraphe,
  image. Les visuels animés sont joués à leur taille naturelle.
  L'encart **Actualités** montre les trois dernières annonces de l'éditeur —
  notes de mise à jour et actus — avec leur **visuel** (image du BBCode
  `{STEAM_CLAN_IMAGE}`, `og:image` en secours), leur titre, le début du texte
  et leur date. La carte entière, comme le bouton « Voir les N annonces »,
  ouvre un lecteur plein écran : une colonne étroite de sections sur un voile
  sombre translucide qui laisse voir le launcher derrière, avec sa **barre de
  défilement au bord droit de la colonne** et un **sommaire fixe daté juste à
  droite de cette barre**. Chaque ligne du
  sommaire porte la date et le titre d'une annonce et y fait défiler
  directement ; le survol d'un article s'efface dès qu'on clique ailleurs.
  Le lecteur n'a pas de barre de titre ; Échap ou un clic sur le voile, à côté
  de la colonne et du sommaire, le ferme. Le lecteur garde **toutes les publications** du flux dédié
  de l'éditeur — jusqu'à une centaine, 98 pour 007 First Light — en texte
  complet, paragraphes préservés et visuel **pleine largeur en tête**, sans
  cadre autour de l'image. Elles viennent du flux public
  `ISteamNews/GetNewsForApp`, sans clé, où les dépêches de presse se mêlent aux
  publications de l'éditeur : seules ces dernières sont retenues, et le
  balisage de Steam (`[img]`, `[url]`, chemins de visuels) disparaît de
  l'aperçu, phrases recollées comprises. Les cartes se rangent côte à côte
  quand la colonne est large et s'empilent sinon. Un jeu qui ne publie rien
  garde sa fiche sans encart. L'encart **Mises à jour** n'existe plus : le
  build public, les branches et le nombre de dépôts restent lisibles dans le
  bloc **Détails Steam** du centre. La
  bibliothèque n'affiche ni titre ni
  bouton de synchronisation : ses sources (Disque, Steam, Famille Steam,
  Tout Steam) sont centrées en haut et peuvent être désélectionnées
  indépendamment. La **loupe et le dé** forment une seule pastille segmentée,
  comme les sources Steam ; la loupe ouvre la barre de recherche (le champ
  affiche alors sa propre loupe, seul le dé reste), et les filtres
  segmentés (Tous / Installés / À installer) affichent leurs compteurs.
  Les synchronisations vivent dans Réglages → Bibliothèque, avec la taille
  des vignettes et le masquage des outils Steam. Une icône d'écran bascule
  entre fenêtre et plein écran, animation de démarrage Lapinux.
  Les outils et runtimes Steam (Proton, Steam Linux Runtime, Steamworks Common
  Redistributables) peuvent être retirés de la grille sans toucher à leur
  installation.
- **Jaquettes et illustrations** : les pochettes Steam sont complétées par
  SteamGridDB quand une clé API personnelle est enregistrée
  (Réglages → Comptes → SteamGridDB) ; l'API est interrogée avec
  `Authorization: Bearer`, toutes les dimensions proposées sont acceptées et
  recadrées au centre plutôt que déformées, et les images sont mises en cache.
  Les sources suivent l'ordre de priorité Lapinux puis SteamGridDB, pour les
  jaquettes comme pour les bannières, capsules, logos et icônes. La première
  source qui fournit une image gagne ; la suivante sert de repli si
  le jeu est absent, la clé refusée ou l'image introuvable. Le cache local du
  client Steam compte comme une image Lapinux : une source placée avant lui
  est donc toujours essayée en premier. Côté Lapinux, le CDN Valve est
  interrogé dans l'ordre `library_600x900_2x` (600×900), `library_600x900`
  (300×450, la version simple n'est que la moitié), `library_capsule_2x` et
  `library_capsule`, sur `cdn.` puis `shared.` : une partie des jeux n'a pas le
  premier fichier, et c'est ce qui faisait retomber leur jaquette sur une
  source communautaire — parfois l'affiche d'un autre jeu quand SteamGridDB
  ne connaissait pas l'appid. Chaque image téléchargée est **décodée avant
d'être acceptée** : un visuel paysage (hero, bannière) n'est jamais rogné en
  pochette. Quand aucun visuel n'existe, une vignette affiche le nom du jeu.
  Un clic droit sur une pochette ouvre une galerie des jaquettes SteamGridDB
  du jeu : l'image choisie est enregistrée localement et prime sur la
  sélection automatique. « Jaquette automatique » efface l'image choisie
  **et** les jaquettes téléchargées (Steam comme SteamGridDB) : la fiche
  repart de la source préférée, de quoi remplacer une pochette qui ne
  correspondait pas au jeu. La clé n'est jamais affichée en clair.
- **Système** : onglet des Réglages avec le modèle CPU, les threads, la charge,
  la mémoire utilisée, les cartes graphiques, le pilote et le renderer OpenGL
  réellement utilisé. Lecture directe de `/proc` et `/sys/class/drm`, sans
  dépendance supplémentaire.
- **Cartes de téléchargement** : chaque tâche est un bandeau du jeu — fond
  flou tiré du hero (SteamGridDB, CDN Steam ou cache du client), icône et titre
  en haut à gauche, pourcentage et hôte CDN en haut à droite. La barre de
  progression reprend un dégradé des deux teintes dominantes du fond flou et
  les débits **réseau** (chunks réellement demandés au CDN, sans les morceaux
  réutilisés depuis le disque) et **écriture** sont mesurés séparément, avec le
  volume traité et le temps restant. Une carte peut être mise en pause puis
  reprise au même point. Une tâche arrêtée par un redémarrage garde ses
  fichiers et attend dans **En pause** — sa carte propose « Reprendre » — et
  jamais dans « Terminées », où elle passerait pour un transfert abouti. Le
  fond et la palette sont calculés une fois par jeu puis gardés en mémoire.
- **Dossier de téléchargements** : une barre compacte dans l'en-tête de
  Téléchargements montre le pourcentage utilisé et, hachurée en transparence,
  la place que prendront les téléchargements en cours et en attente (le détail
  octets et le chemin sont dans l'infobulle). Une carte « Débit » autonome, à
  côté du dossier, propose un test de 10 Mo et affiche le débit mesuré. Une
  carte « Limites » plafonne le débit — un plafond global, puis un par launcher
  — avec une piste dessinée à la main, lisible sur le fond de la carte et
  alignée sur ses icônes ; le plafond s'applique après un court temps d'arrêt
  du curseur, sans interrompre les téléchargements. Le dossier se change
  directement depuis cette page et est créé au premier téléchargement.
- **Réglages et logs** : le bouton « Réglages » (ou `Ctrl+,`) ouvre une fenêtre
  centrée à huit onglets (général, bibliothèque, apparence, comptes,
  journaux, puis outils Protons, Système et Diagnostic), sans quitter la
  bibliothèque. _Général_ garde un brouillon jusqu'à « Enregistrer » (le
  rafraîchissement périodique n'écrase plus une saisie) ; son encart
  _Lancement_ choisit le **Proton par défaut dans une liste** — « Automatique
  (recommandé) » puis chaque Proton installé, avec la phrase qui dit ce que le
  choix engage — et la **limite de FPS parmi des paliers**, de l'illimité
  (aucun argument posé à GameScope, donc la cadence du compositeur) à 360 FPS,
  plus une valeur libre ; un encart
  _Langue_ fixe la **langue des jeux** — un nom Steam, ou « Comme Steam / le
  système » : elle choisit les dépôts de doublage téléchargés (donc le poids
  annoncé) et la locale passée au jeu au lancement ; un encart _Écran
  d'attente_ — et l'interrupteur du même nom à côté de GameScope et MangoHud —
  choisit ce qui couvre **tout l'écran** pendant qu'un jeu démarre : la
  **capsule du jeu** (sans logo qui la traverse), le **lapin animé** historique,
  ou **aucun** voile. La fenêtre passe en plein écran le temps de la
  préparation puis retrouve son état, et le voile apparaît comme il s'efface,
  en fondu, sans clignotement quand le jeu prend la main ;
  _Apparence_ règle
  l'ordre des sections de la barre du haut, appliqué aussitôt ; _Comptes_ contient
  tout le parcours Lapinux (mot de passe, jeton collé, Steam Guard,
  confirmation mobile, QR code, jeton enregistré à oublier) et la clé
  SteamGridDB ; la fiche du compte tient en un en-tête (avatar, nom, pastille
  d'état) et trois tuiles — compte, session, âge du jeton — avec
  « Renouveler par QR » et « Oublier le jeton » à portée ; _Système_ affiche la version
  du daemon qui tourne, le build du binaire présent sur le disque et permet de
  le redémarrer — un daemon plus ancien que le binaire est signalé et remplacé
  automatiquement au lancement, sans quoi il effacerait les réglages récents ;
  _Journaux_ liste quatre journaux stables du dossier de données — `daemon.log`,
  `ui.log`, `games.log` et `overlay.log`, tournés au-delà de 8 Mo — affiche les
  400 dernières lignes du fichier choisi et les relit tant que l'onglet est
  ouvert. Le daemon journalise ses actions (scan,
  lancement, installation, connexion) sans
  jamais écrire de secret. Aucun écran « outils Steam » : le moteur de
  téléchargement est interne et n'apparaît pas dans l'interface.
- **Page Protons** : la liste des builds se remplit **à l'ouverture de la
  page**, les dépôts étant interrogés en tâche de fond pour que l'interface
  reste vivante ; « Vérifier les mises à jour » les relit à la demande — GitHub
  pour GE-Proton et Proton-CachyOS, le Forgejo de Dawn Winery pour DW-Proton,
  qui n'a plus de dépôt GitHub — et la ligne d'état dit depuis quand la liste
  date. Quand des builds « latest » manquent, une **proposition les
  nomme et « Tout installer » les lance d'un coup** ; sinon une pastille verte
  dit que tout est à jour. La page se lit ensuite comme une liste, à la manière
  de ProtonPlus : une **bande par source** — son nom, ce qu'elle apporte, son
  état (« recommandé », « à jour », « mise à jour » ou « installés seulement »)
  et un **chevron** qui déplie tous ses builds, le plus récent en tête et les
  préversions derrière les stables ; chaque build montre sa date, son poids et
  sa propre action — « Installer », « installé », ou la barre et le pourcentage
  du téléchargement en cours. Les étiquettes sont des **fonds pleins arrondis**
  sans contour, et les boutons partagent trois tons (vert d'action, blanc de
  vérification, gris de relecture locale). Suivent les **versions installées**,
  en liste : recommandée en tête, source, « incomplet » quand le build ne peut
  pas lancer un jeu, chemin en infobulle.
- **Quick menu** : surface `wlr-layer-shell` (overlay) inspirée du menu rapide
  Steam Deck (panneau latéral, sections, interrupteurs), ouverte avec le bouton
  **Guide** de la manette ou **Shift+Tab** au clavier par défaut, navigation
  clavier, souris et manette, actions session (profil, capture, arrêt). Deux
  onglets, **Jeu** et **Performances** : le second ouvre un panneau plus haut
  avec la limite de FPS, l'interrupteur **MangoHud**, trois profils —
  **Simple**, **Basique**, **Détaillée** — qui affichent tous le nom du GPU et
  du CPU (raccourcis au modèle, pour ne pas élargir le HUD) et les températures
  (le détaillé ajoute VRAM, fréquences et puissance), une ligne **Disposition**
  (**En ligne** ou **En bloc**), un interrupteur **Graphique FPS** pour la
  courbe de frametime — coupée par défaut — puis une grille des **neuf
  ancrages** du HUD (haut/milieu/bas × gauche/centre/droite). Le mode **En
  ligne** s'arrête après la dernière statistique au lieu d'étirer le HUD sur
  toute la largeur de l'écran. Lapinux embarque sa propre couche MangoHud
  0.8.4 corrigée (`packaging/mangohud`) : elle ancre la ligne sur la taille
  réelle du HUD, donc les neuf positions tombent juste, y compris à droite et
  au centre. La couche est installée dans
  `~/.local/share/lapinux/mangohud`, activée par `MANGOHUD=1` ; au lancement,
  le daemon l'ajoute au chemin du loader et désactive celle du système. Sans
  elle, MangoHud d'origine reste seul — avec son défaut de placement. MangoHud relit sa configuration : profils,
  ancrage, disposition, graphique et affichage s'appliquent à chaud quand
  MangoApp tourne. La limite de FPS change aussi à chaud sous GameScope, via
  `gamescopectl` ; hors de lui, elle attend le prochain lancement.
  GameScope n'applique pas une limite libre : il la **divise** dans la cadence
  de son compositeur (`--framerate-limit` : 60 / 25 → 30, 60 / 59 → 60), qui
  ne se règle qu'au lancement. Le compositeur prend donc le plus petit multiple
  de la limite strictement supérieur à l'écran (au moins 2×) : 330 images pour
  un écran et une limite à 165, 180 pour 60 sur un écran à 144. Chaque palier
  tombe sur un diviseur entier, la valeur affichée est celle qui tourne, et
  aucune marge fixe à 720 ne vient brider un jeu. Le HUD MangoHud, lui, ne pose
  jamais de limite : il ne fait qu'afficher les mesures. Pendant
  l'overlay, le service d'entrée relâche le grab des manettes pour que le
  panneau se navigue directement à la croix directionnelle et aux boutons A/B.

## Limites assumées

- Les jeux protégés par un **DRM Steam** sont détectés et signalés ; aucun
  contournement n'est fourni. Pour les jeux Steamworks, Lapinux démarre le
  client Steam officiel en mode headless (`-silent`, sans fenêtre principale)
  et attend qu'il soit prêt avant de lancer le jeu. Le jeu doit appartenir au
  compte Steam connecté, sinon il s'arrête sur son erreur DRM. Un build Linux
  natif qui embarque `libsteam_api.so` reçoit le même traitement. Aucun client
  Steam allégé tiers n'est téléchargé : le client Bionic de GameNative est
  spécifique à Android ARM64 et n'existe pas pour Linux x86.
- Steam Input, cloud, atelier, chat et Remote Play dépendent du client Steam :
  hors périmètre. Les succès et les commentaires sont lus sur le profil public,
  la couche manettes maison couvre boutons, axes, gyro et macros.
- Lapinux **lit** les sauvegardes cloud et compare l'avant/après d'une partie ;
  c'est le jeu, par son client Steamworks, qui les téléverse et les récupère.
- Les succès se lisent par la **session Steam**, sans clé Web API :
  `Player.GetGameAchievements` publie le schéma du jeu (nom, description,
  icône, succès caché, identifiant interne) et la rareté mondiale de chaque
  succès (`player_percent_unlocked`), `Player.GetUserAchievements` l'état du
  compte, rapproché par identifiant interne. Une lecture qui dépasse
  quarante-cinq secondes est abandonnée avec un message et un lien
  « Réessayer », et seules les icônes réellement affichées sont téléchargées,
  quatre en parallèle. Le **Détails Steam** lit de même le temps de jeu par
  `Player.GetOwnedGames` (total, deux semaines, par système, dernière partie) —
  avec, en champ 9, l'identifiant du groupe familial (`FamilyGroups.
  GetFamilyGroupForUser`) : sans lui Steam omet les jeux prêtés par la famille,
  qui affichaient donc « aucun temps de jeu » alors que le compte y avait joué —,
  le niveau par `Player.GetGameBadgeLevels` et les statistiques publiées par
  `Player.GetUserStats`, dont le schéma binaire donne les libellés — l'embed du
  jeu affiche aussi les heures jouées par le compte. Aucune clé Web API n'est
  demandée : la session enregistrée suffit, et la seule lecture HTTP qui reste
  est `GetNumberOfCurrentPlayers`, adresse publique qui ne veut ni clé ni
  jeton. La page des avis de la boutique (`store.steampowered.com/
  appreviews/…`) redirige désormais vers la vitrine pour un client sans cookie :
  seule la page publique `appdetails` est lue, et son compte d'avis
  (`recommendations.total`) remplace le résumé communautaire. Enfin, Steam
  publie pour les jeux sans version Linux un `linux_requirements` réduit à son
  titre, suivi d'une liste vide : la configuration minimale retenue est alors
  celle de Windows, c'est-à-dire celle que Proton met en œuvre.
- Les **contenus additionnels** sont listés en entier : le panneau de choix
  affiche tout ce que Steam publie pour le jeu, possédé ou non — la fiche PICS
  les nomme tous et le catalogue du compte dit lesquels sont au compte, d'où
  un compte du type « 4 sur 9 possédés ». La liste est coupée en deux
  sections, **Possédés** d'abord puis **Non possédés**, chacune comptée ; ce
  que le compte possède est coché d'office, si bien que le bouton de
  téléchargement annonce directement le poids à poser. Un contenu non possédé
  garde sa place, sa bannière et son poids, avec une case grisée : il ne se
  télécharge pas. Leur **poids** se lit dans les dépôts du **jeu parent**,
  ceux qui portent le champ `dlcappid` : la fiche d'un contenu ne publie pas
  les siens, et c'est ce qui affichait « poids inconnu » partout. Un contenu
  possédé sans dépôt à lui se dit **inclus dans le jeu** : ses fichiers
  voyagent dans ceux du jeu de base, cas de la plupart des tenues et
  accessoires. Leur **bannière** (`header.jpg`, chemin `store_item_assets`
  quand PICS le donne, servi par l'hôte `akamai` qui répond directement là où
  `cloudflare` redirige) est téléchargée une fois puis gardée avec les autres
  illustrations ; à défaut, la fiche `appdetails` de la boutique est
  interrogée — une fois par contenu, sa limite d'appels est basse. Aucun
  visuel n'existe pour un contenu non listé (sortie à venir) : la vignette
  reste neutre. Le panneau
  dit aussi lesquels sont **déjà sur le disque** et lesquels manquent : Steam installe
  chaque contenu comme une application à part, avec son manifeste
  `appmanifest_<appid>.acf`, et Lapinux écrit le même pour ceux qu'il installe
  lui-même. Un contenu déjà là ne s'ajoute plus au poids annoncé :
  l'installation le vérifie sans le recharger. Les dépôts d'un
  contenu coché rejoignent ceux du jeu (dépôts `dlcappid` compris) et
  s'installent dans son dossier ; un contenu finalement non possédé refuse sa
  clé de dépôt et se saute sans interrompre le reste. Un contenu acheté après
  coup s'ajoute donc à un jeu déjà installé, depuis sa fiche : l'installation
  commence par vérifier les fichiers présents et ne télécharge que ce qui
  manque.
- Un compte Steam et la 2FA sont exigés pour la connexion ; seul le contenu
  possédé est accessible.
- L'**installation Epic** n'est pas branchée : elle passe par des manifestes
  binaires et des chunks CDN, que Legendary et Heroic implémentent mais que
  Lapinux ne lit pas encore. La bibliothèque Epic s'affiche, et le bouton
  d'installation dit pourquoi il n'est pas là plutôt que d'échouer après coup.
- **itch.io** n'a ni sauvegardes cloud ni succès : son API ne les expose pas,
  et la fiche du jeu ne montre donc pas d'encart vide pour autant.
- Les **sauvegardes Epic et GOG** passent par des points d'entrée que les
  launchers libres ont relevés, non documentés comme API publiques : ils
  peuvent changer sans préavis, et l'erreur s'affiche alors dans la carte au
  lieu d'être avalée. Le dossier local doit être désigné dans les réglages du
  jeu : Epic ne publie ni la taille de ses fichiers ni le chemin employé par le
  jeu. Les succès de ces deux boutiques sont lus, mais **débloquer** un succès
  reste l'affaire du jeu lui-même (overlay EOS pour Epic, client/overlay Galaxy
  pour GOG), comme pour Steam.
- La première synchronisation du catalogue étend chaque paquet de licence par
  PICS puis lit les noms : compter ~40 s pour un millier de jeux, en
  arrière-plan, une seule fois — ensuite seul le cache est relu.
- GameScope et MangoHud peuvent manquer : l'UI le signale et le lancement
  reste direct, sans couche intermédiaire.
- Le runtime sniper de Steam embarque Python 3.9, trop ancien pour le script
  `proton` de GE-Proton 11 ; Lapinux exécute donc `proton` avec le Python de
  l'hôte (validé sur Bazzite/Fedora).

## Lapinux — hôte natif (expérimental)

`lapinux-probe` charge la bibliothèque d'un client Steam installé sur la
machine dans son propre processus, puis rapporte jusqu'où ses interfaces
répondent sans client lancé. `lapinux-probe --session` va plus loin : il ouvre une
session locale dans un processus enfant nommé `lapinux-host`, isolé pour que
l'effondrement du moteur ne tue pas la commande. `lapinux cli steam-probe` reste
un raccourci vers la même surface.

`lapinux-probe account` interroge deux backends derrière la même interface : le
moteur de Valve (session locale, sans authentification) et notre client du
protocole CM (`crates/steam/src/cm`), qui s'authentifie réellement. Sans
drapeau, les deux sont essayés ; `--local` ou `--cm` en isole un, et
`--endpoint HOST:PORT` impose un serveur.

Ce qui est mesuré sur la machine de référence (Bazzite, x86_64) :

- la bibliothèque se charge sans Steam (45,5 Mio, dépendances glibc seules) ;
- les sept versions d'interface `SteamClient017` à `SteamClient023` répondent ;
- `CreateSteamPipe` renvoie **0 sur toutes les versions** : le pipe global
  attend une session cliente, pas un `dlopen` ;
- **la session locale de type 1 (« individu ») tient**, avec `ISteamUser023`,
  un SteamID (`76561197960265728`), `connecte=false` et `abonne=false` ;
- les types 10 puis 4 restent en repli si le moteur tombe ; le type 4
  (serveur anonyme) n'expose aucune interface utilisateur.

### Pourquoi le type 1 tombait, et ce qui l'a réparé

Le moteur cherche `libvideo.so` (visible au journal du chargeur, `LD_DEBUG`) et
échoue en `steamp2pvoice.cpp (153)`. Trois causes distinctes, mesurées une à
une :

1. les `libav*` hérités du lanceur (`/tmp/.mount_*`) n'ont pas le symbole
   `av_mallocz_tracked` : il faut imposer le runtime de Valve dans
   `LD_LIBRARY_PATH` (`ubuntu12_64`, `steamrt64`) ;
2. Fedora ne fournit pas `libbz2.so.1.0`, nom exigé par le `libvideo.so` de
   Valve : Lapinux crée un lien de compatibilité dans
   `~/.local/share/lapinux/compat` (vers `libbz2.so.1` du système) ;
3. le `libavfilter` de Valve appelle `inflate` **sans déclarer zlib** : zlib
   doit être préchargé (`LD_PRELOAD`).

Avec ces trois éléments, le journal montre `calling init: .../libvideo.so`,
l'assertion vidéo disparaît (0 sur 6 exécutions directes puis 3 sur 3 via la
commande) et la session de type 1 survit.

Comme l'hôte sort par `_exit` (voir plus bas), un code 0 ne prouve rien : le
parent analyse aussi le `stderr` de l'enfant et rejette toute trace
`steamp2pvoice`/`assert_`, ce qui permet le repli automatique vers un type de
compte plus léger.

### Le mur de l'authentification, nommé

`ISteamUser` donne bien le SteamID et l'état de connexion, mais **aucune
interface exposée ne porte `LogOn`/`LogOnWithPassword`** :

- `CreateInterface` ne rend que `SteamClient0xx` (`SteamUser021`, `ClientUser001`
  renvoient null, code 1) ;
- `GetISteamGenericInterface` renvoie null pour
  `CLIENTUSER_INTERFACE_VERSION001` à `020` et `STEAMUSER_INTERFACE_VERSION001`
  à `020`, avec au journal « Missing interface adapter » ;
- `STEAMAPPS_INTERFACE_VERSION001` à `009` sont en revanche disponibles
  (`BIsSubscribed` répond `false` hors connexion).

Le SteamID observé (`76561197960265728`, identifiant individuel de compte 0) est
donc un identifiant local, pas un compte authentifié. Ce chemin ne peut pas
remplacer le client CM pour la connexion : l'interface cliente interne n'est pas
exposée.

### L'authentification, par notre propre client CM

Plutôt que d'insister sur une ABI qui ne coopère pas, Lapinux parle désormais
lui-même aux serveurs CM (`crates/steam/src/cm`) :

- découverte des CM `netfilter` par la WebAPI Steam, essais successifs ;
- trames TCP `[longueur][VT01][message]`, handshake `ChannelEncrypt` :
  RSA-OAEP/SHA-1 avec la clé publique de l'univers Public, puis canal
  AES-256-CBC authentifié par HMAC-SHA1 (13 octets de HMAC, 3 octets aléatoires) ;
- `ClientHello` puis `ClientLogOn` protobuf, en mode anonyme ou nominatif : le
  service `Authentication` vérifie le mot de passe RSA et Steam Guard, puis
  `ClientLogOn` ne porte plus que le jeton de refresh.

Deux détails ont coûté le plus de temps et sont désormais couverts par des
tests : la clé de Valve utilise l'exposant **17**, pas 65537 (un mauvais
exposant produit un cryptogramme que le CM rejette sans explication), et les CM
actuels imposent le mode HMAC du canal dès qu'ils envoient un nonce.

Résultat mesuré le 23 octobre 2026, sans aucun identifiant :

```
comptes Steam :
  steamclient  steamid=76561197960265728 connecte=non
  cm           steamid=117317163417430244 connecte=oui
               anonyme=oui endpoint=155.133.248.39:27017 cell=14 battement=9s
```

Le handshake et le logon tiennent donc de bout en bout, et le SteamID renvoyé
est un vrai compte anonyme. En revanche, un compte anonyme ne possède aucun
paquet : le CM n'envoie pas de `ClientLicenseList`, même après plusieurs
secondes.

La connexion nominative suit le chemin de JavaSteam (le moteur qui authentifie
GameNative) : elle n'utilise plus l'ancien `ClientLogon` par mot de passe, que
Steam refuse désormais.

```bash
./target/release/lapinux-probe account --login <compte>
```

Le mot de passe est demandé **sans écho**, chiffré en RSA PKCS#1 v1.5 avec la
clé rendue par `Authentication.GetPasswordRSAPublicKey`, puis remis à
`BeginAuthSessionViaCredentials` sur le canal CM. Steam Guard (code e-mail, code
de l'application ou confirmation dans l'application) est traité pendant
`PollAuthSessionStatus`. Le CM ne reçoit ensuite qu'un jeton de refresh dans
`ClientLogon` (champ `access_token`) : aucun mot de passe ne figure dans ce
message.

Le jeton est écrit dans `~/.local/share/lapinux/steam-session.json` en `0600`,
avec la donnée Steam Guard et sa date d'écriture, jamais le mot de passe.
`--login <compte>` reconnecte donc le compte sans rien redemander. Si le jeton
est refusé, le mot de passe est demandé et la session est remplacée.

Réglages › Compte Steam affiche l'état de ce jeton : son âge (« enregistré il
y a 6 j ») et le verdict de la dernière vérification. La vérification se fait
par le protocole (`ClientLogon` avec le jeton) dès que l'écran s'ouvre, en
arrière-plan ; un refus d'authentification marque le jeton **expiré** —
l'écran repasse alors en « non connecté » et propose « Renouveler par QR » —
tandis qu'une panne réseau le laisse « non vérifié » plutôt que de condamner
un jeton sain. Le verdict est attaché à la date du jeton : un fichier remplacé
n'hérite pas du verdict de l'ancien.

L'écran de connexion du lanceur suit le même parcours : le champ « Jeton
Lapinux » accepte un jeton collé (celui d'une autre installation, par
exemple) et le tente avant le mot de passe ; un jeton valide est enregistré
à son tour, sans jamais conserver de mot de passe. L'écran indique quand un
jeton est déjà enregistré et le bouton « Oublier » supprime ce fichier.

Le QR code passe par `Authentication.BeginAuthSessionViaQR` : Steam rend une
URL `https://s.team/q/…` (et la fait tourner toutes les vingt secondes pendant
l'attente), l'application Steam l'approuve, puis `PollAuthSessionStatus` rend le
jeton de refresh. Le QR est dessiné depuis la matrice du code (module `qrcode`),
sur fond blanc avec sa marge de quatre modules ; chaque nouvelle URL redessine
le code affiché.

Un défi QR dure deux minutes chez Steam, et la connexion au CM peut tomber
pendant l'attente : plutôt que d'abandonner sur une erreur réseau brute,
Lapinux renouvelle le défi (nouvelle URL, même écran) et repart d'un canal neuf
si besoin, dans un budget de cinq minutes. Passé ce délai, l'écran dit
« le défi QR n'a pas abouti ; relancez la connexion » au lieu d'un
« failed to fill whole buffer ».

Les quatre CM de la liste sont ouverts **en parallèle** et le premier canal
prêt gagne : essayés l'un après l'autre, un CM injoignable coûtait son délai
de connexion avant que le suivant soit tenté, et le QR mettait des dizaines de
secondes à s'afficher. Mesuré sur cette machine : l'URL est publiée **~0,5 s**
après le clic (`cargo run -p lapinux-steam --example qr_timing --features probes` ;
les sondes de `examples/` ne sont bâties qu'avec cette fonctionnalité, pour que
`cargo test` ne les recompile pas).

Le jeton de refresh n'est pas renouvelé : essayé, `GenerateAccessTokenForApp`
(réponse `AccessDenied`, eresult 15) refuse les jetons de type client Steam, et
l'essai peut consommer le jeton qu'on lui présente. Le daemon se contente donc
de réutiliser le jeton enregistré ; il reste valable environ 200 jours, puis
une reconnexion (QR ou mot de passe) en produit un neuf. C'est aussi ce que
font les clients qui persistent une session sans mot de passe. Steam peut le
révoquer avant terme si le compte se reconnecte ailleurs ou depuis
« déconnecter tous les appareils ». Attention, `eresult 5` (mot de passe
invalide) n'est pas une preuve de révocation : Steam répond la même chose
quand une autre session du compte vient de s'ouvrir. Mesuré ici : un jeton
refusé à 04 h 07 était accepté à 04 h 44, 1929 licences à l'appui. Lapinux ne
déclare donc jamais une session morte sur un seul refus : l'essai suivant, le
mot de passe ou le QR code sont tentés, et le journal garde la réponse exacte
de Steam (`connexion par jeton Lapinux refusée : …`).

Quand un mot de passe est saisi, l'écran n'affiche que l'erreur de cette
tentative — un jeton enregistré refusé au passage ne vient pas s'y ajouter —
et le préfixe de catégorie des erreurs (`authentification :`) est retiré.

La session sociale passe par un CM WebSocket en TLS. `ring` est installé
explicitement comme fournisseur rustls au démarrage (`install_crypto_provider`) :
`aws-lc-rs` est aussi compilé par nos dépendances, et sans choix explicite
rustls panique à la première connexion — ce qui tuait le fil social (amis,
présence, profil) sans bruit.

État vérifié : le service répond réellement (clé RSA, `BeginAuthSession` et
refus « mot de passe invalide » sur un compte de test inexistant), et les tests
couvrent le chiffrement, les messages, le jeton et les permissions du fichier.
Un login réussi et la récupération des licences d'un compte réel restent à
valider avec un vrai compte : cette machine n'en a pas.

Le démontage du moteur n'est pas sûr hors du client officiel
(`tier0/threadtools.cpp (3113)`, `tier1/mempool.cpp (313)`) : une fois le
rapport écrit, l'hôte rend donc la main sans laisser le moteur démonter ce
qu'il n'a pas construit — la sortie est reproductible.

`LAPINUX_STEAMCLIENT` permet de viser une autre bibliothèque pour comparer
deux arbres ; `LAPINUX_STEAM_CM` impose une liste de serveurs CM (`host:port`,
séparés par des virgules) au lieu de la découverte WebAPI. Les anciens noms
`LAPINUX_STEAMCLIENT` et `LAPINUX_STEAM_CM` restent lus.

Lapinux **installe et télécharge lui-même** : authentification CM, clés de
dépôts, manifestes et chunks viennent de notre client, sans SteamCMD ni client
Steam.

## Architecture

```
crates/core     chemins XDG, config TOML, modèles, store JSON, IPC, processus
crates/steam    VDF, scan des bibliothèques, manifests, DRM, catalogue,
                téléchargements natifs Lapinux, client CM et comptes
crates/proton   scan des Proton, releases GitHub, checksums, installation, préfixes, lancement
crates/perf     GameScope, MangoHud, supervision de session
crates/input    evdev, profils, pad virtuel uinput, navigation
crates/overlay  menu, rendu logiciel, shm Wayland, layer-shell
crates/daemon   état partagé, jobs, lancement, diagnostic, socket IPC
crates/ui       interface egui (console, big picture), thème gris neutre ;
                lib.rs = pages partagées, downloads.rs = page Téléchargements
                (état `Downloads`, cartes; ses tests vivent dans le même fichier)
assets/         marque du lapin et son nom écrit à la main (barre de fenêtre,
                démarrage), icône de fenêtre, trophée du platine et lapin
                d'attente du lancement
src/            binaire `lapinux`, sonde `lapinux-probe`, CLI, dispatch
packaging/      .desktop, service systemd utilisateur, règles udev
scripts/        smoke test et rapport de performances
```

IPC : socket Unix dans `$XDG_RUNTIME_DIR/lapinux/control.sock`, messages JSON
ligne par ligne (`lapinux_core::ipc`).

Chaque requête occupe un fil et un socket jusqu'à sa réponse : le daemon en
sert **64 au plus**, et répond « daemon occupé » aux suivantes plutôt que
d'empiler un fil de plus. Une lecture Steam qui traîne ne peut donc pas laisser
des milliers de connexions ouvertes ; les deux délais de la socket (lecture et
écriture) bornent l'attente, et la fermeture est explicite pour chaque
requête, refus compris.

## Tests

```bash
cargo test --workspace
./scripts/smoke.sh          # diagnostic + scan + protons + dry-run
./scripts/perf.sh           # RSS/CPU au repos, latence IPC, durée de scan
```

# Fonctionnalités

Référence complète, par domaine. Chaque paragraphe décrit un comportement observable ; le raisonnement derrière les choix est dans [ARCHITECTURE.md](ARCHITECTURE.md) et les cas non couverts dans [LIMITES.md](LIMITES.md).

## Sommaire

1. [Compte Steam et installation](#compte-steam-et-installation)
2. [Bibliothèque et boutiques](#bibliothèque-et-boutiques)
3. [Téléchargements : cartes et espace disque](#téléchargements--cartes-et-espace-disque)
4. [Mise à jour intégrée](#mise-à-jour-intégrée)
5. [Lancement, mode jeu et overlay](#lancement-mode-jeu-et-overlay)
6. [Manettes](#manettes)
7. [Interface, fiche de jeu et réglages](#interface-fiche-de-jeu-et-réglages)
8. [Profil et social](#profil-et-social)

## Compte Steam et installation

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

## Bibliothèque et boutiques

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
  renseigné, la synchronisation peut se faire **d'elle-même** : avant le
  lancement, les fichiers cloud absents localement sont récupérés. Si les
  deux côtés concordent, Lapinux prend un relevé ; à la sortie, les fichiers
  créés ou modifiés pendant la partie sont envoyés seulement si le cloud n'a
  pas changé depuis ce relevé. Une divergence demande un choix dans l'onglet
  Sauvegardes ; aucune suppression distante n'est automatique. Un relevé ou
  un envoi impossible est signalé dans le journal et la copie locale reste
  conservée. Les API employées n'offrent pas ici d'écriture conditionnelle
  vérifiée : une modification distante entre le dernier contrôle et l'envoi
  peut encore être écrasée, même si Lapinux vérifie à nouveau après l'envoi.

  La bibliothèque lue est gardée quinze
  minutes puis relue ; le bouton « Relire » passe outre. Un jeu de boutique
  s'installe dans `general.library_root`, comme un jeu géré par Lapinux :
  une tâche de téléchargement avec sa progression et son annulation, puis une
  marque `.lapinux-store.json` dans le dossier — c'est elle qui empêche le
  scan suivant de prendre un jeu GOG pour un jeu Steam. Exécutable et version
  (`linux`, `windows`, `macos`) sont retenus à l'installation : le jeu se
  lance ensuite comme les autres, Proton compris pour une version Windows.

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
  réunit les offres à conserver de **Steam**, **Epic Games**, **GOG** et
  **itch.io** — visuel, éditeur, valeur d'origine et date de fin locale
  (« Fin le 13/11/2026 à 17:00 ») — groupées par boutique, avec les offres
  annoncées pour la suite. Les free-to-play et les free weekends sont écartés :
  la page ne garde que ce qui peut être récupéré et conservé. **Panier Epic**
  ouvre un seul checkout gratuit avec toutes les offres encore disponibles —
  PC, Android et iOS — dans l'URL (`purchase?offers=…-#/free-checkout`) ; il
  s'ouvre dans le navigateur du bureau, car Cloudflare bloque la vérification
  de cette page dans la fenêtre WebKit intégrée ; les
  offres déjà possédées sont retirées une par une, y compris quand une seule
  plateforme d'un jeu mobile est déjà au compte ;
  **Récupérer sur Steam** ajoute directement les licences Steam, quand il y en
  a. Les offres GOG et itch.io ne s'ouvrent
  jamais toutes seules : leur carte est cliquable et ouvre l'offre choisie dans
  la fenêtre de boutique. La liste s'affiche sans compte connecté, et les
  offres Epic déjà au compte sont marquées « Récupéré ». Les offres PC viennent
  du flux de la boutique ; les **versions Android et iOS** de la semaine
  viennent du service mobile et s'affichent à leur suite, avec leur pastille de
  plateforme. Les offres déjà au compte sont lues dans les droits Epic et
  s'affichent « Récupéré », sans commande ni erreur. Une offre mobile se termine
  toujours sur sa page, d'où son bouton « Sur la boutique ». La fenêtre garde
  ses témoins de connexion d'une ouverture à l'autre : une session Epic ouverte
  une fois suffit à remplir le panier. L'offre est liée au compte, pas à une
  machine.

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

## Téléchargements : cartes et espace disque

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

## Mise à jour intégrée

- **Contrôle** : Réglages → Mises à jour affiche la version installée avec sa
  date de fabrication, puis la dernière release GitHub du projet avec sa date de
  publication. La comparaison est numérique (`1.10` vient après `1.9`), et un
  brouillon ou une pré-release n'est jamais proposé.
- **Téléchargement** : le bouton télécharge l'AppImage x86-64 de la release dans
  `~/Lapinux/Cache/Updates/<tag>/`, avec une barre de progression en
  pourcentage et le volume reçu. L'empreinte SHA-256 publiée par la release est
  vérifiée quand elle existe ; sans elle, le fichier est accepté tel quel.
- **Installation** : une fois le fichier complet, « Installer la mise à jour »
  l'écrit à côté de l'AppImage courante puis la renomme dessus — l'instance en
  cours garde l'ancien fichier ouvert, une copie interrompue ne casse rien.
  L'opération exige que Lapinux tourne depuis une AppImage (`$APPIMAGE`) : un
  binaire lancé depuis `target/` se remplace à la main. « Redémarrer
  maintenant » arrête le daemon et relance l'AppImage pour basculer sur la
  nouvelle version.

## Lancement, mode jeu et overlay

### Compatibilité graphique par jeu

Dans les réglages d'un jeu Windows, les options Proton, DXVK et VKD3D sont
facultatives et désactivées par défaut : WineD3D, DXVK D3D8, limite des
extensions OpenGL, désactivation de fsync/esync/ntsync, trois contournements
D3D9 et désactivation du ray tracing DXR de VKD3D. Elles sont transmises au
processus du jeu seulement, sans modifier ses fichiers ni son préfixe. Un
réglage explicitement désactivé cesse d'être transmis ; les variables saisies
par le joueur ou héritées de sa session sont conservées. Des conflits détectés
sont signalés avant le lancement. Proton/DXVK ancien peut ignorer certaines
options : esync est notamment obsolète dans Proton 11.

ReShade 6.8.0 ou OptiScaler 0.9.4 peuvent être installés séparément à côté de
l'exécutable de lancement du jeu : choix du proxy et confirmation, empreintes
SHA-256 figées, aucun écrasement et suivi pour retrait. Un seul injecteur
suivi à la fois. ReShade exige `d3dcompiler_47.dll` dans le préfixe ou près de
l'exécutable et ne fournit pas de pack d'effets ; OptiScaler exige `7z`, un
exécutable 64 bits et un Proton/une carte compatibles. Fermer aussi le jeu dans
Steam ou tout autre lanceur : Lapinux ne détecte que ses propres sessions. Le
proxy du dossier peut également être chargé hors de Lapinux. Les anti-triches
peuvent interdire ces outils. Après modification d'un fichier suivi, le retrait
le conserve et reste incomplet ; une extraction interrompue avant journalisation
peut laisser un dossier temporaire à examiner. Aucun rendu en jeu n'est garanti.

Pour les jeux anciens utilisant **DirectDraw logiciel**, l'option explicite
**cnc-ddraw 7.1** est proposée dans les réglages graphiques du jeu. L'interface
reprend l'exécutable de lancement déjà configuré — le seul emplacement accepté —
lit son architecture sans le lancer et n'active cnc-ddraw que pour un exécutable
32 bits (OptiScaler, lui, n'est proposé que pour un exécutable 64 bits). Le proxy
est fixé à `ddraw.dll` et expliqué, sans liste déroulante inutile. Cette option
exige un exécutable Windows PE 32 bits installé, identique à celui lancé par
Lapinux, et un Proton/Wine fonctionnel. Le jeu doit généralement utiliser son propre
mode plein écran ; ce wrapper ne prend pas en charge les jeux Direct3D et ne
garantit pas leur fonctionnement. Certains jeux nécessitent des étapes propres
à leur titre dans la documentation du projet. Un jeu qui lance un autre
exécutable peut demander une configuration différente et n'est pas garanti.

Après confirmation, Lapinux télécharge l'[archive officielle 7.1](https://github.com/FunkyFr3sh/cnc-ddraw/releases/tag/v7.1.0.0),
vérifie sa taille maximale et son SHA-256 (`0b13ab89a64c9918189b1dadd449ef6ed3cb3b7b19cabd96d8adbd95505bb908`,
également publié par le [miroir SourceForge](https://sourceforge.net/projects/cnc-ddraw.mirror/files/v7.1.0.0/cnc-ddraw.zip/download)),
puis extrait uniquement `ddraw.dll` et `ddraw.ini`, vérifiés individuellement,
sans exécuter l'installateur. La licence MIT est conservée dans le dossier de
suivi. Le shader GLSL de l'archive n'est pas installé : en son absence, le
rendu OpenGL de cnc-ddraw utilise son shader intégré de repli ; le rendu peut
différer. Aucun dossier `Shaders` existant n'est modifié.

Aucun fichier existant n'est écrasé : une DLL ou un INI `ddraw` existant, un
injecteur courant ou une configuration suivie en conflit bloque l'installation.
Un seul injecteur est suivi à la fois. WineD3D, vkBasalt et les générateurs
d'images sont refusés avec ce wrapper ; une variable `CNC_DDRAW_CONFIG_FILE`
ou un override `ddraw` préexistant bloque l'opération. Pour le lancement par
Lapinux, `ddraw=n,b` est ajouté aux autres overrides sans modifier le registre
Wine ; hors de Lapinux, le chargement dépend de la configuration Wine et peut
encore intervenir. Fermer le jeu aussi dans Steam ou tout autre lanceur avant
installation/retrait. Les liens physiques utilisés nécessitent un système de
fichiers compatible.

Le wrapper ou le joueur peut modifier `ddraw.ini`. Au retrait, tout fichier
modifié ou remplacé est conservé, un avertissement est affiché et le suivi reste
incomplet ; la DLL inchangée peut déjà avoir été retirée. Pour terminer ensuite,
sauvegarder manuellement les données souhaitées hors du dossier de suivi,
examiner les chemins signalés et retirer soi-même les copies ou liens restants,
puis réessayer le retrait. Ne pas supprimer aveuglément un fichier utilisateur.
Une interruption avant journalisation peut laisser le dossier temporaire non
suivi `.lapinux-cnc-ddraw-v1` à examiner. Le lancement hors Lapinux et les
paramètres propres à Wine/au jeu ne sont pas tous contrôlables par Lapinux.

Une fois un injecteur posé, la même section affiche un **diagnostic calculé sur
le disque et les réglages**, recalculé après chaque action et dès qu'un réglage
ou l'exécutable de lancement change : état des fichiers (intacts, manquants,
modifiés, copie identique à relier, `ddraw.ini` personnalisé), état de la copie
de suivi, override réellement appliqué au lancement (ajouté par Lapinux, déjà
fourni par `WINEDLLOVERRIDES`, ou bloqué avec sa cause) et installation
incomplète. Un fichier modifié n'est jamais écrasé silencieusement : « Réparer »
le déplace dans `.lapinux-<outil>-v1/user-modified/` avant de restaurer
l'original, et un `ddraw.ini` personnalisé reste intact. « Réparer » repose les
fichiers manquants depuis la copie de suivi, qu'elle reconstruit depuis
l'archive vérifiée si elle a disparu ; elle vise toujours l'installation
enregistrée, même si l'exécutable de lancement a changé. « Réinstaller » n'est
proposé que si cet exécutable a changé : il sauvegarde les DLL modifiées dans
`.lapinux-<outil>-user-modified/`, remet le `ddraw.ini` personnalisé en place
après l'installation, et reconstruit l'installation officielle même quand la
copie de suivi est incomplète ou corrompue. Dans ce cas, l'ancienne copie est
conservée sous `.lapinux-<outil>-stage-ancienne/` au lieu d'être effacée.

La fiche liste les dossiers de sauvegarde réellement présents — fichiers
modifiés et ancienne copie de suivi — avec un bouton qui les ouvre dans le
gestionnaire de fichiers. Seuls les dossiers existants et non vides sont
proposés ; la liste disparaît quand il n'y a rien de conservé. « Corriger les
réglages » n'est proposé que pour les conflits que Lapinux peut lever : un
conflit dû à une variable du joueur est expliqué, mais n'affiche pas de bouton
qui échouerait. « Corriger les réglages » désactive WineD3D, la génération
d'images et vkBasalt pour ce jeu, sans jamais supprimer une variable saisie par
le joueur : si `CNC_DDRAW_CONFIG_FILE`, `WINEDLLOVERRIDES` ou
`PROTON_USE_OPTISCALER` bloque encore, le message nomme la variable à retirer.

WineD3D reste une piste distincte pour certains jeux anciens, pas un correctif
DirectDraw universel. DirectPlay et les redistribuables ne sont pas installés
automatiquement. Sources : [instructions cnc-ddraw](https://github.com/FunkyFr3sh/cnc-ddraw/tree/v7.1.0.0),
[code du repli OpenGL](https://github.com/FunkyFr3sh/cnc-ddraw/blob/v7.1.0.0/src/render_ogl.c).

Sources : [options Proton](https://github.com/ValveSoftware/Proton/blob/proton_11.0/README.md#runtime-config-options),
[options DXVK](https://github.com/doitsujin/dxvk/blob/master/dxvk.conf),
[options VKD3D-Proton](https://github.com/HansKristian-Work/vkd3d-proton#environment-variables).

- **Proton** : détection dans Steam et `compatibilitytools.d`, téléchargement des
  releases GitHub GE-Proton / Proton-CachyOS avec vérification SHA-512,
  extraction pure Rust et installation dans `compatibilitytools.d`.

- **Lancement** : `proton run` direct avec l'environnement Steam
  (`STEAM_COMPAT_*`), `umu-run` pour les préfixes Lapinux quand le Proton choisi
  est complet (`toolmanifest.vdf`) ; un Proton local partiel est lancé par son
  script. Les préfixes Steam existants ne passent **jamais** par umu (umu réécrit
  le fichier `version`, Steam considère alors le préfixe comme invalide). Pour
  les jeux Steamworks, Lapinux ne démarre pas le client Steam. Un jeu natif
  reçoit son appid et peut utiliser le pont expérimental activé par jeu :
  détails et limites dans [Installation](INSTALLATION.md#pont-steamworks-linux-natif).
  La requête part en tâche de fond : dès
  que la commande est prête et que le jeu n'a pas encore affiché
  sa première image, la fenêtre se couvre et le **lapin animé**
  ([launch.rs](../crates/ui/src/launch.rs)) attend avec le nom du jeu — plein
  écran compris. C'est vrai pour **tous les jeux**, Steam comme boutiques :
  une réponse rapide du daemon (jeu de boutique) ne supprime plus l'attente,
  elle est seulement peinte après une grâce de 0,3 s pour ne pas clignoter si
  le jeu arrive tout de suite. L'indicateur animé est dessiné par Lapinux :
  les cinq variantes vectorielles retenues servent également aux cartes de
  chargement (résumé, succès, actualités), sans planche d'images externe.

- **Mode jeu** : GameScope (écran choisi dans les réglages, résolution et cap
  FPS repris par défaut du mode courant de l'écran principal, HDR, adaptive
  sync), HUD natif personnalisable par jeu, restauration à la sortie, journal
  par session.

- **Quick menu** : surface `wlr-layer-shell` (overlay) inspirée du menu rapide
  Steam Deck (panneau latéral, sections, interrupteurs), ouverte avec le bouton
  **Guide** de la manette ou **Shift+Tab** au clavier par défaut, navigation
  clavier, souris et manette, actions session (profil, capture, arrêt et amis).
  La page **Performances** regroupe l'interrupteur du HUD, la limite de FPS,
  les profils **Simple**, **Basique**, **Détaillée** et les neuf positions.
  Ses **Options avancées** donnent accès au HUD natif : disposition (**En ligne**
  ou **En bloc**), graphique de frametime, cœurs CPU et taille du HUD. Le mode
  **En ligne** s'arrête après la dernière statistique au lieu d'étirer le HUD sur
  toute la largeur de l'écran. Les couleurs, l'ordre et les unités se règlent dans
  Réglages → En jeu ; le HUD relit sa configuration à chaud. La limite de FPS
  change aussi à chaud sous GameScope, via `gamescopectl` ; hors de lui, elle
  attend le prochain lancement.
  Pendant l'overlay,
  le service d'entrée transmet la navigation à la croix directionnelle et aux
  boutons A/B ; maintenir une direction répète le déplacement. Dans les amis,
  droite envoie une invitation à l'ami sélectionné.

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

## Manettes

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
  défaut, `hid-steam` autorise le mode clavier/souris au repos ; un client qui
  ouvre le gamepad peut le désactiver automatiquement. La configuration
  `packaging/99-steam-controller.conf` (`options hid_steam lizard_mode=0`)
  désactive cette émulation au repos. Tant qu'aucun gamepad n'est détecté,
  la page signale le Puck ; elle ne déduit pas de sa présence que la manette
  est allumée ou que le pilote est installé. Le pont historique
  `steam-puck-bridge`, s'il prend le hidraw du Puck, crée un pad
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

## Interface, fiche de jeu et réglages

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
  réglages généraux, la limite de FPS, GameScope, le HUD natif, la résolution et
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
  La **bande de tags de la fiche** porte l'état : « Cloud 3 fichiers · à jour
  · sync 24/10 · sur ce PC », « à récupérer » quand un fichier manque ou que le compte en a un
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
  d'attente_ — et l'interrupteur du même nom à côté de GameScope —
  choisit ce qui couvre **tout l'écran** pendant qu'un jeu démarre : la
  **capsule du jeu** (sans logo qui la traverse), l'**indicateur animé** Lapinux,
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

- **Système** : onglet des Réglages avec le modèle CPU, les threads, la charge,
  la mémoire utilisée, les cartes graphiques, le pilote et le renderer OpenGL
  réellement utilisé. Lecture directe de `/proc` et `/sys/class/drm`, sans
  dépendance supplémentaire.

## Profil et social

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

---

[Documentation](README.md) · [README du projet](../README.md)

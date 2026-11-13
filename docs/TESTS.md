# Tests et vérifications

```bash
./scripts/check-i18n.sh
cargo test --workspace
```

Attention : `scripts/smoke.sh` agit sur l'environnement réel et crée un pad
virtuel ; `scripts/perf.sh` mesure et utilise aussi la machine réelle. Ne pas
les lancer sans avoir examiné leurs effets et obtenu l'accord nécessaire.

## Ce que couvre la suite

- **Tests unitaires inline** (~1 300) : chaque module teste son contrat —
  aller-retour JSON des requêtes IPC, lecture VDF et manifestes, chiffrement et
  messages du protocole CM, permissions des fichiers de jeton, normalisation de
  `config.toml`, indicateurs vectoriels de chargement et placement du HUD natif.
- **Tests d'intégration** : `tests/install_flow.rs` monte une bibliothèque
  complète avec un Proton factice, installe un jeu géré, le lance réellement, le
  relit après suppression du manifeste et après redémarrage du daemon, et vérifie
  qu'un manifeste Steam sans fichiers n'est jamais « installé ».
  `crates/daemon/tests/configurations.rs` couvre l'installation et le retrait
  d'une configuration communautaire, fichiers sauvegardés compris.
- **Traductions** : `cargo test -p lapinux-core` refuse une clé manquante, un
  placeholder divergent, un libellé vide, et vérifie le repli vers le français.
- **Banc du HUD natif** : le calcul des neuf ancrages est vérifié hors écran (fenêtre
  605 × 40 sur 1920 × 1080) avant de construire la couche.

## Sondes réseau et matérielles

Les binaires d'`examples/` ne sont construits qu'avec la fonctionnalité
`probes`, pour que `cargo test` ne les recompile pas :

```bash
cargo run -p lapinux-steam --example qr_timing --features probes
cargo run -p lapinux-steam --example login_probe --features probes
```

Elles servent à mesurer le réel (latence du QR, contenu des licences, tailles de
dépôts), pas à valider une livraison : leurs réponses dépendent du compte et de
la machine.

## Sonde d'historique de chat (hors ligne)

`./scripts/steam-chat-history-audit.sh` exécute les tests du contrat
`FriendMessages.GetRecentMessages#1` puis compte les messages de l'archive
locale, sans texte ni identifiant de contact. Elle ne lance pas le client Steam
et ne contacte aucun service ; elle ne peut donc pas mesurer la rétention d'un
compte réel, seulement vérifier le contrat et ce qui est déjà conservé.

```bash
./scripts/steam-chat-history-audit.sh
```

## Avertissement

Les fonctionnalités dépendent de la distribution, des pilotes, des permissions,
des jeux et des comptes utilisés. Les tests automatisés ne remplacent pas un
essai du lancement de jeu, du bureau graphique ou du matériel réel. Certains
tests accèdent au réseau ou au système : vérifier leur portée avant de lancer la
suite sur une machine contenant des données personnelles.

`LAPINUX_HOME` isole le profil Lapinux (configuration, données, cache, socket),
mais ne remplace pas l'isolation de `HOME`, du bureau ou des périphériques :
les racines Steam et les dossiers personnels utilisent aussi `HOME`. Pour les
tests sans clients, isoler les deux et les périphériques ; inspecter aussi les
commandes enfants. `tests/install_flow.rs` utilise un Proton factice et un
profil temporaire.

## Résultats de l'audit du 13 novembre 2026

- `./scripts/check-i18n.sh` : réussi après modification concurrente de la garde
  par l'agent de traduction ; une première exécution avait signalé à tort la
  valeur de configuration `aucun`.
- `cargo test --workspace` avec profil, HOME et périphériques isolés : échec
  code 139 dans les tests UI parallèles ; les tests des autres crates exécutés
  jusque-là passent. Cause identifiée depuis : deux tests de snapshots créer
  une instance GPU en parallèle et `wgpu::Instance::new` segmente dans
  `loader_icd_scan` (libvulkan) ; `RUST_MIN_STACK=64Mio` ne change rien, ce
  n'est pas la pile. Corrigé par un verrou de processus autour de la création
  GPU des captations (`crates/ui/src/snapshot_gpu.rs`).
- `cargo test -p lapinux-ui --lib -- --test-threads=1` : 530 réussis, 1 ignoré.
  Avec `LAPINUX_LANG=en`, 530 réussis, 1 ignoré : les assertions comparent
  désormais aux catalogues (`t`, `t_args`, `t_fmt`) au lieu de littéraux
  français, et les références anglaises de `news` et `steam-input` ont été
  régénérées. Seule clé ajoutée : `game_sheet.download_version` ; les clés
  existantes n'ont pas été touchées. `LAPINUX_LANG=en cargo test -p lapinux-core
  --lib -- --test-threads=1` : 148 réussis.
- Ciblés : lancement/compatibilité des réglages et indicateurs (7), refus
  SteamCMD (1), annulation interrompue/en pause (1), tray (8), sauvegardes (26),
  tests daemon Steam (43), pause (1), parcours d'installation factice (2) :
  réussis dans le même profil isolé. Cela ne valide pas un téléchargement réel.
- Ignorés : deux tests KWin, cinq tests Steam réseau et un banc UI ; les tests
  situés après l'échec workspace ne sont pas déclarés réussis par cette commande.
- AppImage de test : contenu, notices et extraction vérifiés ; binaire extrait
  refusé sur Ubuntu 20.04/22.04, Debian 11 et Fedora 41 pour des symboles GLIBC.
  Sans `/dev/fuse`, le mode normal a échoué (127) et le mode
  `--appimage-extract-and-run --version` a réussi. Le runtime local utilisé n'a
  pas été authentifié indépendamment, et le build de développement contient un
  chemin local : ne pas diffuser cette image. Voir
  [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md).

## Complément : diagnostic et sécurité (13 novembre 2026)

Dans le profil isolé décrit plus haut, les tests ciblés vérifient l'export
JSON sans daemon ni écrasement de fichier, l'exclusion des champs libres du
rapport, le mode `0600`, les écritures de jetons simultanées et le refus de
suivre le lien final. Ils vérifient aussi le refus d'arrêt avec tâche en cours,
en pause ou en attente, moteur annulé encore actif ou requête en cours, ainsi
que le refus d'envoyer un ordre d'arrêt non protégé à un ancien daemon.

L'extraction ZIP a été testée avec des liens symboliques intermédiaires et
finaux, un lien dur préexistant et une archive normale, puis avec des entrées
`..`, absolues, un fichier régulier là où l'archive attend un dossier, une
destination liée, et un bit setuid dans l'archive : chaque cas est refusé ou
neutralisé sans écriture hors de la destination. Un téléchargement HTTP local
tronqué n'écrase pas le fichier précédent.

Les jetons sont écrits en `0600` dans un dossier créé en `0700` quel que soit
le umask, un fichier existant redevient privé, et un renommage manqué ne
laisse aucun temporaire. Côté daemon, un fil qui panique en tenant le portail
d'arrêt ne rend plus le daemon muet : les requêtes suivantes passent et
l'arrêt reste décidable. Ces tests ne valident pas les autres formats
d'archive, installateurs exécutables ou téléchargements réels.

Suite UI relancée après ces changements, en série et en parallèle, dans le
profil isolé : français **530 réussis, 1 ignoré** ; anglais **530 réussis,
1 ignoré**. Le code 139 venait de la création GPU concurrente des deux tests
de snapshots, sérialisée depuis ; les assertions anglaises comparent aux
catalogues, et les deux références visuelles anglaises obsolètes (mise en page
antérieure à la refonte de la page) ont été régénérées avec
`UPDATE_SNAPSHOTS=1` dans un bac à sable dont les sources sont inscriptibles.
Ces tests ne remplacent pas une relecture humaine des deux langues.

Sans aucun client de boutique : tests de manifestes Steam (8), manifestes Epic
(6), entrée de bibliothèque et choix d'installateur GOG (2), bibliothèque et
choix d'archive itch (2). Ce sont des contrats hors ligne, pas des essais de
connexion ou d'installation de jeux réels. La garde i18n UI passe (3 tests).

Le diagnostic exportable est disponible dans la page Diagnostic ou avec
`lapinux cli diagnostic-export <nouveau-fichier.json>`. Il ne démarre pas le
daemon depuis la CLI et ne remplace pas un fichier existant. Il contient des
états techniques et comptes agrégés de tâches, sans journaux bruts, noms de
jeux, chemins ni informations de compte. Les valeurs absentes sont nulles ou
`unknown`. Ces restrictions réduisent les données personnelles mais ne
constituent pas une garantie absolue d'anonymat, ni une couverture de tous les
incidents : relire le rapport avant partage.

## AppImage issue du rangement

Une AppImage distincte a ensuite été construite dans un conteneur Ubuntu 22.04
avec le runtime officiel type 2 version `20251108`, vérifié contre son SHA-256
publié. Elle contient les binaires, le pont natif et les notices identifiées.
En profil isolé, `--appimage-extract-and-run --version` et un scan vide ont
réussi ; le binaire extrait a démarré dans les conteneurs Ubuntu 22.04 et Fedora
41, mais pas Ubuntu 20.04 ni Debian 11 (GLIBC 2.35 requise). Le montage FUSE,
l'interface graphique et les jeux n'ont pas été vérifiés. Une recherche
littérale du chemin local du projet dans les trois binaires n'a rien trouvé,
mais ne constitue pas un audit complet des données personnelles.

## Vérifications manuelles utiles

| Ce qui est vérifié | Commande |
| --- | --- |
| environnement et permissions | `lapinux cli doctor` |
| accès evdev réel | `lapinux cli access` |
| plan de lancement sans lancer | `lapinux cli launch <appid> --dry-run` |
| pad virtuel uinput | `lapinux cli virtual-pad` |
| quick menu hors partie | `lapinux cli overlay-demo --seconds 1` |
| configuration effective | `lapinux cli config` |

## Ce qui manque encore

L'intégration continue (`.github/workflows/ci.yml`) exécute la garde i18n,
`cargo check --workspace`, toute la suite du workspace, puis les suites UI et
cœur en anglais. Les parseurs binaires et texte (`vdf`, `manifest`, `cm/wire`,
`cm/proto`, `epic_manifest`) n'ont pas encore de fuzzing, alors que ce sont les
entrées les plus exposées.

---

[Documentation](README.md) · [README du projet](../README.md)

## Garde i18n

La CI lance ce script avant la suite complète. `scripts/check-i18n.sh` exécute la garde `crates/ui/tests/i18n_guard.rs` puis les
tests de catalogue du cœur. Elle échoue si les catalogues fr/en divergent, si
une clé appelée par l'interface manque, ou si un littéral d'interface visible
non traduit (accent, mot français, ou texte passé directement à un helper
d'affichage) réapparaît. La liste d'exceptions du test est courte et chaque
entrée est justifiée : données externes, identifiants internes, arguments de
commande. Les noms propres et symboles sont admis séparément.

Lancer aussi les deux langues en série :

```sh
cargo test -p lapinux-ui --lib
LAPINUX_LANG=en cargo test -p lapinux-ui --lib -- --test-threads=1
cargo test -p lapinux-core --lib
LAPINUX_LANG=en cargo test -p lapinux-core --lib -- --test-threads=1
```

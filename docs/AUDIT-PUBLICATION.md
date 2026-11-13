# Audit avant publication

État de l'examen : 13 novembre 2026. Ce document distingue les risques
observés dans les sources des problèmes reproduits en conditions réelles.
Une suite de tests réussie ne prouve pas qu'un jeu ou service fonctionne.

## Complément après les corrections du 13 novembre

- **Diagnostic** : export JSON volontaire, limité à des champs techniques
  autorisés, sans détails libres, chemins, noms de jeux ou données de compte.
  Il ne permet pas d'expliquer tous les problèmes et ne garantit pas un
  anonymat parfait. Le bouton UI et les dialogues restent à vérifier visuellement.
- **Jetons** : fichier temporaire unique créé en `0600`, synchronisé et publié
  par renommage ; tests d'écritures concurrentes et de lien final réussis.
- **Arrêt du daemon** : une nouvelle requête `ShutdownIfIdle` refuse l'arrêt
  quand une requête, une session, une tâche active ou un moteur d'annulation
  subsiste. Les anciens daemons qui ne comprennent pas cette requête ne sont
  plus arrêtés automatiquement. Une mise à jour peut donc nécessiter un arrêt
  manuel du daemon une fois les opérations et jeux terminés. La couverture des
  autres fils de fond et les essais en charge restent à compléter.
- **Téléchargement / ZIP** : temporaires uniques, contrôle de longueur HTTP et
  conservation de la destination en cas d'échec ; ZIP protégé contre plusieurs
  liens préexistants, avec tests ciblés. Les autres formats et installateurs
  externes ne sont pas couverts par ces protections.
- **Nettoyage** : retrait du cache de compilation Ubuntu (environ 2,2 Gio), de
  trois AppDir extraits temporaires (environ 132 Mio) et d'un dossier de tests
  vide. Images de `dist/`, copies d'AppImage, journaux et caches partagés
  conservés. L'image Ubuntu construite précédemment ne contient pas les
  modifications ajoutées depuis sa construction ; elle doit être reconstruite.
- **UI** : extraction du sélecteur de dossier dans son propre module ; le
  découpage des grands modules reste progressif et largement à faire.

Les constats initiaux ci-dessous sont conservés pour leur contexte : les
corrections et limites actuelles ci-dessus priment lorsqu'ils se contredisent.

## Priorité haute

| Constat | Preuve et risque | Suite et vérification |
| --- | --- | --- |
| Compatibilité AppImage | Le binaire d'AppDir existant et celui de l'image de test référencent GLIBC_2.43. Le binaire extrait refuse effectivement `--version` dans des conteneurs sans réseau Ubuntu 20.04, Ubuntu 22.04, Debian 11 et Fedora 41 : versions GLIBC manquantes. | Construire sur une base de distribution choisie et vérifier `readelf --version-info` ; tester l'image complète sur chaque distribution ciblée. Les conteneurs ne vérifient ni FUSE ni bureau graphique. |
| Origine et licences | Cargo annonce MIT mais il n'y a pas de licence générale du projet. Polices, egui, vkBasalt, shaders, marques, illustrations et runtime ont chacun une origine à documenter. | Le propriétaire choisit la licence ; inventorier les droits, conserver les notices et vérifier les conditions de redistribution. Ne pas inventer une licence. |
| Mises à jour pendant un transfert | `crates/core/src/client.rs::restart_stale_daemon` remplace un daemon antérieur ; `crates/daemon/src/lib.rs::restore_jobs` transforme les tâches en cours en `Interrupted`. | Éviter le redémarrage pendant un téléchargement ; concevoir une mise à jour sûre et tester les interruptions, files et reprises. L'état est conservé, la reprise automatique n'est pas garantie. |
| Sauvegardes cloud | `crates/daemon/src/stores.rs` compare les versions mais les API n'offrent pas partout d'écriture conditionnelle ; une modification distante entre vérification et envoi reste un risque. `steam_cloud.rs` refuse des divergences détectées, mais la comparaison du fichier local existant précède le renommage : une modification concurrente dans cet intervalle n'est pas exclue. | Tester sur des comptes et fichiers de test, conflits simultanés compris ; documenter les limites et ne jamais supprimer une sauvegarde réelle pour tester. |
| Extraction et programmes tiers | `crates/stores/src/common.rs::unzip` refuse les chemins hors destination (`..`, absolus), les liens symboliques de l'archive, les liens préexistants (dossier, fichier final, destination liée) et les liens durs ; les bits setuid/setgid de l'archive sont perdus. Restent hors protection : les rar/7z/tar, confiés aux outils installés, `run_gog_installer` qui exécute un script fourni par le jeu, et `install_innoextract` qui télécharge un binaire sans empreinte épinglée. | Traiter archives et installateurs comme non fiables : essais dans une sandbox, vérification des outils téléchargés, isolement des installateurs exécutables. Ne pas affirmer qu'une archive est sûre parce qu'elle vient d'un CDN. |
| Absence de clients de boutique | L'ancienne commande `lapinux-probe runtime --prepare` pouvait exécuter SteamCMD. Elle est maintenant refusée et le module de recherche `crates/steam/src/runtime.rs` n'est plus compilé ; le fichier historique reste présent pour examen. Les parcours principaux utilisent les services directement. | Ajouter des tests isolés avec HOME et caches vides couvrant les parcours de chaque boutique ; vérifier aussi les outils externes et jeux réels. La commande désactivée ne prouve pas l'absence de tout autre chemin indirect. |
| Jetons locaux | `crates/steam/src/session.rs::save_to` tronque la session Steam existante avant d'écrire ; une interruption peut l'endommager. `crates/core/src/stores.rs::save_tokens` applique `0600` après création du fichier temporaire, dont les permissions initiales dépendent de l'umask. | Créer directement en mode `0600`, utiliser un temporaire unique sans suivre les liens et publier atomiquement après synchronisation ; tester permissions, interruptions et écritures simultanées. Éviter de recopier des jetons dans un rapport. |
| Publication des secrets | Le binaire de l'image d'audit en profil `dev` contient le chemin absolu du projet ; l'image n'est pas destinée à la diffusion. `.gitignore` exclut notamment `.env`, `dist/` et `.cloud-attachments/`, mais le dépôt contient beaucoup de fichiers modifiés ou non suivis. Les exemples et journaux peuvent contenir des données de compte. | Avant toute publication, contrôler le livrable `dist`, les sources et l'historique avec une recherche de données personnelles et secrets confidentielle ; ne pas diffuser les profils utilisateur, logs, captures ou archives de conversation. Un contrôle de chemin seul n'est pas une certification de sécurité. |

## Priorité moyenne

| Constat | Preuve et risque | Suite et vérification |
| --- | --- | --- |
| Configuration concurrente | `crates/ui/src/settings_window.rs::autosave_config` envoie une copie complète depuis un fil détaché ; d'autres chemins envoient aussi `SetConfig`. Un ancien instantané pourrait arriver après un nouveau. | Reproduire par tests concurrentiels, sérialiser ou versionner les écritures et vérifier le brouillon des réglages. Hypothèse à confirmer, pas perte de données observée ici. |
| Icône du tray | `crates/daemon/src/tray.rs::fetch_steam_icon` fait des requêtes synchrones, ne retente qu'une fois par session et lit la réponse sans limite explicite ; la clé d'actualisation ne dépend pas de l'apparition tardive d'une image. | Charger en arrière-plan avec limites, validation, temporisation et nouvelle tentative ; tester les images absentes/corrompues, le réseau coupé et les téléchargements simultanés. |
| Traduction et contrats de tests | Corrigé depuis : les tests UI anglais passent (530 réussites, 1 ignoré, en série comme en parallèle) après remplacement des littéraux français par les clés des catalogues et régénération des deux références anglaises obsolètes. Une seule clé ajoutée, `game_sheet.download_version`. | Vérification humaine de l'anglais encore à faire : les références visuelles prouvent une mise en page, pas une traduction juste. Les catalogues n'ont pas été réécrits. |
| Gros modules | `crates/ui/src/lib.rs` compte 23 109 lignes et `downloads.rs` 6 849. Les responsabilités et tests sont difficiles à isoler. | Découper progressivement par domaine après stabilisation de la traduction, avec tests de contrat et modifications indépendantes. Pas de refonte globale juste avant livraison. |
| Code natif et appels bloquants | `crates/steam/src/probe.rs` utilise `dlopen`, `dlsym` et des appels de tables virtuelles `unsafe` dont l'ABI dépend de bibliothèques externes ; `settings_window.rs::apply_config` attend une requête IPC sur le fil UI. | Isoler les sondes natives dans un processus, vérifier leurs entrées et les échecs d'ABI ; tester délais IPC et indisponibilité du daemon sans figer l'interface. Ne pas conclure à une faille démontrée sans reproduction. |
| Tests de publication | `cargo test --workspace` isolé s'est terminé avec le code 139 dans les tests UI parallèles, causé par deux créations GPU simultanées (`loader_icd_scan` de libvulkan) et non par la pile. `snapshot_gpu.rs` sérialise cette création : la suite UI passe désormais en parallèle dans les deux langues. La CI vérifie le contenu d'une image avec un runtime factice, pas FUSE, le bureau, les comptes ou le matériel. | Ajouter des essais système isolés, compatibilité Wayland/X11, capture de logs anonymisés, performances mesurées et sondes de protocoles. Les tests marqués `ignored` restent à examiner séparément. |
| Ponts et DRM | Le pont Linux est expérimental et l'AppImage le contient désormais ; le pont Windows et l'environnement des jeux ont d'autres contraintes. Le retrait ou la restauration de DLL demande une prudence particulière. | Tester les ponts sur jeux factices et des jeux de test autorisés, avec arrêt brutal ; vérifier les limites DRM et anti-triche avant d'annoncer une prise en charge. |
| Distribution | Le script demande une empreinte du runtime et ne remplace plus l'image existante. Une empreinte calculée depuis un fichier local non vérifié n'établit pas sa provenance. | Choisir un runtime officiel versionné et vérifier sa provenance par un canal indépendant ; fournir les notices, les sommes de contrôle et une signature si disponibles. Tester aussi le comportement FUSE et extraction. |

## Inventaire et nettoyage

La carte de l'arborescence figure dans [ARCHITECTURE.md](ARCHITECTURE.md).
Les anciennes sources `Bunny/` et la planche embarquée `assets/bunny-sheet.png`
ont été supprimées après remplacement par les cinq dessins vectoriels déjà
présents dans `crates/ui/src/section_icons.rs`.

Conservés volontairement : `target/` (caches et builds, environ 28 Go lors du
premier inventaire), `dist/` (dont l'AppImage existante), maquettes HTML/PNG à
la racine et dans `assets/`, outils, sondes et le module historique non compilé
`crates/steam/src/runtime.rs`. Leur usage ou leur intérêt historique doit être
confirmé avant suppression ; un fichier non suivi n'est pas forcément inutile.
Les ressources embarquées, `vendor/egui-0.29.1` et leurs licences ne sont pas
des déchets de compilation.

## Matrice de validation à compléter

| Domaine | Couverture disponible | Validation réelle restante |
| --- | --- | --- |
| Connexion et bibliothèques | Tests des parseurs, IPC, catalogues et gestion de jetons. | Comptes de test Steam/Epic/GOG/itch, 2FA, session expirée, aucun client installé. |
| Installation et lancement | Tests de file, pause, annulation, redémarrage et lancement avec jeu/Proton factices. | Téléchargements réels, interruption réseau, reprise après arrêt et fenêtre masquée ; jeux et DRM variés. |
| Cloud et données | Tests de conflits, chemins et copies factices. | Deux machines/comptes de test, sauvegardes de test et pannes à chaque phase. |
| Manettes et overlay | Tests de mapping, rendu et logique. | Manettes et pilotes réels, permissions, KDE/Wayland et autres bureaux, jeux en plein écran. |
| Captures et audio | Tests unitaires ; certaines sondes KWin sont ignorées. | GPU, encodeurs, audio, permissions et portails réels. |
| Réglages et traduction | Garde des catalogues réussie ; suite UI réussie en français et en anglais, en série et en parallèle (530 réussis, 1 ignoré). | Vérification humaine des deux langues et redémarrage avant publication. |
| AppImage | Construction locale de test et extraction ; `--version` échoue sur Ubuntu 20.04/22.04, Debian 11 et Fedora 41 (GLIBC). Sur l'hôte sans `/dev/fuse`, mode normal refusé et `--appimage-extract-and-run --version` réussi. Runtime non authentifié indépendamment. | Build de distribution, runtime authentifié, mode FUSE, bureaux et pilotes cibles. |
| Visuel | Tests de dessin hors écran. | Contrôle par l'utilisateur : pas de pilotage ni capture d'interface par cet audit. |

Consulter [TESTS.md](TESTS.md) avant les commandes : certains tests et sondes
peuvent accéder au réseau, au matériel ou à des comptes. `scripts/smoke.sh`
agit sur la machine réelle et crée un pad virtuel ; ne pas le lancer par défaut.

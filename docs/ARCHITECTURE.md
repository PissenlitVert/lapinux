# Architecture

Lapinux est un espace de travail Cargo : un binaire principal (`lapinux`), une
sonde séparée (`lapinux-probe`), et des crates qui séparent les protocoles des
interfaces.

```
crates/core     chemins XDG, config TOML, modèles, store JSON, IPC, processus
crates/steam    VDF, scan des bibliothèques, manifests, DRM, catalogue,
                téléchargements natifs Lapinux, client CM et comptes
crates/stores   Epic Games, GOG, itch.io : comptes, bibliothèques, cloud, succès
crates/proton   scan des Proton, releases GitHub, checksums, installation, préfixes, lancement
crates/perf     GameScope, préparation du lancement, supervision de session
crates/input    evdev, profils, pad virtuel uinput, navigation
crates/overlay  menu, rendu logiciel, shm Wayland, layer-shell
crates/daemon   état partagé, jobs, lancement, diagnostic, socket IPC
crates/ui       interface egui (console, big picture), thème gris neutre ;
                lib.rs = pages partagées, downloads.rs = page Téléchargements
                (état `Downloads`, cartes ; ses tests vivent dans le même fichier)
assets/         logos, polices, marques de boutiques, shaders et runtimes embarqués
src/            binaire `lapinux`, sonde `lapinux-probe`, CLI, dispatch
packaging/      .desktop, service systemd utilisateur, règles udev
scripts/        compilation AppImage et runtimes, vérifications et sondes
```

`crates/steamshim` et `crates/steamshim-win` sont des shims séparés (le second
se construit hors de l'espace de travail, pour Windows) : ils implémentent la
surface `steam_api` attendue par un jeu Steamworks et relaient vers le daemon.

## Carte du projet

| Dossier | Contenu et précautions |
| --- | --- |
| `src/` | Points d'entrée du binaire, CLI et sonde expérimentale. |
| `crates/core/` | Modèles, configuration, IPC, traduction et persistance. |
| `crates/steam/`, `crates/stores/` | Protocoles et services des boutiques, sans client de boutique requis. |
| `crates/steamshim/`, `crates/steamshim-win/` | Ponts Steamworks expérimentaux Linux et Windows ; ce dernier est un projet Cargo distinct. |
| `crates/proton/`, `crates/perf/` | Proton, lancement, GameScope et performances. |
| `crates/input/`, `crates/overlay/`, `crates/ui/`, `crates/daemon/` | Manettes, overlay, interface et état des tâches. |
| `assets/` | Images, polices et composants embarqués dans le binaire ; certains sont nécessaires à la compilation. Les notices correspondantes doivent être conservées. |
| `configurations/` | Catalogue et schéma des correctifs communautaires. |
| `packaging/` | Entrée de bureau, service utilisateur et règles système facultatives. |
| `scripts/` | Construction, diagnostics et tests ; consulter chaque script avant de l'exécuter. |
| `tests/` et `crates/*/tests/` | Tests d'intégration ; les tests unitaires sont aussi dans les sources. |
| `tools/` | Code d'outils graphiques auxiliaires. |
| `vendor/` | Dépendance egui locale corrigée, requise par le manifeste Cargo. |
| `docs/` | Guides, référence, recherches et audit avant publication. |
| `.github/` | Configuration de l'intégration continue. |
| `target/` | Résultats et caches Cargo générés, ignorés par Git ; leur retrait impose des reconstructions et peut gêner une compilation en cours. |
| `dist/` | AppImages, binaires et fichiers de construction locaux, ignorés par Git ; peut contenir un livrable à préserver. |

## Qui fait quoi

- **L'interface** envoie les requêtes de catalogue et de tâches au daemon et
  affiche les réponses. Certaines images ou pages distantes ont leurs propres
  chargements ; leurs attentes utilisent les indicateurs Lapinux.
- **Le daemon** possède l'état : bibliothèque scannée, jobs, sessions, session
  Steam, présence sociale, overlay. Les tâches longues y tournent en arrière-plan
  et l'interface sonde.
- **Les crates protocole** (`steam`, `stores`, `proton`) ne connaissent ni egui
  ni le daemon : ce sont des bibliothèques testables seules.
- **L'overlay** est un processus séparé, lancé par le daemon, qui dessine une
  surface `wlr-layer-shell` au-dessus du jeu.

## IPC

Socket Unix dans `$XDG_RUNTIME_DIR/lapinux/control.sock`, en `0600` : un autre
utilisateur de la machine ne peut pas s'y connecter. Les messages sont du JSON
préfixé par sa longueur (`lapinux_core::ipc`), avec une trame plafonnée à
16 Mio.

Chaque requête occupe un fil et un socket jusqu'à sa réponse : le daemon en sert
**64 au plus**, et répond « daemon occupé » aux suivantes plutôt que d'empiler
un fil de plus. Une lecture Steam qui traîne ne peut donc pas laisser des
milliers de connexions ouvertes ; les deux délais de la socket (lecture et
écriture) bornent l'attente, et la fermeture est explicite pour chaque requête,
refus compris.

Un délai de socket est distingué d'un refus de connexion (`IpcTimeout` contre
`Ipc`) : un daemon occupé ne doit pas passer pour mort, sinon un second
processus serait lancé par-dessus, incapable de reprendre le socket.

`Ping` renvoie la date de modification et la version du binaire du daemon :
l'interface compare avec son propre binaire et demande le remplacement d'un
ancien daemon uniquement par `ShutdownIfIdle`, quand celui-ci prend en charge
l'arrêt protégé. Les anciennes versions sans ce protocole sont conservées ;
une fois toute opération terminée, leur arrêt manuel peut être nécessaire.
Un daemon ancien pourrait ignorer les réglages récents et les effacer en réenregistrant la
configuration.

## Persistance

| Support | Contenu |
| --- | --- |
| `lapinux.redb` | jeux, sessions, historique, tâches ; accès transactionnel |
| `config.toml` | réglages, réécrits en entier quand une clé manque |
| `steam-catalog.json` | catalogue Steam mis en cache (relu en arrière-plan) |
| `steam-session.json`, `stores/<boutique>.json` | jetons en `0600` ; la session Steam est écrite en place, les jetons de boutiques par renommage |
| `cache/` | jaquettes, illustrations, installateurs : jetables |

Les chemins de journalisation examinés ne consignent pas directement les
secrets d'authentification ; cela ne garantit pas que tous les journaux et
messages d'erreur soient exempts de données personnelles. La session Steam est
écrite en `0600`, mais son remplacement n'est pas atomique ; voir l'[audit](AUDIT-PUBLICATION.md).
Les jaquettes choisies à la main vivent dans `data/artwork`, hors du cache, pour
survivre à un vidage.

## Conventions

- **Une seule session Steam** pour toutes les lectures, sur un moteur réseau
  commun au processus : une session posée sur un runtime jetable mourait avec
  lui, d'où un logon par lecture. La session sociale est distincte et vit tant
  que le daemon tourne.
- **Les erreurs remontent** jusqu'à la carte ou l'écran concerné : un échec
  réseau sur les sauvegardes cloud ne bloque pas le lancement, il va au journal.
- **Diagnostic** : certains refus de Steam sont journalisés (`eresult`) et
  certaines fiches expliquent les champs manquants ; il reste des erreurs
  ignorées ou sans message à examiner avant publication.
- **Les commentaires expliquent le pourquoi**, en français, y compris les
  mesures qui ont conduit au choix (« 169,7 Go annoncés pour 83,4 Go réels »).
  Les tests portent des noms en français et vérifient le comportement, pas
  l'implémentation.
- **Traductions** : aucun libellé en double dans le code ; une clé stable
  désigne le texte, les catalogues de `crates/core/locales` le traduisent.

## Ajouter une fonctionnalité

1. **Une nouvelle requête** : une variante dans `lapinux_core::ipc::Req`, son
   traitement dans le daemon, l'appel côté interface, et un test d'aller-retour
   JSON dans `ipc.rs` — c'est le seul contrat entre les deux processus, un champ
   renommé casserait l'écran sans autre signe.
2. **Une nouvelle boutique** : le contrat est dans `crates/stores` (compte,
   bibliothèque, installation, cloud, succès) ; la marque blanche va dans
   `assets/stores`, et la boutique se déclare dans `lapinux_core::stores`.
3. **Une nouvelle chaîne** : clé stable dans **tous** les catalogues, appel par
   `i18n::t` ou `t_args` ; les tests refusent une clé manquante.
4. **Un nouveau réglage** : champ dans la section `Config` concernée, valeur par
   défaut compatible avec les anciens fichiers, et une ligne dans
   [CONFIGURATION.md](CONFIGURATION.md).

Voir [TESTS.md](TESTS.md) pour les vérifications à lancer avant de livrer.

---

[Documentation](README.md) · [README du projet](../README.md)

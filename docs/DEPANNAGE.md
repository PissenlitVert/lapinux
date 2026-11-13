# Dépannage

Chaque section suit le même ordre : ce qu'on observe, ce qu'il faut vérifier,
puis le remède. Commencer par le diagnostic général.

## Diagnostic général

```bash
lapinux cli doctor      # environnement, permissions, Proton, GameScope, manettes
lapinux cli access      # accès réel à /dev/input
lapinux cli devices     # manettes et périphériques evdev
lapinux cli config      # configuration effective, après normalisation
```

Les quatre journaux sont dans `~/.local/share/lapinux/logs/` (`daemon.log`,
`ui.log`, `games.log`, `overlay.log`), tournés au-delà de 8 Mo ; **Réglages ›
Journaux** affiche les 400 dernières lignes sans quitter l'application. Le
daemon journalise notamment scan, lancement et installation. Ces fichiers
peuvent contenir des données personnelles : vérifiez-les et anonymisez-les avant
de les partager, sans divulguer de jetons.

Un daemon plus ancien que le binaire est signalé et **remplacé automatiquement
au lancement** : sinon il ignorerait les réglages récents et les effacerait en
réenregistrant la configuration. **Réglages › Système** affiche la version du
daemon qui tourne, le build du binaire sur le disque, et permet de le
redémarrer à la main. Un redémarrage interrompt les téléchargements en cours :
leurs fichiers reçus sont conservés et les tâches demandent une reprise. Masquer
la fenêtre ne revient pas à arrêter le daemon.

Un message « daemon occupé » signifie que les 64 requêtes simultanées sont
prises : une lecture Steam qui traîne occupe son fil jusqu'à son délai (30 s par
défaut). Ce n'est pas une panne ; réessayer suffit.

## Le daemon ne répond pas

| Vérification | Attendu |
| --- | --- |
| `$XDG_RUNTIME_DIR/lapinux/control.sock` existe | oui |
| permissions du socket | `0600`, propriétaire = utilisateur |
| `LAPINUX_LOG=debug ./target/release/lapinux --daemon` | le daemon dit ce qu'il fait |

Un délai de socket n'est **pas** un daemon mort : le client le distingue d'un
refus de connexion, précisément pour ne pas lancer un second processus par
dessus. Si le socket existe mais que rien ne répond, redémarrer le daemon depuis
**Réglages › Système**.

Le service utilisateur est dans `packaging/lapinux.service` :

```bash
systemctl --user daemon-reload
systemctl --user enable --now lapinux.service
journalctl --user -u lapinux -n 100
```

## Manettes et boutons

### Aucune manette détectée

1. `lapinux cli devices` : la manette apparaît-elle ?
2. `lapinux cli access` : `/dev/input` est-il lisible ?
3. Si l'accès est refusé, installer une fois la règle udev :

```bash
sudo cp packaging/99-lapinux-input.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input
```

Seuls le bouton **Guide** et le pad virtuel lisent `/dev/input` ; la navigation
au pad et `Shift+Tab` n'en dépendent pas.

### Le quick menu ne s'ouvre pas

- **Shift+Tab** est capturé par KWin pendant la session : le daemon charge un
  petit script au lancement et le retire à la fin. Tester `lapinux cli doctor`
  pour voir si le script est en place.
- Au pad, le raccourci par défaut est **Guide** (`overlay.pad_hotkey`). Un
  `L1+R1+Start` enregistré par une configuration antérieure est converti en
  `Guide` à la lecture.
- Pendant l'overlay, le service d'entrée **relâche le grab** des manettes : le
  panneau se navigue directement à la croix et aux boutons A/B.

### Steam Controller 2026

| Symptôme | Cause probable | Remède |
| --- | --- | --- |
| la page signale le Puck mais aucun pad | le dongle répond, la manette n'est pas allumée | allumer la manette ; Lapinux ne déduit pas sa présence de celle du Puck |
| l'interrupteur **Mode clavier/souris (lizard)** est grisé | règle udev absente | bouton **Installer la règle** dans la fiche, ou `packaging/99-steam-controller.rules` à la main |
| l'interrupteur reste grisé malgré la règle | la règle donne l'écriture au groupe `wheel` | sur une distribution dont le groupe admin est `sudo`, adapter la règle |
| la manette cesse de fonctionner après un arrêt du pont | `steam-puck-bridge` a été arrêté temporairement | redémarrer le service depuis la carte **Pont Steam Controller** |
| le pad émulé présente des axes qui n'existent pas | certains dongles 2,4 GHz les annoncent à tort | `input.pad_hide_dongle_axes` (la fiche masque ces tests) |

`lizard_mode=1` autorise l'émulation clavier/souris **au repos** ; cela ne
prouve pas que le pad est muet pendant qu'un programme l'utilise. Pour la
désactiver durablement :

```bash
sudo cp packaging/99-steam-controller.conf /etc/modprobe.d/
# puis redémarrer, ou recharger hid_steam si personne ne tient la manette
```

## Connexion Steam

| Réponse | Ce que ça veut dire | Quoi faire |
| --- | --- | --- |
| « mot de passe invalide », eresult **5** | pas forcément un mauvais mot de passe : Steam répond la même chose quand une autre session vient de s'ouvrir | attendre, réessayer ; ne pas conclure que le jeton est mort |
| refus par jeton ou par QR | Steam garde **une session par compte** | un refus est retenu cinq minutes au lieu d'être redemandé à chaque clic |
| « le défi QR n'a pas abouti » | défi de deux minutes expiré, ou canal CM tombé | relancer la connexion ; Lapinux renouvelle le défi dans un budget de cinq minutes |
| jeton marqué **expiré** | refus d'authentification | l'écran repasse en « non connecté » et propose **Renouveler par QR** |
| jeton **non vérifié** | panne réseau | ne condamne pas un jeton sain : réessayer plus tard |

Le jeton est écrit en `0600` dans `steam-session.json`, jamais le mot de passe.
Il vaut environ 200 jours ; Steam peut le révoquer avant terme si le compte se
reconnecte ailleurs ou depuis « déconnecter tous les appareils ». Un jeton
obtenu sur une autre installation peut être collé dans le champ prévu.

Une seule session sert **toutes** les lectures (fiche, succès, temps de jeu,
statistiques, catalogue, vérification du jeton) : c'est ce qui évite les
déconnexions en cascade. Les lectures par jeu sont gardées cinq minutes.

## Fenêtre de boutique

La page Boutiques est rendue par **WebKitGTK 6** (GTK 4) dans sa propre
fenêtre, pilotée par un petit hôte Python embarqué.

| Symptôme | Cause | Remède |
| --- | --- | --- |
| le panneau annonce une raison au lieu de la page | `python3-gobject` ou WebKitGTK 6 absent | installer les paquets ; le panneau propose d'ouvrir le site dans le navigateur |
| vidéos vertes (souvent NVIDIA) | sinks DMA-BUF/GL de WebKit | Lapinux force le sink logiciel ; le décodage passe sur le processeur, ce qui peut coûter un cœur |
| la page Steam coûte un cœur entier | boucle d'accueil de la vitrine | fermer la fenêtre de boutique quand elle ne sert pas |
| une offre Epic reste « en attente de caisse » | Epic demande un passage en caisse | la carte rejoint la file des offres à terminer, ouverte une par une ; la session WebKit garde les témoins de connexion |

## Jeu qui ne démarre pas

1. `lapinux cli launch <appid> --dry-run` affiche le plan : binaire, Proton,
   préfixe, variables. C'est le premier endroit à lire.
2. **DRM Steam** : Lapinux ne démarre pas le client Steam. Un DRM détecté est
   signalé ; un jeu qui exige un client ou un DRM non pris en charge peut ne
   pas fonctionner. Le pont Steamworks natif reste expérimental et limité.
3. **Proton partiel** : un build local incomplet (`toolmanifest.vdf` absent) est
   lancé par son propre script, pas par `umu-run`.
4. **Préfixe Steam** : un préfixe Steam existant ne passe **jamais** par umu —
   umu réécrit le fichier `version` et Steam considère alors le préfixe comme
   invalide.
5. **GE-Proton 11** : son script ne tourne pas sous le Python 3.9 du runtime
   sniper ; Lapinux utilise le Python de l'hôte (validé sur Bazzite/Fedora).
6. **GameScope absent** : l'interface le signale et le lancement reste direct,
   sans couche intermédiaire.

## Sauvegardes

- Un jeu de boutique a besoin de son **dossier de sauvegarde** renseigné : ni
  Epic ni GOG ne publient le chemin où le jeu écrit sous Proton.
- Un jeu Steam sans dossier configuré utilise son préfixe ; l'onglet
  **Sauvegardes** de la fiche liste les fichiers du cloud et leur état.
- Un envoi est refusé si `change_number` a bougé depuis le relevé de lancement :
  une autre machine a sauvegardé entre-temps, et **rien n'est envoyé**. Le
  journal le dit ; relire la fiche avant de forcer.
- Un fichier identique au cloud n'est pas renvoyé (Steam répond
  `DuplicateRequest`).
- La copie automatique à la fermeture peut être sautée quand le cloud garde déjà
  la partie (`general.saves_skip_cloud`, coché par défaut) : décocher pour
  garder aussi une copie locale.

## Images et vidéos de fiche

- Sans décodeur AVIF embarqué, les visuels AVIF et les affiches de bannières
  animées sont **écartés** plutôt qu'affichés vides. `ffmpeg` convertit les
  autres visuels quand il le faut.
- Les jaquettes de boutique sont demandées en 600 px au CDN : en pleine
  résolution, une rangée de tuiles saturait la connexion et les téléchargements
  expiraient.
- Si une jaquette ne correspond pas au jeu, **clic droit › Jaquette
  automatique** efface le choix manuel et les jaquettes téléchargées, puis
  repart de la source préférée.

## Réseau et quotas

- Steam répond `429` sur les pages publiques du profil : la pénalité protège les
  requêtes suivantes et la fiche se relit seule quelques dizaines de secondes
  plus tard.
- La première synchronisation du catalogue Steam prend ~40 s pour un millier de
  jeux, en arrière-plan, une seule fois.
- La bibliothèque d'une boutique est gardée quinze minutes ; **Relire** passe
  outre.
- Les sauvegardes Epic et GOG passent par des points d'entrée non documentés
  comme API publiques : ils peuvent changer sans préavis, et l'erreur s'affiche
  dans la carte au lieu d'être avalée.
- Une synchronisation cloud qui échoue ne bloque pas le lancement du jeu : le
  message va au journal.

## Signaler un problème

Joindre : la sortie de `lapinux cli doctor`, la version du binaire et du daemon
(**Réglages › Système**), la distribution et la session graphique (Wayland/X11,
KDE/GNOME), et les lignes pertinentes du journal concerné. Retirer les chemins
personnels n'est pas nécessaire — les journaux ne contiennent ni mot de passe ni
jeton.

---

[Documentation](README.md) · [README du projet](../README.md)

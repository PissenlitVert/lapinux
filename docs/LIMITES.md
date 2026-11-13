# Limites assumées

Ce que Lapinux ne fait pas, ou fait partiellement, et pourquoi. Ces limites sont des choix : contourner un DRM, réimplémenter Remote Play ou deviner un format fermé sortent du périmètre.

- Les jeux protégés par un **DRM Steam** sont détectés et signalés ; aucun
  contournement n'est garanti. Lapinux ne démarre pas le client Steam. Un pont
  Steamworks expérimental existe pour les jeux natifs x86-64, désactivé par
  défaut et limité à certaines interfaces ; voir [Installation](INSTALLATION.md#pont-steamworks-linux-natif).
  Des jeux nécessitant d'autres interfaces, un client ou un DRM peuvent échouer.
- Lapinux propose des fonctions Steam Input, Atelier et cloud sans lancer de
  client de boutique ; leur couverture reste partielle et dépend des jeux et
  services disponibles. Remote Play n'est pas implémenté. Le chat passe par la
  session CM : Steam ne rend qu'une fenêtre limitée et l'archive locale ne
  conserve que ce qui a déjà été reçu. Voir
  [RECHERCHE-HISTORIQUE-CHAT-STEAM.md](RECHERCHE-HISTORIQUE-CHAT-STEAM.md).
- La synchronisation cloud compare les données locales et distantes ; les
  conflits et erreurs doivent être traités avant d'écraser une sauvegarde.
  Les possibilités et garanties varient selon la boutique et le jeu.
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
- L'**installation Epic** utilise les manifestes et chunks du CDN directement,
  sans Epic Games Launcher. Les formats et services non documentés peuvent
  changer ; la compatibilité n'est pas garantie pour tous les jeux.
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
- GameScope peut manquer : l'UI le signale et le lancement reste direct, sans
  couche intermédiaire.
- Le runtime sniper de Steam embarque Python 3.9, trop ancien pour le script
  `proton` de GE-Proton 11 ; Lapinux exécute donc `proton` avec le Python de
  l'hôte (validé sur Bazzite/Fedora).

---

[Documentation](README.md) · [README du projet](../README.md)

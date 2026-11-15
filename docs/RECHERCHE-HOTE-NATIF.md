# Sonde de l'hôte natif Steam

`lapinux-probe` charge la bibliothèque d'un client Steam installé et mesure ce que ses interfaces rendent sans client lancé. Ces notes sont un carnet de recherche : elles documentent des mesures datées, pas des fonctionnalités stables.

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

## Pourquoi le type 1 tombait, et ce qui l'a réparé

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

## Le mur de l'authentification, nommé

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

## L'authentification, par notre propre client CM

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

---

[Documentation](README.md) · [README du projet](../README.md)

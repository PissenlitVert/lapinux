# L'historique complet d'une discussion Steam est-il récupérable ?

**Verdict : non, seulement en partie — limite exacte : le protocole ne rend
qu'une fenêtre glissante d'environ deux semaines. Les messages plus anciens,
supprimés ou jamais reçus par cet appareil ne sont pas récupérables par
`FriendMessages.GetRecentMessages#1`. La totalité n'est possible que si
l'utilisateur fournit déjà une archive ou un export antérieur complet ; sans
cela, la réponse est non.**

Cette page répond à la question « peut-on obtenir **toute** la discussion avec
un ami Steam ? », sans lancer le client Steam, sans capture d'écran, sans
envoyer de message et sans souris ni clavier. Elle distingue ce que le
protocole expose réellement de ce que Lapinux conserve localement.

## Ce qui a été vérifié, et comment

- Le contrat du protocole est lu dans le code de Lapinux et dans les protobufs
  Steam : `FriendMessages.GetRecentMessages#1` pour une conversation privée,
  `ChatRoom.GetMessageHistory#1` pour un groupe.
- Les tests hors ligne de Lapinux vérifient l'encodage de la requête, les
  curseurs de pagination, le sens des auteurs et la déduplication. La commande
  est en fin de page.
- La sonde `scripts/steam-chat-history-audit.sh` ne contacte aucun service :
  elle exécute ces tests puis compte les messages de l'archive locale sans
  afficher leur texte ni l'identité des contacts.
- **Aucune requête authentifiée n'a été faite sur votre compte** : elle aurait
  utilisé votre session et vos messages. Les points qui dépendent d'un compte
  réel sont signalés explicitement plus bas.

## 1. Conversation privée : `FriendMessages.GetRecentMessages#1`

C'est la méthode utilisée par Lapinux
(`crates/steam/src/social.rs:59`), appelée sur le CM avec la session Steam.
Le même contrat est publié en Web API sous
`IFriendMessagesService/GetRecentMessages/v1/`. Cette variante demande une clé
(`key`) selon la documentation xPaw ; ce n'est pas une archive distincte, et
Lapinux ne s'en sert pas. Aucune clé n'a été utilisée pour cette recherche.

### Requête

| Champ | Type | Nom | Ce que fait Lapinux |
| --- | --- | --- | --- |
| 1 | `fixed64` | `steamid1` | SteamID64 du compte connecté |
| 2 | `fixed64` | `steamid2` | SteamID64 de l'ami |
| 3 | `uint32` | `count` | 100 messages par page |
| 4 | `bool` | `most_recent_conversation` | omis ; `true` ne prend qu'un bloc d'environ 5 minutes |
| 5 | `fixed32` | `rtime32_start_time` | 1, pour demander tout ce que Steam conserve encore |
| 6 | `bool` | `bbcode_format` | omis ; Lapinux garde le texte brut |
| 7 | `uint32` | `start_ordinal` | non utilisé par Lapinux |
| 8 | `uint32` | `time_last` | curseur de la page plus ancienne |
| 9 | `uint32` | `ordinal_last` | départage deux messages de la même seconde |

Le code exact est dans `crates/steam/src/social.rs:1956-1969`. Les tests
`l_historique_demande_les_deux_comptes_et_une_borne` et
`l_historique_demande_la_page_precedente_avec_son_curseur` vérifient les champs
1, 2, 3, 5, puis 8 et 9.

`rtime32_start_time = 0` laisse Steam appliquer son propre point de départ
récent. Une valeur non nulle demande depuis ce point, mais **ne peut pas
dépasser la rétention serveur** : le paramètre déplace la borne demandée, pas
la durée de conservation.

### Réponse

| Champ | Type | Nom | Contenu |
| --- | --- | --- | --- |
| 1 | message répété | `messages` | messages de la page |
| 1.1 | `uint32` | `accountid` | auteur du message |
| 1.2 | `uint32` | `timestamp` | seconde Unix |
| 1.3 | `string` | `message` | texte, éventuellement en BBCode |
| 1.4 | `uint32` | `ordinal` | identifiant dans la seconde |
| 1.5 | message répété | `reactions` | réactions, non lues par Lapinux |
| 4 | `bool` | `more_available` | il reste des messages avant cette page |

Le parseur de Lapinux lit les champs 1 à 4 et `more_available`
(`crates/steam/src/social.rs:2286-2340`). Il ignore le champ 5
(`reactions`) : le protocole les rend, mais Lapinux ne les archive pas et ne
les affiche pas.

### Pagination réelle

Steam renvoie les messages du plus récent au plus ancien. Lapinux :

1. demande une première page de 100 messages avec `rtime32_start_time = 1` ;
2. remet la page dans l'ordre chronologique ;
3. prend le message le plus ancien de la page et garde son couple
   `(timestamp, ordinal)` ;
4. redemande une page avec `time_last` et `ordinal_last` ;
5. s'arrête quand `more_available` est faux.

C'est ce que vérifie `les_pages_d_historique_forment_une_chronologie_unique`
(`crates/steam/src/social.rs:3244`). Le test
`l_historique_demande_la_page_precedente_avec_son_curseur` prouve que les
champs 8 et 9 sont bien envoyés avec le couple de la page précédente.

**Conséquence :** `more_available` n'est pas un booléen décoratif ; il permet
de parcourir toute la fenêtre conservée. Mais aucun curseur ne permet de
franchir la rétention serveur.

## 2. Groupes : `ChatRoom.GetMessageHistory#1`

Une discussion de groupe n'utilise pas `FriendMessages`. Le service
`ChatRoom` expose `GetMessageHistory` avec les champs suivants
(protobuf Steam `steammessages_chat.steamclient.proto`) :

| Champ | Type | Nom |
| --- | --- | --- |
| 1 | `uint64` | `chat_group_id` |
| 2 | `uint64` | `chat_id` (le canal) |
| 3 | `uint32` | `last_time` |
| 4 | `uint32` | `last_ordinal` |
| 5 | `uint32` | `start_time` |
| 6 | `uint32` | `start_ordinal` |
| 7 | `uint32` | `max_count` |

La réponse contient `messages` (champ 1) et `more_available` (champ 4). Chaque
message porte `sender`, `server_timestamp`, `message`, `ordinal`,
`server_message`, `deleted` et `reactions`. Le champ `deleted` existe donc
pour les groupes, contrairement aux messages privés.

Node-steam-user documente le même appel sous `getChatMessageHistory`, avec
`maxCount` par défaut à 100. L'accès dépend de l'appartenance au groupe et de
la permission `can_view_history` (`CChatRoleActions`, champ 8). Lapinux
n'implémente pas l'historique de groupe ; cette partie est donc une limite du
produit, pas seulement du protocole.

Valve annonce pour les groupes la même fenêtre que pour les messages privés :
« Group chats also keep up to two weeks of chat history ». Une suppression de
salon détruit son historique ; Node-steam-user le dit explicitement :
« All of the chat room's chat history will be permanently lost. »

## 3. Les limites, une par une

### Fenêtre temporelle

L'annonce officielle de Valve sur le nouveau chat dit :

> Can now scroll back to load up to two weeks of chat history
>
> Group chats also keep up to two weeks of chat history

C'est la seule durée documentée publiquement. Le code de Lapinux la reprend
dans le libellé `history_limit` (« environ 14 jours »,
`crates/core/locales/fr.toml:1953`) et dans l'état
`SteamChatHistoryState` (`crates/core/src/social.rs:385-417`). Le protocole
n'expose aucun paramètre pour allonger cette conservation.

### Nombre maximal

- `count` est un plafond **par requête** ; la documentation xPaw dit
  « If non-zero, cap the number of recent messages to return » sans publier de
  maximum serveur.
- Lapinux demande 100 messages par page (`HISTORY_MESSAGES`,
  `crates/steam/src/social.rs:63`).
- Node-steam-user utilise aussi 100 par défaut pour `getFriendMessageHistory`.
- Le nombre total n'est pas plafonné par un `count` global : il est borné par
  la fenêtre de deux semaines et par la pagination.
- Lapinux garde au plus 5 000 messages par conversation en mémoire
  (`CHAT_MESSAGES`, `crates/steam/src/social.rs:69`) et 5 000 par conversation
  dans son archive (`CHAT_ARCHIVE_MAX`, `crates/daemon/src/social.rs:38`).
  Ce sont des limites de Lapinux, pas de Steam.

Le plafond exact que le serveur accepte au-delà de 100 n'est pas publié et
n'a pas été mesuré sans session authentifiée. Cela ne change pas la limite
temporelle : une page plus grosse ne rend pas les messages expirés.

### Messages hors ligne

Le service expose `FriendMessages.GetActiveMessageSessions#1` pour les
sessions récentes : `accountid_friend`, `last_message`, `last_view`,
`unread_message_count` et `notices`. La documentation xPaw le décrit comme
« Get information about recent offline messages and chats ». Cela donne des
**métadonnées** et le compteur de non-lus, pas un journal de contenu séparé.

Le contenu des messages hors ligne se lit par la même fenêtre
`GetRecentMessages`. `FriendMessages.AckMessage#1` (et
`MarkOfflineMessagesRead` côté Web API) marque comme lu ; aucune de ces
méthodes ne rend un message plus ancien que la fenêtre.

Conséquence : un message hors ligne encore conservé par Steam apparaît dans
l'historique récent ; un message hors ligne plus vieux que la rétention n'a
pas de méthode dédiée pour être récupéré.

### Messages envoyés depuis un autre appareil

La requête ne contient ni identifiant de session ni identifiant d'appareil :
elle ne dépend que du couple de comptes. Les messages envoyés depuis une autre
session du même compte sont donc dans l'historique serveur s'ils sont encore
dans la fenêtre. La notification `IncomingMessage` porte `local_echo`
(champ 7) pour signaler l'écho d'un message envoyé ailleurs ; cela ne crée pas
un historique par appareil.

### Messages supprimés

Le message privé (`CFriendMessages_GetRecentMessages_Response.FriendMessage`)
n'a **aucun champ `deleted`**. La méthode ne permet ni de lister les messages
supprimés, ni de distinguer un message supprimé d'un message jamais envoyé.
Si le texte a été supprimé avant la lecture, le protocole ne le rend pas ;
seule une copie locale antérieure peut le contenir.

Pour un groupe, le message possède bien `deleted`. Lapinux pourrait savoir
qu'un message a été supprimé, mais le texte d'origine n'est pas dans la
réponse : le champ sert à afficher l'état, pas à restaurer le contenu.

### Médias, réactions et appels

La réponse d'historique est un texte (`message`) et, pour les groupes, des
réactions. Elle ne contient pas les fichiers image/vidéo eux-mêmes : une image
partagée n'est présente que sous la forme du lien ou du BBCode que le message
contenait. Les réactions sont dans le protobuf mais ignorées par le parseur de
Lapinux. L'historique des appels vocaux n'est pas un message et n'est pas
exposé par ces méthodes.

### La page « données de compte » de Steam

La page `https://help.steampowered.com/en/accountdata/GetFriendMessagesLog`
existe et est indexée comme « Friend Chat Messages » dans le relevé
SteamTracking-GDPR. Elle **exige une connexion** à l'assistance Steam : sans
session, elle ne rend qu'une page vide. La politique de confidentialité de
Valve confirme que l'accès aux données passe par une connexion au compte et
par « My Account → View Account Data ».

Je n'ai pas pu vérifier son contenu sans votre compte. Les témoignages publics
sont contradictoires : un message de 2019 affirme qu'elle contient « any chat
message », mais des observations de 2018, 2021 et 2022 rapportent une limite
d'environ deux semaines, et un utilisateur de 2021 y voit la même coupure que
dans le client. Valve ne la documente pas comme une archive complète. Elle ne
permet donc pas de garantir la totalité ; au mieux, elle peut confirmer ou
infirmer la fenêtre sur votre compte, ce qui demande votre action.

### Le cache local et l'archive Lapinux

Le cache du client Steam ou de l'application mobile peut contenir des messages
déjà reçus, mais ce n'est pas une archive serveur : il ne contient que ce que
cet appareil a vu et n'est pas interrogeable par le protocole. Lapinux fait le
même travail avec `steam-chats.json` : il garde ce qui a été chargé ou importé.
L'archive observée sur cette machine contient **une conversation, 15 messages,
tous sortants et aucun entrant** ; elle ne démontre donc pas un historique
complet. L'import JSON (`crates/daemon/src/social.rs:102-210`) peut ajouter un
ancien export, mais il ne peut pas inventer les messages absents.

## 4. Ce qui est récupérable, et ce qui manque

| Élément | Par le protocole | Après la fenêtre | Depuis un autre appareil | Supprimé |
| --- | --- | --- | --- | --- |
| Texte d'un message privé | oui, dans la fenêtre | non | oui, si dans la fenêtre | non |
| Texte d'un message de groupe | oui, avec `GetMessageHistory` | non | oui, si dans la fenêtre | état `deleted`, pas le texte |
| Message hors ligne | via la même fenêtre | non | sans objet | non |
| Accusé de lecture / non-lus | `GetActiveMessageSessions`, `AckMessage` | métadonnées seulement | partagé par compte | sans objet |
| Réactions | oui dans le protobuf | non | oui, si dans la fenêtre | non |
| Média uploadé | lien/BBCode éventuel | non | oui, si dans la fenêtre | non |
| Historique des appels vocaux | non exposé | non | non | non |
| Archive complète | non | non | non | non |

La totalité n'est donc **pas reconstructible par les appels** : il manque au
minimum tout ce qui précède la rétention, les messages supprimés avant
lecture et tout ce qui n'a jamais été chargé sur cet appareil. Une archive
locale complète, si l'utilisateur en possède une, est le seul complément
capable de restituer le passé ; elle doit être fournie et importée.

## 5. Sans identifiants, avec identifiants, avec export

| Moyen | Ce qu'il faut | Ce qu'on obtient |
| --- | --- | --- |
| Tests et audit hors ligne | rien | le contrat, la pagination et le contenu déjà archivé ; aucun message serveur |
| `FriendMessages.GetRecentMessages#1` | une session Steam valide (jeton de refresh ou connexion) | la fenêtre récente, paginée jusqu'à `more_available = false` |
| `IFriendMessagesService/GetRecentMessages/v1/` | une clé Web API | le même contrat ; aucun élargissement de la fenêtre n'est documenté |
| `ChatRoom.GetMessageHistory#1` | une session et l'appartenance au groupe | la fenêtre récente du canal, avec `deleted` |
| Page `GetFriendMessagesLog` | une connexion au compte dans un navigateur | un journal récent, non documenté comme complet |
| Import d'un ancien export | le fichier fourni par l'utilisateur | les messages de l'export, conservés localement |
| Client Steam / cache mobile | le client et son cache | ce que l'appareil a déjà vu ; hors protocole et non complet |

Lapinux n'utilise ni mot de passe ni client Steam pour l'historique : il
reprend un jeton de refresh déjà enregistré, en `0600`
(`crates/steam/src/session.rs:8-20`), et le passe à
`LoginBuilder::with_refresh_token` (`crates/steam/src/social.rs:972`). Un
jeton vide est refusé (`crates/steam/src/social.rs:631`). Sans jeton
enregistré, l'utilisateur doit se connecter lui-même (mot de passe/Steam Guard
ou QR) ; ce n'est pas une action que la sonde fait à sa place. Le jeton et le
mot de passe ne doivent jamais être collés dans un document ou un journal.

La page de données de compte est un moyen **hors protocole** : elle demande une
connexion navigateur et n'est pas reproductible par la sonde. L'import JSON est
le seul chemin prévu par Lapinux pour du contenu plus ancien que la fenêtre.

## 6. Preuves exécutées

```bash
./scripts/steam-chat-history-audit.sh
```

Sortie observée le 10 novembre 2026 :

```text
running 5 tests
test social::tests::l_historique_demande_les_deux_comptes_et_une_borne ... ok
test social::tests::l_historique_steam_rend_les_deux_auteurs_dans_l_ordre ... ok
test social::tests::l_historique_demande_la_page_precedente_avec_son_curseur ... ok
test social::tests::l_historique_ne_double_pas_les_messages_de_la_session ... ok
test social::tests::les_pages_d_historique_forment_une_chronologie_unique ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 310 filtered out
Conversation 0 : 15 message(s), 15 sortant(s), 0 entrant(s),
  du 2026-11-03T17:30:49+00:00 au 2026-11-10T04:48:13+00:00.
Total : 15 message(s), 15 sortant(s), 0 entrant(s).
Messages de plus de 14 jours encore présents localement : 0.
```

La sonde ne fait aucun appel réseau et n'affiche ni le texte des messages ni
les identifiants des contacts. Elle vérifie le code réel de Lapinux, pas une
copie de son contrat.

## 7. Points qui restent indécidables sans identifiants

- Le plafond serveur exact par page au-delà de 100 : la documentation ne le
  publie pas, et une mesure demanderait une session authentifiée.
- Le comportement exact de Steam pour un message privé supprimé avant la
  lecture : le schéma n'expose aucun champ permettant de l'observer.
- Le contenu et la portée réelle de `GetFriendMessagesLog` sur votre compte :
  la page exige votre connexion et les témoignages publics se contredisent.
- La rétention effective au moment de la lecture : elle peut être plus courte
  que deux semaines, jamais plus longue d'après la documentation.

Aucun de ces points ne permet de promettre l'historique complet : le schéma ne
contient ni Archive complète, ni paramètre « tout l'historique », et la seule
fenêtre documentée est de deux semaines.

## Conclusion

**Non, seulement en partie — limite exacte : `FriendMessages.GetRecentMessages#1`
ne rend que la fenêtre de chat que Steam conserve, documentée comme allant
jusqu'à deux semaines. Les groupes suivent la même limite avec
`ChatRoom.GetMessageHistory#1`. Au-delà, les messages supprimés ou jamais reçus
par cet appareil ne sont pas récupérables par le protocole. La totalité n'est
possible que si l'utilisateur fournit une archive ou un export antérieur
complet et l'importe ; sans cette pièce, non.**

## Sources

Sources primaires :

- Annonce officielle Steam, *New friends list, voice, and chat features* :
  <https://steamcommunity.com/updates/chatupdate> — « up to two weeks of chat
  history » et « Group chats also keep up to two weeks ».
- Protobuf Steam, `steammessages_friendmessages.steamclient.proto` :
  <https://raw.githubusercontent.com/SteamDatabase/Protobufs/master/steam/steammessages_friendmessages.steamclient.proto>
- Protobuf Steam, `steammessages_chat.steamclient.proto` :
  <https://raw.githubusercontent.com/SteamDatabase/Protobufs/master/steam/steammessages_chat.steamclient.proto>
- Politique de confidentialité Valve (accès aux données par connexion au
  compte) : <https://store.steampowered.com/privacy_agreement>

Documentation et implémentations de référence :

- xPaw, `IFriendMessagesService` (signification de `count`,
  `most_recent_conversation`, `rtime32_start_time`, `time_last`,
  `ordinal_last`, `GetActiveMessageSessions`) :
  <https://steamapi.xpaw.me/IFriendMessagesService>
- Node-steam-user, `SteamChatRoomClient` (`getFriendMessageHistory`,
  `getChatMessageHistory`, défaut 100, `deleted`, `more_available`,
  suppression d'un salon) :
  <https://github.com/DoctorMcKay/node-steam-user/wiki/SteamChatRoomClient>
- Code source Node-steam-user, `components/chatroom.js` (appels exacts
  `FriendMessages.GetRecentMessages#1` et `ChatRoom.GetMessageHistory#1`) :
  <https://raw.githubusercontent.com/DoctorMcKay/node-steam-user/master/components/chatroom.js>
- Relevé communautaire des pages de données de compte :
  <https://github.com/SteamDatabase/SteamTracking-GDPR/blob/master/steam_accountdata.md>

Sources secondaires, utilisées uniquement pour la page de données de compte et
clairement non normatives :

- Discussion Steam Community de 2021 sur la coupure à deux semaines :
  <https://steamcommunity.com/discussions/forum/1/3094515496027705588>
- Discussion Steam Community de 2022 sur la même page :
  <https://steamcommunity.com/discussions/forum/0/3283695222735724414>
- Discussion Steam Community de 2018-2019, avec des avis contradictoires :
  <https://steamcommunity.com/discussions/forum/1/1734333281940992839>

Preuves dans le dépôt :

- `crates/steam/src/social.rs:59-69` : nom de la méthode, 100 messages par
  page, borne basse 1.
- `crates/steam/src/social.rs:1956-1969` : corps exact de la requête.
- `crates/steam/src/social.rs:2286-2340` : lecture de la réponse et de
  `more_available`.
- `crates/steam/src/social.rs:3183-3355` : tests du contrat et de la
  pagination.
- `crates/core/src/social.rs:385-417` : fenêtre récente, curseur et mention
  « Steam ne fournit pas d'historique complet ».
- `crates/daemon/src/social.rs:37-105` : archive locale et plafond de 5 000
  messages par conversation.
- `crates/daemon/src/social.rs:102-210` : import d'un export JSON.
- `crates/steam/src/session.rs:8-43` : jeton de refresh, jamais le mot de
  passe.
- `scripts/steam-chat-history-audit.sh` : sonde hors ligne reproductible.

---

[Documentation](README.md) · [README du projet](../README.md)

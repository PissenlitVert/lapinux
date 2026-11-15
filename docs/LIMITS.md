# Accepted limits

What Lapinux does not do, or does only partly, and why. These limits are
choices: bypassing DRM, reimplementing Remote Play or guessing a closed format
are out of scope.

- Games protected by **Steam DRM** are detected and reported; no bypass is
  guaranteed. Lapinux does not start the Steam client. An experimental
  Steamworks bridge exists for native x86-64 games, disabled by default and
  limited to some interfaces; see
  [Installation](INSTALLATION.md#native-steamworks-bridge). Games needing other
  interfaces, a client or a DRM may fail.
- Lapinux offers Steam Input, Workshop and cloud features without starting a
  store client; their coverage remains partial and depends on the games and
  services available. Remote Play is not implemented. Chat goes through the CM
  session: Steam only returns a limited window and the local archive keeps only
  what was already received. See
  [RECHERCHE-HISTORIQUE-CHAT-STEAM.md](RECHERCHE-HISTORIQUE-CHAT-STEAM.md).
- Cloud sync compares local and remote data; conflicts and errors must be
  handled before overwriting a save. Capabilities and guarantees vary by store
  and game.
- Achievements are read through the **Steam session**, without a Web API key:
  `Player.GetGameAchievements` publishes the game's schema (name, description,
  icon, hidden achievement, internal identifier) and each achievement's global
  rarity (`player_percent_unlocked`), `Player.GetUserAchievements` the account
  state, matched by internal identifier. A read longer than forty-five seconds
  is abandoned with a message and a "Retry" link, and only icons actually shown
  are downloaded, four at a time. **Steam Details** likewise reads playtime
  through `Player.GetOwnedGames` (total, two weeks, per system, last played) —
  with, in field 9, the family group identifier
  (`FamilyGroups.GetFamilyGroupForUser`): without it Steam omits family-shared
  games, which therefore displayed "no playtime" even though the account had
  played them — the level through `Player.GetGameBadgeLevels` and the stats
  published by `Player.GetUserStats`, whose binary schema provides the labels.
  The game embed also shows hours played by the account. No Web API key is
  requested: the saved session is enough, and the only remaining HTTP read is
  `GetNumberOfCurrentPlayers`, a public address that wants neither key nor
  token. The store's review page (`store.steampowered.com/appreviews/…`) now
  redirects to the storefront for a client without cookies: only the public
  `appdetails` page is read, and its review count
  (`recommendations.total`) replaces the community summary. Finally, for games
  without a Linux version Steam publishes a `linux_requirements` reduced to its
  title, followed by an empty list: the minimum configuration retained is then
  the Windows one, which is what Proton implements.
- **Additional content** is listed in full: the selection panel shows everything
  Steam publishes for the game, owned or not — the PICS sheet names them all and
  the account catalog says which are on the account, hence a count such as
  "4 of 9 owned". The list is split into two sections, **Owned** first then
  **Not owned**, each counted; what the account owns is checked by default, so
  the download button directly announces the weight to install. A not-owned
  item keeps its place, banner and weight, with a greyed checkbox: it cannot be
  downloaded. Their **weight** is read from the **parent game's** depots, those
  carrying the `dlcappid` field: a content sheet does not publish its own, which
  is what displayed "unknown weight" everywhere. An owned item without its own
  depot is reported as **included in the game**: its files travel with the base
  game's, as for most outfits and accessories. Its **banner** (`header.jpg`,
  path `store_item_assets` when PICS provides it, served by the `akamai` host
  which answers directly where `cloudflare` redirects) is downloaded once then
  kept with the other artwork; otherwise the store's `appdetails` sheet is
  queried — once per item, its call limit is low. No artwork exists for an
  unlisted item (future release): the thumbnail stays neutral. The panel also
  says which are **already on disk** and which are missing: Steam installs each
  item as a separate application, with its `appmanifest_<appid>.acf` manifest,
  and Lapinux writes the same for those it installs itself. An item already
  there is no longer added to the announced weight: installation checks without
  downloading it again. The depots of a checked item join the game's (including
  `dlcappid` depots) and install into its folder; an item finally not owned
  refuses its depot key and is skipped without interrupting the rest. An item
  bought later is therefore added to an installed game from its sheet:
  installation first checks existing files and downloads only what is missing.
- A Steam account and 2FA are required to log in; only owned content is
  accessible.
- **Epic installs** use CDN manifests and chunks directly, without the Epic
  Games Launcher. Undocumented formats and services may change; compatibility is
  not guaranteed for every game.
- **itch.io** has neither cloud saves nor achievements: its API exposes neither,
  and the game sheet therefore does not show an empty card for them.
- **Epic and GOG saves** go through endpoints that free launchers documented,
  not public APIs: they may change without notice, and the error then appears in
  the card instead of being swallowed. The local folder must be set in the game
  settings: Epic publishes neither its file sizes nor the path the game uses.
  Achievements for both stores are read, but **unlocking** one remains the game's
  job (EOS overlay for Epic, Galaxy client/overlay for GOG), as for Steam.
- The first catalog sync expands every licence package through PICS then reads
  names: expect ~40 s for a thousand games, in the background, once — afterwards
  only the cache is read.
- GameScope may be missing: the UI reports it and launching stays direct, without
  an intermediate layer.
- Steam's sniper runtime embeds Python 3.9, too old for the `proton` script of
  GE-Proton 11; Lapinux therefore runs `proton` with the host's Python (validated
  on Bazzite/Fedora).

---

[Documentation](README.md) · [Project README](../README.md)

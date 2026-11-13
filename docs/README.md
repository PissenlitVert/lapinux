# Documentation de Lapinux

Cette documentation est séparée par usage : le [README](../README.md) présente le
projet et renvoie ici.

| Page | Pour qui | Contenu |
| --- | --- | --- |
| [GUIDE.md](GUIDE.md) | qui débute ou publie | l'ordre complet : installer, développer, tester, construire l'AppImage, publier |
| [INSTALLATION.md](INSTALLATION.md) | qui installe | build, dépendances, service utilisateur, règles udev, stockage, Steam Controller |
| [CONFIGURATION.md](CONFIGURATION.md) | qui sauvegarde ou scénarise | chemins, variables `LAPINUX_*`, clés de `config.toml` |
| [FONCTIONNALITES.md](FONCTIONNALITES.md) | qui veut savoir ce que fait Lapinux | référence complète, par domaine |
| [DEPANNAGE.md](DEPANNAGE.md) | qui a un problème | symptôme → vérification → remède |
| [LIMITES.md](LIMITES.md) | qui évalue le projet | ce qui n'est pas fait, et pourquoi |
| [RECHERCHE-HISTORIQUE-CHAT-STEAM.md](RECHERCHE-HISTORIQUE-CHAT-STEAM.md) | qui cherche l'historique d'un chat | méthodes Steam, fenêtre de deux semaines, preuves et sonde hors ligne |
| [ARCHITECTURE.md](ARCHITECTURE.md) | qui contribue | crates, IPC, persistance, conventions, où ajouter du code |
| [TESTS.md](TESTS.md) | qui contribue | suite de tests, smoke test, sondes |
| [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md) | qui prépare une diffusion | risques, nettoyage prudent et vérifications restantes |
| [RECHERCHE-HOTE-NATIF.md](RECHERCHE-HOTE-NATIF.md) | qui cherche | carnet de mesures sur `lapinux-probe` |

## Autres notices

- `crates/core/locales/README.md` — ajouter un libellé ou une langue.
- `configurations/README.md` — catalogue communautaire de correctifs,
  traductions et mods déclaratifs.

## Conventions

- **Français** pour les commentaires, les noms de tests et cette documentation ;
  l'interface est traduite par catalogues (`fr`, `en`).
- **Expliquer le pourquoi**, pas seulement le quoi : une mesure, un symptôme
  observé ou une contrainte de protocole vaut mieux qu'un adjectif.
- **Une page par question** : si un paragraphe ne répond plus au titre de la
  page, il change de page.
- Les commandes sont copiables telles quelles depuis la racine du dépôt.

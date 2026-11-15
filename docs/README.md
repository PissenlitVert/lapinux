# Lapinux documentation

This documentation is split by use: the [project README](../README.md)
introduces Lapinux and points here.

| Page | For | Contents |
| --- | --- | --- |
| [GUIDE.md](GUIDE.md) | getting started, or publishing | the full order: install, develop, test, build the AppImage, publish |
| [INSTALLATION.md](INSTALLATION.md) | installing | build, dependencies, user service, udev rules, storage, Steam Controller |
| [CONFIGURATION.md](CONFIGURATION.md) | backing up or scripting | paths, `LAPINUX_*` variables, `config.toml` keys |
| [FEATURES.md](FEATURES.md) | learning what Lapinux does | complete reference, by area |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | hitting a problem | symptom → check → fix |
| [LIMITS.md](LIMITS.md) | evaluating the project | what is not done, and why |
| [RECHERCHE-HISTORIQUE-CHAT-STEAM.md](RECHERCHE-HISTORIQUE-CHAT-STEAM.md) | looking for a chat history | Steam methods, the two-week window, evidence and the offline probe |
| [ARCHITECTURE.md](ARCHITECTURE.md) | contributing | crates, IPC, persistence, conventions, where to add code |
| [TESTS.md](TESTS.md) | contributing | test suite, smoke test, probes |
| [AUDIT-PUBLICATION.md](AUDIT-PUBLICATION.md) | preparing a release | risks, careful cleanup and remaining checks |
| [RECHERCHE-HOTE-NATIF.md](RECHERCHE-HOTE-NATIF.md) | researching | measurement notes on `lapinux-probe` |

## Other notices

- `crates/core/locales/README.md` — adding a string or a language.
- `configurations/README.md` — the community catalog of fixes, translations
  and declarative mods.

## Conventions

- **English** for these public pages; code comments and test names stay in
  French. The interface is translated by catalogs (`fr`, `en`).
- **Explain the why**, not only the what: a measurement, an observed symptom or
  a protocol constraint beats an adjective.
- **One page per question**: if a paragraph no longer answers the page title,
  it belongs on another page.
- Commands can be copied as-is from the repository root.

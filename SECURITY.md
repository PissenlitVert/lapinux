# Security policy

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | yes |
| older | no |

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Use GitHub's
private reporting: **Security → Advisories → Report a vulnerability** on
<https://github.com/PissenlitVert/lapinux/security/advisories/new>.

Include the version (`lapinux --version`), the distribution, what an attacker
can do, and the smallest reproduction you have. A first answer within seven
days, and a fix or a mitigation plan within thirty days, is the target.

## Scope

In scope:

- the daemon IPC socket and its permissions;
- credential and token storage for Steam, Epic, GOG and itch.io;
- downloaded files: catalogue entries, plugins, themes, palettes, AppImage
  updates;
- the native plugin host and its ABI boundary;
- privilege boundaries around launching games and writing to prefixes.

Out of scope:

- DRM or anti-cheat bypass: Lapinux reports those limits and does not bypass
  them;
- vulnerabilities in games, Proton builds, drivers or store clients;
- a compromised local account or a machine where an attacker can already
  replace `~/.local/bin/lapinux`.

## What we never ask for

Never paste a Steam password, a Steam Guard code, a refresh token or a
SteamGridDB key into an issue. Redact them first.

# MangoHud corrigé pour Lapinux

Lapinux embarque sa propre couche MangoHud (Vulkan), construite depuis la
version **0.8.4** avec un correctif sur la position du HUD **en ligne**.

## Pourquoi

Dans MangoHud 0.8, `position_layer()` place une ligne à partir de
`window_size`, une largeur calculée à la trame précédente
(`GetContentRegionAvail - colonnes × 64`) qui devient négative quand la
fenêtre s'ajuste à son contenu :

- **à droite** (`top-right`, `middle-right`, `bottom-right`), la fenêtre part
  hors de l'écran et le HUD disparaît ;
- **au centre** (`top-center`, `bottom-center`), un `table_columns × 64` est
  retranché en plus, et `offset_x` n'est même pas appliqué : la ligne dérive
  vers la droite sans moyen de la recentrer.

Ce sont les tickets [#1403](https://github.com/flightlessmango/MangoHud/issues/1403)
et [#1746](https://github.com/flightlessmango/MangoHud/issues/1746), toujours
ouverts.

## Le correctif

`lapinux-inline-anchors.patch` (sur `src/overlay.cpp`) :

1. l'ancrage se calcule sur la **taille réelle** de la fenêtre du HUD
   (`ImGui::FindWindowByName("Main")->SizeFull`), celle de la dernière trame ;
2. la largeur « auto » est écrite `0` (ajustement au contenu) au lieu d'une
   valeur négative ;
3. les deux ancrages du centre utilisent la même formule que les autres et
   respectent `offset_x`.

Résultat : les **neuf ancrages** tombent juste, à une trame près après un
changement de largeur du HUD. Le calcul est vérifié par un banc ImGui hors
écran (fenêtre 605 × 40 sur 1920 × 1080 : 10 / 657 / 1305 en x, 10 / 510-520 /
1030 en y).

## Construire et installer

```sh
./packaging/mangohud/build.sh
```

Le script télécharge la version 0.8.4, applique le patch, compile dans
l'image `fedora-toolbox:latest` (podman) et installe
`~/.local/share/lapinux/mangohud/libMangoHud.so` (variable `LAPINUX_DATA` pour
un autre dossier). La bibliothèque ne dépend que de `libxkbcommon`,
`libwayland-client`, `libm`, `libgcc_s` et `libc` ; `libstdc++` est liée
statiquement.

## Activation

Au lancement d'un jeu, le daemon écrit le manifeste
`lapinux-mangohud.json` à côté de la bibliothèque et ajoute à la commande :

| variable | rôle |
| --- | --- |
| `VK_ADD_IMPLICIT_LAYER_PATH` | cherche la couche dans le dossier de Lapinux |
| `VK_LOADER_LAYERS_DISABLE` | coupe `VK_LAYER_MANGOHUD_overlay_x86_64` (celle du système) |

`MANGOHUD=1` reste la clé d'activation, comme avant : la couche s'appelle
`VK_LAYER_LAPINUX_MANGOHUD_overlay_x86_64` et garde
`enable_environment: MANGOHUD=1`. Sans la bibliothèque, rien n'est déclaré et
MangoHud du système reste seul en jeu.

Seule la couche 64 bits est fournie : les jeux 32 bits gardent celle du
système.

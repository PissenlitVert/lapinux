#!/usr/bin/env bash
# Construit la couche MangoHud corrigée de Lapinux et l'installe dans
# ~/.local/share/lapinux/mangohud.
#
# MangoHud 0.8 calcule la position d'un HUD « en ligne » depuis une largeur de
# fenêtre négative : à droite la ligne sort de l'écran, au centre elle dérive
# et `offset_x` ne la rattrape pas. Le patch `lapinux-inline-anchors.patch`
# ancre la ligne sur la taille réelle de la fenêtre du HUD, ce qui rend les
# neuf positions exactes.
#
# Le build tourne dans l'image toolbox Fedora, comme la distribution : la
# bibliothèque produite se charge dans le loader du système sans dépendance
# supplémentaire. Le manifeste de couche est écrit par Lapinux au lancement
# d'un jeu (`lapinux_perf::mangohud::ensure_vendor_layer`).
#
# Variables :
#   LAPINUX_DATA      dossier de données de Lapinux (défaut : XDG)
#   LAPINUX_BUILD_IMAGE  image du build (défaut : fedora-toolbox:latest)
set -euo pipefail

VERSION=0.8.4
HERE=$(cd "$(dirname "$0")" && pwd)
DATA="${LAPINUX_DATA:-${XDG_DATA_HOME:-$HOME/.local/share}/lapinux}"
DEST="$DATA/mangohud"
WORK="${TMPDIR:-/tmp}/lapinux-mangohud-$VERSION"
IMAGE="${LAPINUX_BUILD_IMAGE:-fedora-toolbox:latest}"
TARBALL="$WORK/MangoHud-$VERSION.tar.gz"

command -v podman >/dev/null || {
    echo "podman est requis pour compiler la couche." >&2
    exit 1
}
command -v curl >/dev/null || {
    echo "curl est requis pour télécharger MangoHud." >&2
    exit 1
}

mkdir -p "$WORK"
if [ ! -f "$TARBALL" ]; then
    echo "Téléchargement de MangoHud v$VERSION…"
    curl -fsSL \
        "https://github.com/flightlessmango/MangoHud/archive/refs/tags/v$VERSION.tar.gz" \
        -o "$TARBALL"
fi

WANT=$(sha256sum "$HERE/lapinux-inline-anchors.patch" | cut -d' ' -f1)
STAMP="$WORK/src/.lapinux-patch"
if [ ! -f "$STAMP" ] || [ "$(cat "$STAMP")" != "$WANT" ]; then
    echo "Application du patch Lapinux…"
    rm -rf "$WORK/src"
    tar xzf "$TARBALL" -C "$WORK"
    mv "$WORK/MangoHud-$VERSION" "$WORK/src"
    patch -d "$WORK/src" -p1 < "$HERE/lapinux-inline-anchors.patch"
    echo "$WANT" > "$STAMP"
fi

rm -rf "$WORK/build"
CID=$(podman create -v "$WORK:/work:Z" "$IMAGE" bash -lc '
    set -e
    dnf install -y --quiet meson ninja-build gcc-c++ glibc-devel pkgconf-pkg-config \
        libX11-devel wayland-devel libxkbcommon-devel dbus-devel vulkan-headers \
        python3-mako libdrm-devel libXNVCtrl-devel glslang libstdc++-static > /dev/null
    meson setup /work/build /work/src -Dwith_x11=enabled -Dwith_wayland=enabled \
        -Dwith_dbus=enabled -Dwith_xnvctrl=enabled -Dwith_nvml=enabled \
        -Dinclude_doc=false -Dtests=disabled -Dmangoplot=disabled
    ninja -C /work/build src/libMangoHud.so
')
trap 'podman rm -f "$CID" >/dev/null 2>&1 || true' EXIT

podman start -a "$CID"
mkdir -p "$DEST"
podman cp "$CID:/work/build/src/libMangoHud.so" "$DEST/libMangoHud.so"
echo "Couche installée : $DEST/libMangoHud.so"
echo "Lapinux l'active au lancement d'un jeu (MANGOHUD=1) et désactive celle du système."

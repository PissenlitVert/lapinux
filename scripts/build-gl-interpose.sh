#!/usr/bin/env bash
# Construit l'interposeur OpenGL de génération d'images et ses clients de
# validation.
#
# L'interposeur prend la présentation d'un jeu OpenGL (EGL/GLX) et la fait
# passer par une swapchain Vulkan, où la couche afmf-linux génère les images du
# multiplicateur comme pour un jeu Vulkan. Rien ne passe par la mémoire du
# processeur : la texture partagée est une image Vulkan dont la mémoire est
# exportée par fd et importée côté OpenGL.
#
# Les en-têtes Vulkan de cette machine ne sont pas installés dans /usr : ils
# viennent du dépôt de la mission (target/vulkan-headers-mission). Aucun
# en-tête OpenGL, EGL, Wayland ou X11 n'est requis : les quelques dizaines de
# déclarations utiles sont dans les sources.
#
# Usage : scripts/build-gl-interpose.sh
#   GL_INTERPOSE_OUT, VULKAN_HEADERS remplacent les chemins.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="${GL_INTERPOSE_OUT:-$ROOT/dist/gl-framegen}"
HEADERS="${VULKAN_HEADERS:-$ROOT/target/vulkan-headers-mission/include}"
SRC="$ROOT/tools/gl-framegen"
CC="${CC:-cc}"

if [ ! -f "$HEADERS/vulkan/vulkan.h" ]; then
    echo "en-têtes Vulkan absents : $HEADERS" >&2
    echo "renseigne VULKAN_HEADERS ou prépare le dépôt de la mission" >&2
    exit 1
fi

mkdir -p "$OUT"

# La couche elle-même n'est pas liée : elle arrive par le manifeste Vulkan
# (VK_ADD_IMPLICIT_LAYER_PATH ou l'installation locale de Lapinux).
"$CC" -O2 -Wall -Wextra -I "$HEADERS" -fPIC -shared \
    -o "$OUT/liblapinux_gl_afmf.so" "$SRC/lapinux_gl_afmf.c" \
    -ldl -l:libvulkan.so.1

# Sonde de précondition : un périphérique Vulkan se crée-t-il encore ?
"$CC" -O2 -Wall -Wextra -I "$HEADERS" -o "$OUT/vulkan-probe" "$SRC/vulkan-probe.c" \
    -l:libvulkan.so.1

# Client de référence : Wayland natif, GLES2, sans en-têtes de développement.
"$CC" -O2 -Wall -Wextra -o "$OUT/gl-client-wayland" "$SRC/gl-client-wayland.c" \
    -ldl -lm

# Second client de référence : GLX sous X11 (Xwayland).
"$CC" -O2 -Wall -Wextra -o "$OUT/gl-client-x11" "$SRC/gl-client-x11.c" -ldl -lm

cat <<EOF
construit dans $OUT :
  liblapinux_gl_afmf.so   interposeur (LD_PRELOAD)
  gl-client-wayland       client OpenGL(GLES2)/EGL Wayland de validation
  gl-client-x11           client OpenGL/GLX X11 de validation
  vulkan-probe            vérifie qu'un périphérique Vulkan se crée

exemple :
  VK_ADD_IMPLICIT_LAYER_PATH=\$HOME/.local/share/lapinux/tools/frame-generation/afmf-linux/local \\
  AFMF_ENABLE=1 AFMF_LOG=3 \\
  LD_PRELOAD=$OUT/liblapinux_gl_afmf.so \\
  $OUT/gl-client-wayland 8
EOF
